//! Dispatches every non-2xx-to-non-INVITE and INVITE/BYE response for a
//! dialog. `on_response` classifies a response into a local `Act` (no
//! `.await` while `self.dialogs` is mutably borrowed), then executes it --
//! the actual per-outcome handling lives in `connected.rs`/
//! `session_timers.rs`/`challenge.rs`, split out purely for file size (same
//! precedent as `views/settings/`, `views/dialer/`,
//! `sip-core/src/call/lifecycle/`), not a behavior change.

mod challenge;
mod connected;
mod session_timers;

use std::net::SocketAddr;

use tracing::debug;

use crate::{
    call::dialog::{Dialog, DialogState, PendingOfferMedia},
    client::SipStack,
    events::SipEvent,
    wire::message::{SipMessage, SipMethod},
    wire::util::new_branch,
};

impl SipStack {
    // ── Response handler ──────────────────────────────────────────────────────

    pub(crate) async fn on_response(&mut self, msg: SipMessage, status: u16, _from: SocketAddr) {
        let call_id = match msg.call_id() {
            Some(id) => id.to_string(),
            None => return,
        };
        if call_id == self.reg_call_id {
            return;
        }

        // REFER responses (blind transfer) don't follow the INVITE/BYE 200
        // convention (success is 202 Accepted) — handle by CSeq method
        // before the status-keyed dispatch below.
        if matches!(msg.cseq(), Some((_, SipMethod::Refer))) {
            let ev = if status < 300 {
                SipEvent::TransferAccepted { call_id }
            } else {
                SipEvent::TransferFailed { call_id, reason: format!("{status}") }
            };
            let _ = self.event_tx.send(ev);
            return;
        }

        // SUBSCRIBE responses don't follow the INVITE/BYE convention either
        // (success is 200 OK but with Expires semantics, no ACK) and aren't
        // call dialogs at all -- handled entirely against `self.subscriptions`
        // / `self.mwi_subscriptions`, whichever map this call-id belongs to.
        if matches!(msg.cseq(), Some((_, SipMethod::Subscribe))) {
            if self.subscriptions.contains_key(&call_id) {
                self.on_presence_subscribe_response(msg, status, call_id).await;
            } else if self.mwi_subscriptions.contains_key(&call_id) {
                self.on_mwi_subscribe_response(msg, status, call_id).await;
            }
            return;
        }

        // MESSAGE (RFC 3428) is a standalone transaction, not a `Dialog` --
        // resolved against `pending_messages` instead.
        if matches!(msg.cseq(), Some((_, SipMethod::Message))) {
            self.on_message_response(msg, status, call_id).await;
            return;
        }

        // PUBLISH (RFC 3903), our own outgoing presence -- resolved against
        // `self.presence_publish`, not a dialog either.
        if matches!(msg.cseq(), Some((_, SipMethod::Publish))) {
            self.on_publish_response(msg, status, call_id).await;
            return;
        }

        // See docs/crates/sip-core.md's "SipEvent/Act left un-boxed" note.
        #[allow(clippy::large_enum_variant)]
        enum Act {
            Nothing,
            Ringing,
            Connected {
                call_id: String,
                remote_sdp: String,
                pending_offer: Option<PendingOfferMedia>,
                ice_gathered: Option<deelip_nat::IceGathered>,
                ack_cid: String,
                ack_from_tag: String,
                ack_to_uri: String,
                ack_to_tag: Option<String>,
                ack_cseq: u32,
                session_expires_hdr: Option<(u32, Option<String>)>,
            },
            ReInviteAck {
                call_id: String,
                hold: bool,
                ack_cid: String,
                ack_from_tag: String,
                ack_to_uri: String,
                ack_to_tag: Option<String>,
                ack_cseq: u32,
            },
            /// RFC 4028 Session Timers: 200 OK to our own refresh re-INVITE
            /// (see `send_session_refresh`) -- just an ACK, no hold/resume
            /// event, unlike `ReInviteAck`.
            SessionRefreshAck {
                call_id: String,
                ack_cid: String,
                ack_from_tag: String,
                ack_to_uri: String,
                ack_to_tag: Option<String>,
                ack_cseq: u32,
                session_expires_hdr: Option<(u32, Option<String>)>,
            },
            ByeOk(String),
            Failed {
                call_id: String,
                code: u16,
                reason: String,
            },
            InviteChallenged {
                call_id: String,
                to_uri: String,
                local_sdp: String,
                challenge_raw: String,
                ack_cid: String,
                ack_from_tag: String,
                ack_to_uri: String,
                ack_to_tag: Option<String>,
                ack_cseq: u32,
            },
            /// 422 Session Interval Too Small (RFC 4028) -- retry once with
            /// a `Session-Expires` at least as large as the response's own
            /// `Min-SE`.
            SessionIntervalTooSmall {
                call_id: String,
                to_uri: String,
                local_sdp: String,
                min_se: u32,
                ack_cid: String,
                ack_from_tag: String,
                ack_to_uri: String,
                ack_to_tag: Option<String>,
                ack_cseq: u32,
            },
        }

        // RFC 4028 Session Timers: parsed once here (while `msg` is still in
        // scope) rather than threaded into the handler methods below --
        // `Act::Connected`/`Act::SessionRefreshAck` carry the parsed result
        // forward instead of the message itself.
        let session_expires_hdr = msg.header("Session-Expires").and_then(crate::wire::util::parse_session_expires);

        let act = 'blk: {
            let Some(dialog) = self.dialogs.get_mut(&call_id) else {
                debug!(call_id, "Response for unknown dialog");
                break 'blk Act::Nothing;
            };
            match status {
                180 => {
                    dialog.state = DialogState::Ringing;
                    Act::Ringing
                }
                200 => {
                    let Some((cseq_n, method)) = msg.cseq() else {
                        break 'blk Act::Nothing;
                    };
                    match method {
                        SipMethod::Invite => {
                            match dialog.state {
                                DialogState::Calling | DialogState::Ringing => {
                                    // Initial call connected
                                    dialog.state = DialogState::Confirmed;
                                    dialog.remote_tag = crate::wire::util::parse_tag(msg.header("To").unwrap_or(""));
                                    dialog.remote_sdp = Some(String::from_utf8_lossy(&msg.body).into_owned());
                                    // Must populate `remote_contact` here too, not just on the
                                    // callee side -- see docs/crates/sip-core.md's
                                    // "Dialog::remote_contact must be populated on the caller
                                    // side too" note (a real bug, confirmed live).
                                    dialog.remote_contact = Dialog::parse_remote_contact(msg.header("Contact"));
                                    Act::Connected {
                                        call_id: dialog.call_id.clone(),
                                        remote_sdp: dialog.remote_sdp.clone().unwrap_or_default(),
                                        pending_offer: dialog.pending_offer.take(),
                                        ice_gathered: dialog.ice_gathered.take(),
                                        ack_cid: dialog.call_id.clone(),
                                        ack_from_tag: dialog.local_tag.clone(),
                                        ack_to_uri: dialog.remote_uri.clone(),
                                        ack_to_tag: dialog.remote_tag.clone(),
                                        ack_cseq: cseq_n,
                                        session_expires_hdr: session_expires_hdr.clone(),
                                    }
                                }
                                DialogState::Confirmed if dialog.session_refresh_pending => {
                                    // Must check this before the hold/resume path below --
                                    // see "Session-refresh vs. hold/resume disambiguation"
                                    // in docs/crates/sip-core.md.
                                    dialog.session_refresh_pending = false;
                                    Act::SessionRefreshAck {
                                        call_id: dialog.call_id.clone(),
                                        ack_cid: dialog.call_id.clone(),
                                        ack_from_tag: dialog.local_tag.clone(),
                                        ack_to_uri: dialog.remote_uri.clone(),
                                        ack_to_tag: dialog.remote_tag.clone(),
                                        ack_cseq: cseq_n,
                                        session_expires_hdr: session_expires_hdr.clone(),
                                    }
                                }
                                DialogState::Confirmed => {
                                    // re-INVITE response (hold/resume)
                                    let hold = dialog.hold_pending.take().unwrap_or(true);
                                    dialog.is_held = hold;
                                    Act::ReInviteAck {
                                        call_id: dialog.call_id.clone(),
                                        hold,
                                        ack_cid: dialog.call_id.clone(),
                                        ack_from_tag: dialog.local_tag.clone(),
                                        ack_to_uri: dialog.remote_uri.clone(),
                                        ack_to_tag: dialog.remote_tag.clone(),
                                        ack_cseq: cseq_n,
                                    }
                                }
                                _ => Act::Nothing,
                            }
                        }
                        SipMethod::Bye => {
                            dialog.state = DialogState::Terminated;
                            Act::ByeOk(dialog.call_id.clone())
                        }
                        _ => Act::Nothing,
                    }
                }
                100 => Act::Nothing,
                401 | 407 if dialog.state == DialogState::Calling && !dialog.auth_retried => {
                    let Some((cseq_n, SipMethod::Invite)) = msg.cseq() else {
                        break 'blk Act::Failed {
                            call_id: call_id.clone(),
                            code: status,
                            reason: "Unauthorized".into(),
                        };
                    };
                    let hdr_name = if status == 407 { "Proxy-Authenticate" } else { "WWW-Authenticate" };
                    let Some(challenge_raw) = msg.header(hdr_name).map(str::to_string) else {
                        break 'blk Act::Failed {
                            call_id: call_id.clone(),
                            code: status,
                            reason: "Missing auth challenge".into(),
                        };
                    };
                    dialog.auth_retried = true;
                    Act::InviteChallenged {
                        call_id: dialog.call_id.clone(),
                        to_uri: dialog.remote_uri.clone(),
                        local_sdp: dialog.local_sdp.clone().unwrap_or_default(),
                        challenge_raw,
                        ack_cid: dialog.call_id.clone(),
                        ack_from_tag: dialog.local_tag.clone(),
                        ack_to_uri: dialog.remote_uri.clone(),
                        ack_to_tag: dialog.remote_tag.clone(),
                        ack_cseq: cseq_n,
                    }
                }
                422 if dialog.state == DialogState::Calling && !dialog.session_expires_retried => {
                    let Some((cseq_n, SipMethod::Invite)) = msg.cseq() else {
                        break 'blk Act::Failed {
                            call_id: call_id.clone(),
                            code: status,
                            reason: "Session Interval Too Small".into(),
                        };
                    };
                    let Some(min_se) = msg.header("Min-SE").and_then(|v| v.trim().parse::<u32>().ok()) else {
                        break 'blk Act::Failed {
                            call_id: call_id.clone(),
                            code: status,
                            reason: "422 with no Min-SE".into(),
                        };
                    };
                    Act::SessionIntervalTooSmall {
                        call_id: dialog.call_id.clone(),
                        to_uri: dialog.remote_uri.clone(),
                        local_sdp: dialog.local_sdp.clone().unwrap_or_default(),
                        min_se,
                        ack_cid: dialog.call_id.clone(),
                        ack_from_tag: dialog.local_tag.clone(),
                        ack_to_uri: dialog.remote_uri.clone(),
                        ack_to_tag: dialog.remote_tag.clone(),
                        ack_cseq: cseq_n,
                    }
                }
                c if c >= 300 => Act::Failed {
                    call_id: call_id.clone(),
                    code: c,
                    reason: msg.reason_phrase().unwrap_or("").to_string(),
                },
                _ => Act::Nothing,
            }
        }; // mutable borrow of self.dialogs released here

        match act {
            Act::Nothing => {}
            Act::Ringing => {
                let _ = self.event_tx.send(SipEvent::CallRinging { call_id });
            }
            Act::Connected {
                call_id,
                remote_sdp,
                pending_offer,
                ice_gathered,
                ack_cid,
                ack_from_tag,
                ack_to_uri,
                ack_to_tag,
                ack_cseq,
                session_expires_hdr,
            } => {
                self.handle_connected(
                    call_id,
                    remote_sdp,
                    pending_offer,
                    ice_gathered,
                    ack_cid,
                    ack_from_tag,
                    ack_to_uri,
                    ack_to_tag,
                    ack_cseq,
                    session_expires_hdr,
                )
                .await;
            }
            Act::ReInviteAck { call_id, hold, ack_cid, ack_from_tag, ack_to_uri, ack_to_tag, ack_cseq } => {
                self.handle_reinvite_ack(call_id, hold, ack_cid, ack_from_tag, ack_to_uri, ack_to_tag, ack_cseq).await;
            }
            Act::SessionRefreshAck {
                call_id,
                ack_cid,
                ack_from_tag,
                ack_to_uri,
                ack_to_tag,
                ack_cseq,
                session_expires_hdr,
            } => {
                self.handle_session_refresh_ack(
                    call_id,
                    ack_cid,
                    ack_from_tag,
                    ack_to_uri,
                    ack_to_tag,
                    ack_cseq,
                    session_expires_hdr,
                )
                .await;
            }
            Act::ByeOk(id) => {
                self.dialogs.remove(&id);
                let _ = self.event_tx.send(SipEvent::CallEnded { call_id: id });
            }
            Act::Failed { call_id, code, reason } => {
                self.dialogs.remove(&call_id);
                let _ = self.event_tx.send(SipEvent::CallFailed { call_id, code, reason });
            }
            Act::InviteChallenged {
                call_id,
                to_uri,
                local_sdp,
                challenge_raw,
                ack_cid,
                ack_from_tag,
                ack_to_uri,
                ack_to_tag,
                ack_cseq,
            } => {
                self.handle_invite_challenged(
                    call_id,
                    to_uri,
                    local_sdp,
                    challenge_raw,
                    ack_cid,
                    ack_from_tag,
                    ack_to_uri,
                    ack_to_tag,
                    ack_cseq,
                )
                .await;
            }
            Act::SessionIntervalTooSmall {
                call_id,
                to_uri,
                local_sdp,
                min_se,
                ack_cid,
                ack_from_tag,
                ack_to_uri,
                ack_to_tag,
                ack_cseq,
            } => {
                self.handle_session_interval_too_small(
                    call_id,
                    to_uri,
                    local_sdp,
                    min_se,
                    ack_cid,
                    ack_from_tag,
                    ack_to_uri,
                    ack_to_tag,
                    ack_cseq,
                )
                .await;
            }
        }
    }

    #[allow(clippy::too_many_arguments)] // mirrors the `Act::ReInviteAck` variant's field set
    async fn handle_reinvite_ack(
        &mut self, call_id: String, hold: bool, ack_cid: String, ack_from_tag: String, ack_to_uri: String,
        ack_to_tag: Option<String>, ack_cseq: u32,
    ) {
        let ack = self.build_ack(&ack_cid, &ack_from_tag, &ack_to_uri, ack_to_tag.as_deref(), ack_cseq, &new_branch());
        let _ = self.transport.send(ack.as_bytes(), self.server_addr).await;
        let ev = if hold { SipEvent::CallHeld { call_id } } else { SipEvent::CallResumed { call_id } };
        let _ = self.event_tx.send(ev);
    }
}
