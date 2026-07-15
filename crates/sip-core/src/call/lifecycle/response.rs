//! Dispatches every non-2xx-to-non-INVITE and INVITE/BYE response for a
//! dialog. `on_response` classifies a response into a local `Act` (no
//! `.await` while `self.dialogs` is mutably borrowed), then executes it.

use std::net::SocketAddr;

use tracing::debug;

use crate::{
    call::dialog::{DialogState, PendingOfferMedia},
    call::media_setup,
    client::{OutgoingVideoConnected, SipStack, StackEvent},
    events::SipEvent,
    wire::auth::build_challenge_response,
    wire::message::{SipMessage, SipMethod},
    wire::sdp::{Setup, parse_sdp, parse_video_section, split_media_sections},
    wire::util::new_branch,
};

use super::VIDEO_CODECS;

/// RFC 4145 §4.1: as the offerer, our final role is simply the complement
/// of whatever the answer committed to. `None` if the answer didn't
/// actually negotiate DTLS-SRTP at all (no fingerprint/setup, or an
/// answer nonsensically echoing `actpass` instead of resolving it) --
/// callers treat that the same as the remote not supporting DTLS-SRTP.
fn resolve_offerer_dtls_role(answered_setup: Option<Setup>) -> Option<Setup> {
    match answered_setup {
        Some(Setup::Active) => Some(Setup::Passive),
        Some(Setup::Passive) => Some(Setup::Active),
        Some(Setup::ActPass) | None => None,
    }
}

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

        // `Connected`'s `pending_offer: Option<PendingOfferMedia>` now carries
        // `DtlsCallParams` (cert/key DER bytes), making it noticeably larger
        // than `Act`'s other variants -- deliberately not boxed, matching
        // `EventSender::send`'s own established precedent for `SipEvent`
        // (see its doc comment) of accepting this cost rather than adding
        // indirection to every construction/match site.
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
                                    // Session-refresh re-INVITE response --
                                    // must not fall into the hold/resume
                                    // path below (`hold_pending` was never
                                    // set for this one, so it would default
                                    // to `true` and wrongly report the call
                                    // as held).
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

    #[allow(clippy::too_many_arguments)] // mirrors the `Act::Connected` variant's own
    // field set one-for-one; a struct wrapper would
    // just rename this same list, not shrink it.
    async fn handle_connected(
        &mut self, call_id: String, remote_sdp: String, pending_offer: Option<PendingOfferMedia>,
        ice_gathered: Option<deelip_nat::IceGathered>, ack_cid: String, ack_from_tag: String, ack_to_uri: String,
        ack_to_tag: Option<String>, ack_cseq: u32, session_expires_hdr: Option<(u32, Option<String>)>,
    ) {
        // A 2xx ACK is a new transaction in its own right (RFC 3261
        // §13.2.2.4) -- unlike a non-2xx ACK, it gets a fresh branch.
        let ack = self.build_ack(&ack_cid, &ack_from_tag, &ack_to_uri, ack_to_tag.as_deref(), ack_cseq, &new_branch());
        let _ = self.transport.send(ack.as_bytes(), self.server_addr).await;

        // RFC 4028 Session Timers: fold the response's negotiated
        // `Session-Expires`/`refresher=` into the dialog -- absent
        // entirely if the far end doesn't support them (no
        // `Session-Expires` in the 200 OK), regardless of whether we
        // proposed one ourselves.
        if let Some((interval, refresher)) = session_expires_hdr
            && let Some(dialog) = self.dialogs.get_mut(&call_id)
        {
            dialog.session_expires = Some(interval);
            dialog.we_are_refresher = refresher.as_deref() != Some("uas");
            if dialog.we_are_refresher {
                dialog.session_refresh_at =
                    Some(tokio::time::Instant::now() + tokio::time::Duration::from_secs((interval / 2).max(1) as u64));
            }
        }

        let codecs = media_setup::account_codecs(&self.account);
        let Some(parsed) = parse_sdp(&remote_sdp, &codecs) else {
            // We've already ACKed the 2xx -- both sides consider this
            // dialog Confirmed, so just dropping our own map entry
            // would leave the far end's side dangling forever with no
            // teardown signal. Send a real BYE, unlike the pre-ACK
            // failure paths elsewhere in this function (401/407 with
            // a bad challenge, etc.), which only ever reach a Calling
            // dialog the far end doesn't yet consider established.
            self.hang_up(&call_id).await;
            self.dialogs.remove(&call_id);
            let _ = self.event_tx.send(SipEvent::CallFailed {
                call_id,
                code: 0,
                reason: "No compatible codec in answer".into(),
            });
            return;
        };
        let Some(PendingOfferMedia { local_rtp, local_srtp, relay, local_dtls }) = pending_offer else {
            debug!(call_id, "Connected with no pending offer media -- dropping");
            return;
        };

        // Resolve our final DTLS-SRTP role now that the answer's
        // `a=setup` is known -- `None` (dropping `local_dtls` entirely)
        // if the answer didn't actually negotiate it, same fallback
        // shape `resolve_srtp_and_relay` already uses for SDES.
        let local_dtls = local_dtls.and_then(|mut params| match resolve_offerer_dtls_role(parsed.setup) {
            Some(role) if parsed.fingerprint.is_some() => {
                params.role = Some(role);
                params.remote_fingerprint = parsed.fingerprint.clone();
                Some(params)
            }
            _ => {
                tracing::warn!("DTLS-SRTP requested but answer didn't negotiate it -- falling back to plaintext RTP");
                None
            }
        });

        // Video (negotiation only): if we offered a video leg,
        // parse the answer's own video section (a separate,
        // independent parse of `remote_sdp` -- never folded into
        // `parsed`/`ParsedSdp`, same as `accept_call`'s treatment).
        let pending_offer_video = self.dialogs.get_mut(&call_id).and_then(|d| d.pending_offer_video.take());
        let parsed_video = pending_offer_video.as_ref().and_then(|_| {
            split_media_sections(&remote_sdp)
                .into_iter()
                .find(|(m_line, _)| m_line.starts_with("m=video "))
                .and_then(|(m_line, attrs)| parse_video_section(m_line, &attrs, &VIDEO_CODECS))
        });

        // ICE connectivity checks (RFC 8445) are real network I/O,
        // same reasoning as `initiate_call`/`accept_call` -- defer to
        // a background task rather than blocking this whole account's
        // event loop on `finish_ice_connect`'s multi-second timeout.
        let internal_tx = self.internal_tx.clone();
        let remote_rtp = parsed.rtp_addr;
        let remote_srtp = parsed.srtp.clone();
        let codec = parsed.codec;
        let dtmf_type = parsed.dtmf_type;
        let cn_type = parsed.cn_type;
        tokio::spawn(async move {
            let ice = media_setup::finish_ice_connect(ice_gathered, true, &parsed).await;

            let video = match (pending_offer_video, parsed_video) {
                (Some(pending_video), Some(parsed_video)) => {
                    let video_ice = media_setup::finish_ice_connect_raw(
                        pending_video.ice_gathered,
                        true,
                        parsed_video.ice_ufrag.as_deref(),
                        parsed_video.ice_pwd.as_deref(),
                        &parsed_video.ice_candidates,
                    )
                    .await;
                    Some(OutgoingVideoConnected {
                        local_rtp: pending_video.local_rtp,
                        local_srtp: pending_video.local_srtp,
                        relay: pending_video.relay,
                        ice: video_ice,
                        codec: parsed_video.codec,
                        remote_rtp: parsed_video.rtp_addr,
                        remote_srtp: parsed_video.srtp,
                    })
                }
                _ => None,
            };

            let _ = internal_tx.send(StackEvent::OutgoingConnected {
                call_id,
                local_rtp,
                local_srtp,
                relay,
                ice,
                codec,
                dtmf_type,
                cn_type,
                remote_rtp,
                remote_srtp,
                local_dtls,
                video,
            });
        });
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

    #[allow(clippy::too_many_arguments)] // mirrors the `Act::SessionRefreshAck` variant's field set
    async fn handle_session_refresh_ack(
        &mut self, call_id: String, ack_cid: String, ack_from_tag: String, ack_to_uri: String,
        ack_to_tag: Option<String>, ack_cseq: u32, session_expires_hdr: Option<(u32, Option<String>)>,
    ) {
        let ack = self.build_ack(&ack_cid, &ack_from_tag, &ack_to_uri, ack_to_tag.as_deref(), ack_cseq, &new_branch());
        let _ = self.transport.send(ack.as_bytes(), self.server_addr).await;
        // Reschedule from this response's (possibly renegotiated)
        // Session-Expires -- same parsing as the initial INVITE's
        // 200 OK in `handle_connected` above.
        if let Some((interval, refresher)) = session_expires_hdr
            && let Some(dialog) = self.dialogs.get_mut(&call_id)
        {
            dialog.session_expires = Some(interval);
            dialog.we_are_refresher = refresher.as_deref() != Some("uas");
            if dialog.we_are_refresher {
                dialog.session_refresh_at =
                    Some(tokio::time::Instant::now() + tokio::time::Duration::from_secs((interval / 2).max(1) as u64));
            }
        }
    }

    #[allow(clippy::too_many_arguments)] // mirrors the `Act::InviteChallenged` variant's field set
    async fn handle_invite_challenged(
        &mut self, call_id: String, to_uri: String, local_sdp: String, challenge_raw: String, ack_cid: String,
        ack_from_tag: String, ack_to_uri: String, ack_to_tag: Option<String>, ack_cseq: u32,
    ) {
        // ACK to a non-2xx response must reuse the *original*
        // INVITE's branch (RFC 3261 §17.1.1.3), unlike a 2xx ACK
        // which is a new transaction with its own fresh branch.
        let Some(invite_branch) = self.dialogs.get(&call_id).map(|d| d.invite_branch.clone()) else {
            return;
        };
        let ack = self.build_ack(&ack_cid, &ack_from_tag, &ack_to_uri, ack_to_tag.as_deref(), ack_cseq, &invite_branch);
        let _ = self.transport.send(ack.as_bytes(), self.server_addr).await;

        let Some(auth) = build_challenge_response(
            self.account.auth_username(),
            &self.account.password,
            "INVITE",
            &to_uri,
            &challenge_raw,
        ) else {
            self.dialogs.remove(&call_id);
            let _ =
                self.event_tx.send(SipEvent::CallFailed { call_id, code: 401, reason: "Bad auth challenge".into() });
            return;
        };

        let Some(dialog) = self.dialogs.get_mut(&call_id) else {
            return;
        };
        let cseq = dialog.next_local_cseq();
        let branch = new_branch();
        dialog.invite_branch = branch.clone();
        let dialog_call_id = dialog.call_id.clone();
        let dialog_from_tag = dialog.local_tag.clone();
        let msg = self.build_invite(
            &dialog_call_id,
            &dialog_from_tag,
            cseq,
            &to_uri,
            &local_sdp,
            Some(&auth),
            &branch,
            crate::client::SESSION_EXPIRES_DEFAULT,
        );
        debug!("→ INVITE {to_uri} (authenticated)");
        let _ = self.transport.send(msg.as_bytes(), self.server_addr).await;
    }

    #[allow(clippy::too_many_arguments)] // mirrors the `Act::SessionIntervalTooSmall` variant's field set
    async fn handle_session_interval_too_small(
        &mut self, call_id: String, to_uri: String, local_sdp: String, min_se: u32, ack_cid: String,
        ack_from_tag: String, ack_to_uri: String, ack_to_tag: Option<String>, ack_cseq: u32,
    ) {
        let Some(invite_branch) = self.dialogs.get(&call_id).map(|d| d.invite_branch.clone()) else {
            return;
        };
        let ack = self.build_ack(&ack_cid, &ack_from_tag, &ack_to_uri, ack_to_tag.as_deref(), ack_cseq, &invite_branch);
        let _ = self.transport.send(ack.as_bytes(), self.server_addr).await;

        let Some(dialog) = self.dialogs.get_mut(&call_id) else {
            return;
        };
        let cseq = dialog.next_local_cseq();
        let branch = new_branch();
        dialog.invite_branch = branch.clone();
        dialog.session_expires_retried = true;
        let dialog_call_id = dialog.call_id.clone();
        let dialog_from_tag = dialog.local_tag.clone();
        let session_expires = min_se.max(crate::client::SESSION_EXPIRES_DEFAULT);
        let msg = self.build_invite(
            &dialog_call_id,
            &dialog_from_tag,
            cseq,
            &to_uri,
            &local_sdp,
            None,
            &branch,
            session_expires,
        );
        debug!("→ INVITE {to_uri} (Session-Expires: {session_expires}, retried after 422)");
        let _ = self.transport.send(msg.as_bytes(), self.server_addr).await;
    }

    /// Finish connecting an outgoing call once `on_response`'s background
    /// ICE-connect resolution completes: fold the result into `CallMedia`/
    /// `CallMediaReady` and emit `CallConnected`. A no-op if the dialog is
    /// already gone (hung up, CANCELed, or failed some other way while this
    /// was resolving in the background) -- there's nothing left to connect.
    pub(crate) async fn on_outgoing_connected(&mut self, ev: StackEvent) {
        let StackEvent::OutgoingConnected {
            call_id,
            local_rtp,
            local_srtp,
            relay,
            ice,
            codec,
            dtmf_type,
            cn_type,
            remote_rtp,
            remote_srtp,
            local_dtls,
            video,
        } = ev
        else {
            unreachable!("caller already matched this variant")
        };

        if !self.dialogs.contains_key(&call_id) {
            return;
        }
        let wants_srtp = self.account.wants_srtp(self.resolved_transport);
        let (mut media, mut ready) = media_setup::resolve_call_media(
            local_rtp,
            local_srtp,
            relay,
            ice,
            codec,
            dtmf_type,
            cn_type,
            remote_rtp,
            remote_srtp,
            wants_srtp,
            local_dtls,
        );
        debug!(call_id, video_negotiated = video.is_some(), "Outgoing call connected");
        if let Some(v) = video {
            let (video_media, video_ready) = media_setup::resolve_video_media(
                v.local_rtp,
                v.local_srtp,
                v.relay,
                v.ice,
                v.codec,
                v.remote_rtp,
                v.remote_srtp,
                wants_srtp,
            );
            media.video = Some(video_media);
            ready.video = Some(video_ready);
        }
        self.dialogs.get_mut(&call_id).expect("checked above").media = Some(media);
        let _ = self.event_tx.send(SipEvent::CallConnected { call_id, media: ready });
    }
}
