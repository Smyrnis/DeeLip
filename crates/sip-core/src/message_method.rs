//! Standalone SIP instant messaging (RFC 3428, `MESSAGE` method) -- neither
//! a call dialog nor a SUBSCRIBE-style subscription, just a single
//! request/response transaction, so it gets its own small home rather than
//! living in `call/` or `subscription/`.

use std::net::SocketAddr;

use tracing::{debug, error};

use crate::{
    client::SipStack,
    events::SipEvent,
    wire::auth::build_challenge_response,
    wire::message::SipMessage,
    wire::util::{new_branch, new_call_id, new_tag, parse_uri},
};

/// An outstanding MESSAGE request awaiting its response, enough to resend
/// once with digest credentials on a 401/407 challenge (mirrors
/// `Dialog::auth_retried` for INVITE, `PresenceSubscription::auth_retried`
/// for SUBSCRIBE).
pub struct PendingMessage {
    to: String,
    body: String,
    from_tag: String,
    auth_retried: bool,
}

impl SipStack {
    // ── Outgoing MESSAGE ──────────────────────────────────────────────────────

    pub(crate) async fn send_message(&mut self, to: &str, body: &str) {
        let call_id = new_call_id(&self.local_ip);
        let from_tag = new_tag();
        let msg = self.build_message(&call_id, &from_tag, 1, to, body, None);
        debug!("→ MESSAGE {to}");
        if let Err(e) = self.transport.send(msg.as_bytes(), self.server_addr).await {
            error!("Failed to send MESSAGE: {e}");
            let _ = self.event_tx.send(SipEvent::MessageSendResult {
                to: to.to_string(),
                ok: false,
                reason: Some(e.to_string()),
            });
            return;
        }
        self.pending_messages.insert(
            call_id,
            PendingMessage {
                to: to.to_string(),
                body: body.to_string(),
                from_tag,
                auth_retried: false,
            },
        );
    }

    fn build_message(
        &self,
        call_id: &str,
        from_tag: &str,
        cseq: u32,
        to: &str,
        body: &str,
        auth: Option<&str>,
    ) -> String {
        let branch = new_branch();
        let server = &self.account.server;
        let username = &self.account.username;
        let adv_ip = &self.advertised_ip;
        let local_ip = &self.local_ip;
        let local_port = self.local_port;
        let display = self.account.display_name.as_deref().unwrap_or(username);
        let body_len = body.len();
        let via_proto = self.via_proto();
        let contact_transport = self.contact_transport_param();

        let mut msg = format!(
            "MESSAGE {to} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{to}>\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} MESSAGE\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
             Content-Type: text/plain\r\n\
             User-Agent: DeeLip/0.1.0\r\n"
        );
        if let Some(a) = auth {
            msg.push_str(a);
            msg.push_str("\r\n");
        }
        msg.push_str(&format!("Content-Length: {body_len}\r\n\r\n{body}"));
        msg
    }

    pub(crate) async fn on_message_response(
        &mut self,
        msg: SipMessage,
        status: u16,
        call_id: String,
    ) {
        debug!("← {status} response to MESSAGE {call_id}");
        match status {
            200..=299 => {
                if let Some(pending) = self.pending_messages.remove(&call_id) {
                    let _ = self.event_tx.send(SipEvent::MessageSendResult {
                        to: pending.to,
                        ok: true,
                        reason: None,
                    });
                }
            }
            401 | 407 => {
                // Copy out everything needed up front, then work with owned
                // locals -- keeping `pending` borrowed across the later
                // `remove`/`get_mut` calls below would fight the borrow checker.
                let Some((to, body, from_tag, auth_retried)) =
                    self.pending_messages.get(&call_id).map(|p| {
                        (
                            p.to.clone(),
                            p.body.clone(),
                            p.from_tag.clone(),
                            p.auth_retried,
                        )
                    })
                else {
                    return;
                };

                if auth_retried {
                    self.pending_messages.remove(&call_id);
                    let _ = self.event_tx.send(SipEvent::MessageSendResult {
                        to,
                        ok: false,
                        reason: Some(format!("{status}")),
                    });
                    return;
                }
                let hdr_name = if status == 407 {
                    "Proxy-Authenticate"
                } else {
                    "WWW-Authenticate"
                };
                let Some(challenge_raw) = msg.header(hdr_name) else {
                    self.pending_messages.remove(&call_id);
                    let _ = self.event_tx.send(SipEvent::MessageSendResult {
                        to,
                        ok: false,
                        reason: Some("Missing auth challenge".into()),
                    });
                    return;
                };
                let Some(auth) = build_challenge_response(
                    &self.account.username,
                    &self.account.password,
                    "MESSAGE",
                    &to,
                    challenge_raw,
                ) else {
                    self.pending_messages.remove(&call_id);
                    let _ = self.event_tx.send(SipEvent::MessageSendResult {
                        to,
                        ok: false,
                        reason: Some("Bad auth challenge".into()),
                    });
                    return;
                };
                let retry = self.build_message(&call_id, &from_tag, 2, &to, &body, Some(&auth));
                debug!("→ MESSAGE {to} (authenticated)");
                let _ = self
                    .transport
                    .send(retry.as_bytes(), self.server_addr)
                    .await;
                if let Some(pending) = self.pending_messages.get_mut(&call_id) {
                    pending.auth_retried = true;
                }
            }
            c if c >= 300 => {
                if let Some(pending) = self.pending_messages.remove(&call_id) {
                    let _ = self.event_tx.send(SipEvent::MessageSendResult {
                        to: pending.to,
                        ok: false,
                        reason: Some(format!("{c}")),
                    });
                }
            }
            _ => {}
        }
    }

    // ── Incoming MESSAGE ──────────────────────────────────────────────────────

    pub(crate) async fn on_message(&mut self, msg: SipMessage, from: SocketAddr) {
        let from_hdr = msg.header("From").unwrap_or("").to_string();
        let from_uri = parse_uri(&from_hdr).unwrap_or_else(|| from_hdr.clone());
        let body = String::from_utf8_lossy(&msg.body).into_owned();
        debug!("← MESSAGE from {from_uri} ({from}): {body}");
        self.send_ok(&msg, from).await;
        let _ = self.event_tx.send(SipEvent::MessageReceived {
            from: from_uri,
            body,
        });
    }
}
