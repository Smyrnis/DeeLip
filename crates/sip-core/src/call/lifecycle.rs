use std::net::SocketAddr;
use std::sync::Arc;

use tracing::{debug, error};
use webrtc_util::Conn;

use crate::{
    call::dialog::{CallMedia, Dialog, DialogState, PendingOfferMedia},
    call::media_setup,
    client::SipStack,
    events::{CallMediaReady, SipEvent},
    wire::auth::build_challenge_response,
    wire::message::{SipMessage, SipMethod},
    wire::sdp::{build_answer, build_hold_offer, build_offer, build_resume_offer, parse_sdp, IceAttrs, SrtpParams, SrtpSession},
    wire::util::{new_branch, new_call_id, new_tag, parse_tag, parse_uri},
};

impl SipStack {
    // ── Outgoing call ─────────────────────────────────────────────────────────

    /// `attempt_ice` lets the caller opt this specific call out of ICE even
    /// when it's enabled globally (see `SipCommand::MakeCall`'s doc comment).
    pub(crate) async fn initiate_call(&mut self, to: &str, attempt_ice: bool) {
        let call_id  = new_call_id(&self.local_ip);
        let from_tag = new_tag();
        let branch   = new_branch();

        let local_rtp = match deelip_nat::alloc_rtp_port() {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to allocate local RTP port: {e}");
                let _ = self.event_tx.send(SipEvent::CallFailed {
                    call_id, code: 0, reason: "Local RTP port allocation failed".into(),
                });
                return;
            }
        };
        let mut relay = None;
        let ice_gathered = if attempt_ice { media_setup::try_gather_ice(&self.network, true).await } else { None };
        let ice_attrs = ice_gathered.as_ref().map(|g| {
            IceAttrs { ufrag: g.local_ufrag.clone(), pwd: g.local_pwd.clone(), candidates: g.candidates.clone() }
        });
        // Same reasoning as the pre-move code this replaced: the plain c=/m=
        // fallback address is deliberately never the ICE agent's own
        // gathered candidate socket -- that only becomes usable once the
        // answer confirms the far end also speaks ICE (`on_response`'s
        // `Act::Connected` handling), and if it doesn't, the ICE agent (and
        // that socket) is simply dropped. Advertising it here and binding an
        // unrelated `local_rtp` on connect would leave the far end sending
        // RTP to a socket nothing is listening on.
        let (rtp_ip, rtp_port) = media_setup::resolve_rtp_endpoint(&self.network, &self.advertised_ip, local_rtp, &mut relay).await;

        let account_secure = self.account.transport == deelip_config::TransportProtocol::Tls;
        let srtp = if account_secure { Some(SrtpParams::generate()) } else { None };
        let codecs = media_setup::account_codecs(&self.account);
        let sdp = build_offer(&rtp_ip, rtp_port, srtp.as_ref(), &codecs, ice_attrs.as_ref());

        let mut dialog = Dialog::new_outgoing(call_id.clone(), from_tag.clone(), to.to_string());
        dialog.local_sdp     = Some(sdp.clone());
        dialog.invite_branch = branch.clone();
        dialog.ice_gathered  = ice_gathered;
        dialog.pending_offer = Some(PendingOfferMedia { local_rtp, local_srtp: srtp, relay });

