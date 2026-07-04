use std::net::SocketAddr;

use tracing::{debug, error};

use crate::{
    call::dialog::{Dialog, DialogState},
    client::SipStack,
    events::SipEvent,
    wire::auth::build_challenge_response,
    wire::message::{SipMessage, SipMethod},
    wire::util::{new_branch, new_call_id, new_tag, parse_tag, parse_uri},
};

impl SipStack {
    // ── Outgoing call ─────────────────────────────────────────────────────────

    pub(crate) async fn initiate_call(&mut self, to: &str, local_sdp: &str) {
        let call_id  = new_call_id(&self.local_ip);
        let from_tag = new_tag();
        let mut dialog = Dialog::new_outgoing(call_id.clone(), from_tag.clone(), to.to_string());
        dialog.local_sdp = Some(local_sdp.to_string());

        let msg = self.build_invite(&dialog.call_id, &dialog.local_tag, dialog.local_cseq, to, local_sdp, None);
        debug!("→ INVITE {to}");
        if let Err(e) = self.transport.send(msg.as_bytes(), self.server_addr).await {
            error!("Failed to send INVITE: {e}");
            return;
        }
        self.dialogs.insert(call_id, dialog);
    }

    fn build_invite(
        &self,
        call_id:  &str,
        from_tag: &str,
        cseq:     u32,
        to:       &str,
        sdp:      &str,
        auth:     Option<&str>,
    ) -> String {
        let branch     = new_branch();
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

    pub(crate) async fn send_reinvite(&mut self, call_id: &str, local_sdp: &str, hold: bool) {
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
            Act::Connected { call_id, remote_sdp, ack_cid, ack_from_tag, ack_to_uri, ack_to_tag, ack_cseq } => {
                let ack = self.build_ack(&ack_cid, &ack_from_tag, &ack_to_uri, ack_to_tag.as_deref(), ack_cseq);
                let _ = self.transport.send(ack.as_bytes(), self.server_addr).await;
                let _ = self.event_tx.send(SipEvent::CallConnected { call_id, remote_sdp });
            }
            Act::ReInviteAck { call_id, hold, ack_cid, ack_from_tag, ack_to_uri, ack_to_tag, ack_cseq } => {
                let ack = self.build_ack(&ack_cid, &ack_from_tag, &ack_to_uri, ack_to_tag.as_deref(), ack_cseq);
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
                let ack = self.build_ack(&ack_cid, &ack_from_tag, &ack_to_uri, ack_to_tag.as_deref(), ack_cseq);
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
                let cseq = dialog.next_local_cseq();
                let dialog_call_id  = dialog.call_id.clone();
                let dialog_from_tag = dialog.local_tag.clone();
                let msg = self.build_invite(&dialog_call_id, &dialog_from_tag, cseq, &to_uri, &local_sdp, Some(&auth));
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

        let _ = self.event_tx.send(SipEvent::IncomingCall { call_id, from: from_uri, remote_sdp });
    }

    pub(crate) async fn accept_call(&mut self, call_id: &str, local_sdp: &str) {
        let contact_transport = self.contact_transport_param();
        let dialog = match self.dialogs.get_mut(call_id) {
            Some(d) => d,
            None    => return,
        };

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
        dialog.state     = DialogState::Confirmed;
        dialog.local_sdp = Some(local_sdp.to_string());
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