        let msg = self.build_invite(&dialog.call_id, &dialog.local_tag, dialog.local_cseq, to, &sdp, None, &branch);
        debug!("→ INVITE {to}");
        if let Err(e) = self.transport.send(msg.as_bytes(), self.server_addr).await {
            error!("Failed to send INVITE: {e}");
            return;
        }
        self.dialogs.insert(call_id, dialog);
    }

    #[allow(clippy::too_many_arguments)] // each param is a distinct, meaningfully-named
                                          // piece of an INVITE's identity; bundling them
                                          // into a struct wouldn't reduce real complexity here.
    fn build_invite(
        &self,
        call_id:  &str,
        from_tag: &str,
        cseq:     u32,
        to:       &str,
        sdp:      &str,
        auth:     Option<&str>,
        branch:   &str,
    ) -> String {
        let server     = &self.account.server;
        let username   = &self.account.username;
        let adv_ip     = &self.advertised_ip;
        let local_ip   = &self.local_ip;
        let local_port = self.local_port;
        let display    = self.account.display_name.as_deref().unwrap_or(username);
        let body_len   = sdp.len();
        let via_proto  = self.via_proto();
        let contact_transport = self.contact_transport_param();

        let mut msg = format!(
            "INVITE {to} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{to}>\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} INVITE\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
             Content-Type: application/sdp\r\n\
             User-Agent: DeeLip/0.1.0\r\n"
        );
        if let Some(a) = auth { msg.push_str(a); msg.push_str("\r\n"); }
        msg.push_str(&format!("Content-Length: {body_len}\r\n\r\n{sdp}"));
        msg
    }

    // ── Hold / Resume (re-INVITE) ─────────────────────────────────────────────

    pub(crate) async fn send_reinvite(&mut self, call_id: &str, hold: bool) {
        let advertised_ip = self.advertised_ip.clone();
        let dialog = match self.dialogs.get_mut(call_id) {
            Some(d) if d.state == DialogState::Confirmed => d,
            _ => return,
        };
        let Some(media) = &dialog.media else { return };
        let (rtp_ip, rtp_port) = match &media.relay {
            Some(r) => (r.relayed_addr.ip().to_string(), r.relayed_addr.port()),
            None    => (advertised_ip, media.local_rtp),
        };
        let local_sdp = if hold {
            build_hold_offer(&rtp_ip, rtp_port, media.codec, media.local_srtp.as_ref())
        } else {
            build_resume_offer(&rtp_ip, rtp_port, media.codec, media.local_srtp.as_ref())
        };
        let local_sdp = local_sdp.as_str();

        let cseq   = dialog.next_local_cseq();
        let branch = new_branch();

        let server     = self.account.server.clone();
        let username   = self.account.username.clone();
        let display    = self.account.display_name.clone().unwrap_or_else(|| username.clone());
        let adv_ip     = self.advertised_ip.clone();
        let local_ip   = self.local_ip.clone();
        let local_port = self.local_port;
        let call_id_s  = dialog.call_id.clone();
        let from_tag   = dialog.local_tag.clone();
        let to_uri     = dialog.remote_uri.clone();
        let to_tag     = dialog.remote_tag.as_deref()
            .map(|t| format!(";tag={t}")).unwrap_or_default();
        let contact: SocketAddr = dialog.remote_contact
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(self.server_addr);
        let body_len = local_sdp.len();

        dialog.hold_pending = Some(hold);
        dialog.local_sdp    = Some(local_sdp.to_string());
        let via_proto = self.via_proto();
        let contact_transport = self.contact_transport_param();

        let reinvite = format!(
            "INVITE {to_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id_s}\r\n\
             CSeq: {cseq} INVITE\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
             Content-Type: application/sdp\r\n\
             User-Agent: DeeLip/0.1.0\r\n\
             Content-Length: {body_len}\r\n\r\n\
             {local_sdp}"
        );
        debug!("→ re-INVITE ({})", if hold { "hold" } else { "resume" });
        let _ = self.transport.send(reinvite.as_bytes(), contact).await;
    }

    /// Send one DTMF digit via SIP INFO (`application/dtmf-relay`, the
    /// long-standing de facto format most PBXes/gateways that support this
    /// scheme at all expect) instead of an RFC 2833 RTP telephone-event
    /// burst. Mirrors `transfer::blind_transfer`'s header shape exactly.
    pub(crate) async fn send_dtmf_info(&mut self, call_id: &str, digit: char) {
        let dialog = match self.dialogs.get_mut(call_id) {
            Some(d) if d.state == DialogState::Confirmed => d,
            _ => return,
        };

        let cseq   = dialog.next_local_cseq();
        let branch = new_branch();

        let server     = self.account.server.clone();
        let username   = self.account.username.clone();
        let display    = self.account.display_name.clone().unwrap_or_else(|| username.clone());
        let adv_ip     = self.advertised_ip.clone();
        let local_ip   = self.local_ip.clone();
        let local_port = self.local_port;
        let call_id_s  = dialog.call_id.clone();
        let from_tag   = dialog.local_tag.clone();
        let to_uri     = dialog.remote_uri.clone();
        let to_tag     = dialog.remote_tag.as_deref()
            .map(|t| format!(";tag={t}")).unwrap_or_default();
        let contact: SocketAddr = dialog.remote_contact
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(self.server_addr);
        let via_proto = self.via_proto();
        let contact_transport = self.contact_transport_param();

        let body = format!("Signal={digit}\r\nDuration=250\r\n");
        let info = format!(
            "INFO {to_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id_s}\r\n\
             CSeq: {cseq} INFO\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
             Content-Type: application/dtmf-relay\r\n\
             User-Agent: DeeLip/0.1.0\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        debug!("→ INFO {to_uri} (DTMF digit={digit})");
        let _ = self.transport.send(info.as_bytes(), contact).await;
    }

    // ── Response handler ──────────────────────────────────────────────────────

    pub(crate) async fn on_response(&mut self, msg: SipMessage, status: u16, _from: SocketAddr) {
        let call_id = match msg.call_id() {
            Some(id) => id.to_string(),
            None     => return,
        };
        if call_id == self.reg_call_id { return; }

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

        enum Act {
            Nothing,
            Ringing,
            Connected {
                call_id: String, remote_sdp: String,
                pending_offer: Option<PendingOfferMedia>, ice_gathered: Option<deelip_nat::IceGathered>,
                ack_cid: String, ack_from_tag: String,
                ack_to_uri: String, ack_to_tag: Option<String>, ack_cseq: u32,
            },
            ReInviteAck {
                call_id: String, hold: bool,
                ack_cid: String, ack_from_tag: String,
                ack_to_uri: String, ack_to_tag: Option<String>, ack_cseq: u32,
            },
            ByeOk(String),
            Failed { call_id: String, code: u16, reason: String },
            InviteChallenged {
                call_id: String, to_uri: String, local_sdp: String, challenge_raw: String,
                ack_cid: String, ack_from_tag: String,
                ack_to_uri: String, ack_to_tag: Option<String>, ack_cseq: u32,
            },
        }

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
                                    dialog.state      = DialogState::Confirmed;
                                    dialog.remote_tag = parse_tag(msg.header("To").unwrap_or(""));
                                    dialog.remote_sdp = Some(
                                        String::from_utf8_lossy(&msg.body).into_owned(),
                                    );
                                    Act::Connected {
                                        call_id:      dialog.call_id.clone(),
                                        remote_sdp:   dialog.remote_sdp.clone().unwrap_or_default(),
                                        pending_offer: dialog.pending_offer.take(),
                                        ice_gathered:  dialog.ice_gathered.take(),
                                        ack_cid:      dialog.call_id.clone(),
                                        ack_from_tag: dialog.local_tag.clone(),
                                        ack_to_uri:   dialog.remote_uri.clone(),
                                        ack_to_tag:   dialog.remote_tag.clone(),
                                        ack_cseq:     cseq_n,
                                    }
                                }
                                DialogState::Confirmed => {
                                    // re-INVITE response (hold/resume)
                                    let hold = dialog.hold_pending.take().unwrap_or(true);
                                    dialog.is_held = hold;
                                    Act::ReInviteAck {
                                        call_id:      dialog.call_id.clone(),
                                        hold,
                                        ack_cid:      dialog.call_id.clone(),
                                        ack_from_tag: dialog.local_tag.clone(),
                                        ack_to_uri:   dialog.remote_uri.clone(),
                                        ack_to_tag:   dialog.remote_tag.clone(),
                                        ack_cseq:     cseq_n,
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
                        break 'blk Act::Failed { call_id: call_id.clone(), code: status, reason: "Unauthorized".into() };
                    };
                    let hdr_name = if status == 407 { "Proxy-Authenticate" } else { "WWW-Authenticate" };
                    let Some(challenge_raw) = msg.header(hdr_name).map(str::to_string) else {
                        break 'blk Act::Failed { call_id: call_id.clone(), code: status, reason: "Missing auth challenge".into() };
                    };
                    dialog.auth_retried = true;
                    Act::InviteChallenged {
                        call_id:      dialog.call_id.clone(),
                        to_uri:       dialog.remote_uri.clone(),
                        local_sdp:    dialog.local_sdp.clone().unwrap_or_default(),
                        challenge_raw,
                        ack_cid:      dialog.call_id.clone(),
                        ack_from_tag: dialog.local_tag.clone(),
                        ack_to_uri:   dialog.remote_uri.clone(),
                        ack_to_tag:   dialog.remote_tag.clone(),
                        ack_cseq:     cseq_n,
                    }
                }
                c if c >= 300 => Act::Failed {
                    call_id: call_id.clone(),
                    code:    c,
                    reason:  msg.reason_phrase().unwrap_or("").to_string(),
                },
                _ => Act::Nothing,
            }
        }; // mutable borrow of self.dialogs released here

        match act {
            Act::Nothing  => {}
            Act::Ringing  => { let _ = self.event_tx.send(SipEvent::CallRinging { call_id }); }
            Act::Connected { call_id, remote_sdp, pending_offer, ice_gathered, ack_cid, ack_from_tag, ack_to_uri, ack_to_tag, ack_cseq } => {
                // A 2xx ACK is a new transaction in its own right (RFC 3261
                // §13.2.2.4) -- unlike a non-2xx ACK, it gets a fresh branch.
                let ack = self.build_ack(&ack_cid, &ack_from_tag, &ack_to_uri, ack_to_tag.as_deref(), ack_cseq, &new_branch());
                let _ = self.transport.send(ack.as_bytes(), self.server_addr).await;

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
                        call_id, code: 0, reason: "No compatible codec in answer".into(),
                    });
                    return;
                };
                let Some(PendingOfferMedia { local_rtp, local_srtp, mut relay }) = pending_offer else {
                    debug!(call_id, "Connected with no pending offer media -- dropping");
                    return;
                };
                let ice_conn = media_setup::finish_ice_connect(ice_gathered, true, &parsed).await;

                let account_secure = self.account.transport == deelip_config::TransportProtocol::Tls;
                let srtp_session = match (&local_srtp, &parsed.srtp) {
                    (Some(local), Some(remote)) => Some(SrtpSession { local: local.clone(), remote: remote.clone() }),
                    _ => {
                        if account_secure {
                            tracing::warn!("TLS signaling active but remote SDP has no a=crypto -- falling back to plaintext RTP");
                        }
                        None
                    }
                };
                let relay_conn: Option<Arc<dyn Conn + Send + Sync>> = ice_conn.as_ref().map(|c| c.conn.clone())
                    .or_else(|| relay.as_ref().map(|r| r.conn.clone()));

                if let Some(dialog) = self.dialogs.get_mut(&call_id) {
                    dialog.media = Some(CallMedia {
                        local_rtp, local_srtp, relay: relay.take(), ice: ice_conn, codec: parsed.codec, dtmf_type: parsed.dtmf_type,
                    });
                }

                let _ = self.event_tx.send(SipEvent::CallConnected {
                    call_id,
                    media: CallMediaReady {
                        codec: parsed.codec, dtmf_type: parsed.dtmf_type, local_rtp,
                        remote_rtp: parsed.rtp_addr, srtp: srtp_session, relay: relay_conn,
                    },
                });
            }
            Act::ReInviteAck { call_id, hold, ack_cid, ack_from_tag, ack_to_uri, ack_to_tag, ack_cseq } => {
                let ack = self.build_ack(&ack_cid, &ack_from_tag, &ack_to_uri, ack_to_tag.as_deref(), ack_cseq, &new_branch());
                let _ = self.transport.send(ack.as_bytes(), self.server_addr).await;
                let ev = if hold {
                    SipEvent::CallHeld { call_id }
                } else {
                    SipEvent::CallResumed { call_id }
                };
                let _ = self.event_tx.send(ev);
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
                call_id, to_uri, local_sdp, challenge_raw,
                ack_cid, ack_from_tag, ack_to_uri, ack_to_tag, ack_cseq,
            } => {
                // ACK to a non-2xx response must reuse the *original*
                // INVITE's branch (RFC 3261 §17.1.1.3), unlike a 2xx ACK
                // which is a new transaction with its own fresh branch.
                let Some(invite_branch) = self.dialogs.get(&call_id).map(|d| d.invite_branch.clone()) else { return };
                let ack = self.build_ack(&ack_cid, &ack_from_tag, &ack_to_uri, ack_to_tag.as_deref(), ack_cseq, &invite_branch);
                let _ = self.transport.send(ack.as_bytes(), self.server_addr).await;

                let Some(auth) = build_challenge_response(
                    &self.account.username, &self.account.password, "INVITE", &to_uri, &challenge_raw,
                ) else {
                    self.dialogs.remove(&call_id);
                    let _ = self.event_tx.send(SipEvent::CallFailed {
                        call_id, code: 401, reason: "Bad auth challenge".into(),
                    });
                    return;
                };

                let Some(dialog) = self.dialogs.get_mut(&call_id) else { return; };
                let cseq   = dialog.next_local_cseq();
                let branch = new_branch();
                dialog.invite_branch = branch.clone();
                let dialog_call_id  = dialog.call_id.clone();
                let dialog_from_tag = dialog.local_tag.clone();
                let msg = self.build_invite(&dialog_call_id, &dialog_from_tag, cseq, &to_uri, &local_sdp, Some(&auth), &branch);
                debug!("→ INVITE {to_uri} (authenticated)");
                let _ = self.transport.send(msg.as_bytes(), self.server_addr).await;
            }
        }
    }

    // ── Incoming INVITE ───────────────────────────────────────────────────────

    pub(crate) async fn on_invite(&mut self, msg: SipMessage, from: SocketAddr) {
        let call_id = match msg.call_id() {
            Some(id) => id.to_string(),
            None     => return,
        };

        // re-INVITE on existing confirmed dialog
        let reinvite_action = if let Some(dialog) = self.dialogs.get_mut(&call_id) {
            if dialog.state == DialogState::Confirmed {
                let body = String::from_utf8_lossy(&msg.body).into_owned();
                let is_sendonly = body.lines().any(|l| l.trim() == "a=sendonly");
                let local_sdp = dialog.local_sdp.clone().unwrap_or_default();
                let local_tag = dialog.local_tag.clone();
                Some((is_sendonly, local_sdp, local_tag))
            } else {
                None
            }
        } else {
            None
        };
        if let Some((is_sendonly, local_sdp, local_tag)) = reinvite_action {
            let ok = self.build_response_with_body(&msg, 200, "OK", &local_tag, &local_sdp);
            let _ = self.transport.send(ok.as_bytes(), from).await;
            let ev = if is_sendonly {
                SipEvent::RemoteHeld   { call_id: call_id.clone() }
            } else {
                SipEvent::RemoteResumed { call_id: call_id.clone() }
            };
            let _ = self.event_tx.send(ev);
            return;
        }

        // Fresh INVITE
        let from_hdr  = msg.header("From").unwrap_or("").to_string();
        let from_uri  = parse_uri(&from_hdr).unwrap_or_else(|| from_hdr.clone());
        let from_tag  = parse_tag(&from_hdr).unwrap_or_default();
        let (cseq_n, _) = msg.cseq().unwrap_or((1, SipMethod::Invite));
        let remote_sdp = String::from_utf8_lossy(&msg.body).into_owned();
        let remote_via = msg.header("Via").unwrap_or("").to_string();
        let local_tag  = new_tag();

        debug!("← INVITE from {from_uri} ({from})");
        let trying  = self.build_response(&msg, 100, "Trying",  &local_tag, "");
        let ringing = self.build_response(&msg, 180, "Ringing", &local_tag, "");
        let _ = self.transport.send(trying.as_bytes(),  from).await;
        let _ = self.transport.send(ringing.as_bytes(), from).await;

        let mut dialog = Dialog::new_incoming(
            call_id.clone(), local_tag, from_uri.clone(),
            from_tag, cseq_n, remote_sdp.clone(), remote_via,
        );
        dialog.remote_contact = Some(from.to_string());
        self.dialogs.insert(call_id.clone(), dialog);

        let _ = self.event_tx.send(SipEvent::IncomingCall { call_id, from: from_uri });
    }

    /// Build our SDP answer (codec/SRTP/ICE/TURN resolution, all internal
    /// now -- see `media_setup`), send the 200 OK, and emit
    /// `SipEvent::CallConnected` once media is ready. Declines with 486 (via
    /// `reject_call`) and emits `SipEvent::CallFailed` instead if no
    /// mutually-acceptable codec is found or a local RTP port can't be
    /// allocated -- either way the caller (`ui`) only ever finds out via
    /// events, since this command is fire-and-forget.
    pub(crate) async fn accept_call(&mut self, call_id: &str) {
        let Some(dialog) = self.dialogs.get(call_id) else { return };
        let remote_sdp = dialog.remote_sdp.clone().unwrap_or_default();

        let codecs = media_setup::account_codecs(&self.account);
        let Some(parsed) = parse_sdp(&remote_sdp, &codecs) else {
            self.reject_call(call_id).await;
            let _ = self.event_tx.send(SipEvent::CallFailed {
                call_id: call_id.to_string(), code: 488, reason: "No compatible codec".into(),
            });
            return;
        };

        let local_rtp = match deelip_nat::alloc_rtp_port() {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to allocate local RTP port: {e}");
                self.reject_call(call_id).await;
                let _ = self.event_tx.send(SipEvent::CallFailed {
                    call_id: call_id.to_string(), code: 0, reason: "Local RTP port allocation failed".into(),
                });
                return;
            }
        };

        let mut relay = None;
        let ice_result = media_setup::try_answer_with_ice(&self.network, &parsed).await;
        let (rtp_ip, rtp_port, ice_attrs, ice_conn) = match ice_result {
            Some((attrs, addr, conn)) => (addr.ip().to_string(), addr.port(), Some(attrs), Some(conn)),
            None => {
                let (ip, port) = media_setup::resolve_rtp_endpoint(&self.network, &self.advertised_ip, local_rtp, &mut relay).await;
                (ip, port, None, None)
            }
        };

        let account_secure = self.account.transport == deelip_config::TransportProtocol::Tls;
        let local_srtp = if account_secure { Some(SrtpParams::generate()) } else { None };
        let local_sdp = build_answer(&rtp_ip, rtp_port, parsed.codec, local_srtp.as_ref(), ice_attrs.as_ref());

        let contact_transport = self.contact_transport_param();
        let Some(dialog) = self.dialogs.get_mut(call_id) else { return };

        let contact: SocketAddr = dialog.remote_contact
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(self.server_addr);

        let cseq_n       = dialog.remote_cseq.unwrap_or(1);
        let call_id_str  = dialog.call_id.clone();
        let local_tag    = dialog.local_tag.clone();
        let remote_tag   = dialog.remote_tag.clone();
        let remote_uri   = dialog.remote_uri.clone();
        let remote_via   = dialog.remote_via.clone();
        let adv_ip       = self.advertised_ip.clone();
        let local_port   = self.local_port;
        let username     = self.account.username.clone();
        let server       = self.account.server.clone();
        let display      = self.account.display_name.clone()
            .unwrap_or_else(|| username.clone());
        let body_len     = local_sdp.len();

        let from_tag_part = remote_tag.as_deref()
            .map(|t| format!(";tag={t}"))
            .unwrap_or_default();

        let ok_msg = format!(
            "SIP/2.0 200 OK\r\n\
             Via: {remote_via}\r\n\
             To: \"{display}\" <sip:{username}@{server}>;tag={local_tag}\r\n\
             From: <{remote_uri}>{from_tag_part}\r\n\
             Call-ID: {call_id_str}\r\n\
             CSeq: {cseq_n} INVITE\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
             Content-Type: application/sdp\r\n\
             User-Agent: DeeLip/0.1.0\r\n\
             Content-Length: {body_len}\r\n\r\n\
             {local_sdp}"
        );

        let _ = self.transport.send(ok_msg.as_bytes(), contact).await;

        let srtp_session = match (&local_srtp, &parsed.srtp) {
            (Some(local), Some(remote)) => Some(SrtpSession { local: local.clone(), remote: remote.clone() }),
            _ => {
                if account_secure {
                    tracing::warn!("TLS signaling active but remote SDP has no a=crypto -- falling back to plaintext RTP");
                }
                None
            }
        };
        let relay_conn: Option<Arc<dyn Conn + Send + Sync>> = ice_conn.as_ref().map(|c| c.conn.clone())
            .or_else(|| relay.as_ref().map(|r| r.conn.clone()));

        let dialog = self.dialogs.get_mut(call_id).expect("dialog present -- checked above, nothing removes it in between");
        dialog.state     = DialogState::Confirmed;
        dialog.local_sdp = Some(local_sdp);
        dialog.media = Some(CallMedia {
            local_rtp, local_srtp, relay, ice: ice_conn, codec: parsed.codec, dtmf_type: parsed.dtmf_type,
        });

        let _ = self.event_tx.send(SipEvent::CallConnected {
            call_id: call_id.to_string(),
            media: CallMediaReady {
                codec: parsed.codec, dtmf_type: parsed.dtmf_type, local_rtp,
                remote_rtp: parsed.rtp_addr, srtp: srtp_session, relay: relay_conn,
            },
        });
    }

    pub(crate) async fn reject_call(&mut self, call_id: &str) {
        if let Some(dialog) = self.dialogs.remove(call_id) {
            let contact: SocketAddr = dialog.remote_contact
                .as_deref()
                .and_then(|s| s.parse().ok())
                .unwrap_or(self.server_addr);
            let username   = &self.account.username;
            let server     = &self.account.server;
            let display    = self.account.display_name.as_deref().unwrap_or(username);
            let local_tag  = &dialog.local_tag;
            let remote_uri = &dialog.remote_uri;
            let remote_via = &dialog.remote_via;
            let from_tag   = dialog.remote_tag.as_deref()
                .map(|t| format!(";tag={t}")).unwrap_or_default();
            let cseq_n = dialog.remote_cseq.unwrap_or(1);

            let decline = format!(
                "SIP/2.0 486 Busy Here\r\n\
                 Via: {remote_via}\r\n\
                 To: \"{display}\" <sip:{username}@{server}>;tag={local_tag}\r\n\
                 From: <{remote_uri}>{from_tag}\r\n\
                 Call-ID: {call_id}\r\n\
                 CSeq: {cseq_n} INVITE\r\n\
                 Content-Length: 0\r\n\r\n"
            );
            debug!("→ 486 Busy Here {call_id} to {contact}");
            if let Err(e) = self.transport.send(decline.as_bytes(), contact).await {
                error!("Failed to send 486 for {call_id}: {e}");
            }
        }
    }

    pub(crate) async fn hang_up(&mut self, call_id: &str) {
        let dialog = match self.dialogs.get_mut(call_id) {
            Some(d) => d,
            None    => return,
        };

        dialog.state   = DialogState::Terminating;
        let cseq       = dialog.next_local_cseq();
        let branch     = new_branch();
        let server     = self.account.server.clone();
        let username   = self.account.username.clone();
        let display    = self.account.display_name.clone().unwrap_or_else(|| username.clone());
        let local_ip   = self.local_ip.clone();
        let adv_ip     = self.advertised_ip.clone();
        let local_port = self.local_port;
        let call_id_s  = dialog.call_id.clone();
        let from_tag   = dialog.local_tag.clone();
        let to_uri     = dialog.remote_uri.clone();
        let to_tag     = dialog.remote_tag.as_deref()
            .map(|t| format!(";tag={t}")).unwrap_or_default();
        let contact: SocketAddr = dialog.remote_contact
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(self.server_addr);
        let via_proto = self.via_proto();
        let contact_transport = self.contact_transport_param();

        let bye = format!(
            "BYE {to_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id_s}\r\n\
             CSeq: {cseq} BYE\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
             User-Agent: DeeLip/0.1.0\r\n\
             Content-Length: 0\r\n\r\n"
        );
        let _ = self.transport.send(bye.as_bytes(), contact).await;
    }

    // ── Incoming BYE / ACK / CANCEL ──────────────────────────────────────────

    pub(crate) async fn on_bye(&mut self, msg: SipMessage, from: SocketAddr) {
        let call_id = match msg.call_id() {
            Some(id) => id.to_string(),
            None     => return,
        };
        debug!("← BYE {call_id}");
        let ok = self.build_response(&msg, 200, "OK", "", "");
        let _ = self.transport.send(ok.as_bytes(), from).await;
        if let Some(mut dialog) = self.dialogs.remove(&call_id) {
            dialog.state = DialogState::Terminated;
        }
        let _ = self.event_tx.send(SipEvent::CallEnded { call_id });
    }

    pub(crate) fn on_ack(&mut self, msg: SipMessage) {
        if let Some(id) = msg.call_id().map(str::to_string) {
            if let Some(d) = self.dialogs.get_mut(&id) {
                if d.state == DialogState::Calling {
                    d.state = DialogState::Confirmed;
                }
            }
        }
    }

    pub(crate) async fn on_cancel(&mut self, msg: SipMessage, from: SocketAddr) {
        let ok = self.build_response(&msg, 200, "OK", "", "");
        let _ = self.transport.send(ok.as_bytes(), from).await;
        if let Some(call_id) = msg.call_id() {
            let call_id = call_id.to_string();
            debug!("← CANCEL {call_id}");
            self.dialogs.remove(&call_id);
            let _ = self.event_tx.send(SipEvent::CallEnded { call_id });
        }
    }
}
