use std::net::SocketAddr;

use tokio::time::Instant;
use tracing::{debug, error};

use crate::{
    client::{SipStack, MWI_ACCEPT, MWI_EVENT, PRESENCE_ACCEPT, PRESENCE_EVENT, SUBSCRIBE_EXPIRES},
    events::SipEvent,
    subscription::mwi::{parse_mwi_summary, MwiSubscription},
    subscription::presence::{parse_pidf_basic, parse_subscription_state, PresenceSubscription},
    wire::auth::build_challenge_response,
    wire::message::SipMessage,
    wire::util::{extract_expires, new_call_id, new_tag, parse_tag},
};

impl SipStack {
    // ── Presence (SUBSCRIBE/NOTIFY, Event: presence) ─────────────────────────

    pub(crate) async fn subscribe_presence(&mut self, target_uri: &str) {
        let call_id  = new_call_id(&self.local_ip);
        let from_tag = new_tag();
        let sub = PresenceSubscription::new(call_id.clone(), from_tag.clone(), target_uri.to_string());

        let msg = self.build_subscribe(&call_id, &from_tag, 1, target_uri, SUBSCRIBE_EXPIRES, None, PRESENCE_EVENT, PRESENCE_ACCEPT);
        debug!("→ SUBSCRIBE {target_uri} (Event: presence)");
        if let Err(e) = self.transport.send(msg.as_bytes(), self.server_addr).await {
            error!("Failed to send SUBSCRIBE: {e}");
            let _ = self.event_tx.send(SipEvent::PresenceSubscribeFailed {
                uri: target_uri.to_string(), reason: e.to_string(),
            });
            return;
        }
        self.subscriptions.insert(call_id, sub);
    }

    /// Sends SUBSCRIBE with `Expires: 0` per RFC 3265's unsubscribe mechanism,
    /// then removes the subscription locally without waiting for its response.
    pub(crate) async fn unsubscribe_presence(&mut self, target_uri: &str) {
        let matching: Vec<String> = self.subscriptions.iter()
            .filter(|(_, s)| s.target_uri == target_uri)
            .map(|(id, _)| id.clone())
            .collect();

        for call_id in matching {
            if let Some(sub) = self.subscriptions.get_mut(&call_id) {
                let cseq     = sub.next_local_cseq();
                let from_tag = sub.local_tag.clone();
                let msg = self.build_subscribe(&call_id, &from_tag, cseq, target_uri, 0, None, PRESENCE_EVENT, PRESENCE_ACCEPT);
                debug!("→ SUBSCRIBE {target_uri} (Expires: 0, unsubscribe)");
                let _ = self.transport.send(msg.as_bytes(), self.server_addr).await;
            }
            self.subscriptions.remove(&call_id);
        }
    }

    /// Re-SUBSCRIBE any subscription whose `refresh_after` has passed —
    /// called from a coarse 30s tick in `run()` rather than a precise
    /// per-subscription deadline, which is plenty for hour-scale expiries.
    pub(crate) async fn refresh_presence_subscriptions(&mut self) {
        let now = Instant::now();
        let due: Vec<String> = self.subscriptions.iter()
            .filter(|(_, s)| s.refresh_after <= now)
            .map(|(id, _)| id.clone())
            .collect();

        for call_id in due {
            let Some(sub) = self.subscriptions.get_mut(&call_id) else { continue };
            // A refresh is a fresh transaction -- allow a new auth challenge/retry cycle.
            sub.auth_retried = false;
            let cseq       = sub.next_local_cseq();
            let from_tag   = sub.local_tag.clone();
            let target_uri = sub.target_uri.clone();
            let msg = self.build_subscribe(&call_id, &from_tag, cseq, &target_uri, SUBSCRIBE_EXPIRES, None, PRESENCE_EVENT, PRESENCE_ACCEPT);
            debug!("→ SUBSCRIBE {target_uri} (refresh)");
            let _ = self.transport.send(msg.as_bytes(), self.server_addr).await;
        }
    }

    /// `event_package`/`accept` parameterize this over the presence
    /// (`presence`/`application/pidf+xml`) and MWI
    /// (`message-summary`/`application/simple-message-summary`) use sites --
    /// everything else about the SUBSCRIBE (dialog identity, auth retry,
    /// refresh) is identical regardless of which event package it's for.
    #[allow(clippy::too_many_arguments)] // each param is a distinct, meaningfully-named
                                          // piece of a SUBSCRIBE's identity; bundling them
                                          // into a struct wouldn't reduce real complexity here.
    fn build_subscribe(
        &self,
        call_id:       &str,
        from_tag:      &str,
        cseq:          u32,
        target_uri:    &str,
        expires:       u32,
        auth:          Option<&str>,
        event_package: &str,
        accept:        &str,
    ) -> String {
        let branch     = crate::wire::util::new_branch();
        let server     = &self.account.server;
        let username   = &self.account.username;
        let adv_ip     = &self.advertised_ip;
        let local_ip   = &self.local_ip;
        let local_port = self.local_port;
        let display    = self.account.display_name.as_deref().unwrap_or(username);
        let via_proto  = self.via_proto();
        let contact_transport = self.contact_transport_param();

        let mut msg = format!(
            "SUBSCRIBE {target_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{target_uri}>\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} SUBSCRIBE\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
             Event: {event_package}\r\n\
             Accept: {accept}\r\n\
             Expires: {expires}\r\n\
             User-Agent: DeeLip/0.1.0\r\n"
        );
        if let Some(a) = auth { msg.push_str(a); msg.push_str("\r\n"); }
        msg.push_str("Content-Length: 0\r\n\r\n");
        msg
    }

    pub(crate) async fn on_presence_subscribe_response(&mut self, msg: SipMessage, status: u16, call_id: String) {
        match status {
            200 => {
                let expires = extract_expires(&msg).unwrap_or(SUBSCRIBE_EXPIRES);
                let uri = if let Some(sub) = self.subscriptions.get_mut(&call_id) {
                    if sub.remote_tag.is_none() {
                        sub.remote_tag = parse_tag(msg.header("To").unwrap_or(""));
                    }
                    sub.refresh_after = Instant::now() + tokio::time::Duration::from_secs((expires as u64 * 9) / 10);
                    sub.auth_retried  = false;
                    sub.target_uri.clone()
                } else {
                    return;
                };
                let _ = self.event_tx.send(SipEvent::PresenceSubscribed { uri, expires });
            }
            401 | 407 => {
                let Some(sub) = self.subscriptions.get(&call_id) else { return };
                if sub.auth_retried {
                    let uri = self.subscriptions.remove(&call_id).map(|s| s.target_uri).unwrap_or_default();
                    let _ = self.event_tx.send(SipEvent::PresenceSubscribeFailed {
                        uri, reason: format!("{status}"),
                    });
                    return;
                }
                let target_uri = sub.target_uri.clone();
                let hdr_name = if status == 407 { "Proxy-Authenticate" } else { "WWW-Authenticate" };
                let Some(challenge_raw) = msg.header(hdr_name) else {
                    let uri = self.subscriptions.remove(&call_id).map(|s| s.target_uri).unwrap_or_default();
                    let _ = self.event_tx.send(SipEvent::PresenceSubscribeFailed {
                        uri, reason: "Missing auth challenge".into(),
                    });
                    return;
                };
                let Some(auth) = build_challenge_response(
                    &self.account.username, &self.account.password, "SUBSCRIBE", &target_uri, challenge_raw,
                ) else {
                    let uri = self.subscriptions.remove(&call_id).map(|s| s.target_uri).unwrap_or_default();
                    let _ = self.event_tx.send(SipEvent::PresenceSubscribeFailed {
                        uri, reason: "Bad auth challenge".into(),
                    });
                    return;
                };
                let Some(sub) = self.subscriptions.get_mut(&call_id) else { return };
                sub.auth_retried = true;
                let cseq     = sub.next_local_cseq();
                let from_tag = sub.local_tag.clone();
                let retry = self.build_subscribe(&call_id, &from_tag, cseq, &target_uri, SUBSCRIBE_EXPIRES, Some(&auth), PRESENCE_EVENT, PRESENCE_ACCEPT);
                let _ = self.transport.send(retry.as_bytes(), self.server_addr).await;
            }
            c if c >= 300 => {
                let uri = self.subscriptions.remove(&call_id).map(|s| s.target_uri).unwrap_or_default();
                let _ = self.event_tx.send(SipEvent::PresenceSubscribeFailed { uri, reason: format!("{c}") });
            }
            _ => {}
        }
    }

    // ── MWI (SUBSCRIBE/NOTIFY, Event: message-summary) ───────────────────────

    pub(crate) async fn subscribe_mwi(&mut self, target_uri: &str) {
        let call_id  = new_call_id(&self.local_ip);
        let from_tag = new_tag();
        let sub = MwiSubscription::new(call_id.clone(), from_tag.clone(), target_uri.to_string());

        let msg = self.build_subscribe(&call_id, &from_tag, 1, target_uri, SUBSCRIBE_EXPIRES, None, MWI_EVENT, MWI_ACCEPT);
        debug!("→ SUBSCRIBE {target_uri} (Event: message-summary)");
        if let Err(e) = self.transport.send(msg.as_bytes(), self.server_addr).await {
            error!("Failed to send SUBSCRIBE: {e}");
            let _ = self.event_tx.send(SipEvent::MwiSubscribeFailed {
                uri: target_uri.to_string(), reason: e.to_string(),
            });
            return;
        }
        self.mwi_subscriptions.insert(call_id, sub);
    }

    /// Re-SUBSCRIBE any MWI subscription whose `refresh_after` has passed --
    /// mirrors `refresh_presence_subscriptions` exactly.
    pub(crate) async fn refresh_mwi_subscriptions(&mut self) {
        let now = Instant::now();
        let due: Vec<String> = self.mwi_subscriptions.iter()
            .filter(|(_, s)| s.refresh_after <= now)
            .map(|(id, _)| id.clone())
            .collect();

        for call_id in due {
            let Some(sub) = self.mwi_subscriptions.get_mut(&call_id) else { continue };
            sub.auth_retried = false;
            let cseq       = sub.next_local_cseq();
            let from_tag   = sub.local_tag.clone();
            let target_uri = sub.target_uri.clone();
            let msg = self.build_subscribe(&call_id, &from_tag, cseq, &target_uri, SUBSCRIBE_EXPIRES, None, MWI_EVENT, MWI_ACCEPT);
            debug!("→ SUBSCRIBE {target_uri} (MWI refresh)");
            let _ = self.transport.send(msg.as_bytes(), self.server_addr).await;
        }
    }

    pub(crate) async fn on_mwi_subscribe_response(&mut self, msg: SipMessage, status: u16, call_id: String) {
        match status {
            200 => {
                let expires = extract_expires(&msg).unwrap_or(SUBSCRIBE_EXPIRES);
                let uri = if let Some(sub) = self.mwi_subscriptions.get_mut(&call_id) {
                    if sub.remote_tag.is_none() {
                        sub.remote_tag = parse_tag(msg.header("To").unwrap_or(""));
                    }
                    sub.refresh_after = Instant::now() + tokio::time::Duration::from_secs((expires as u64 * 9) / 10);
                    sub.auth_retried  = false;
                    sub.target_uri.clone()
                } else {
                    return;
                };
                let _ = self.event_tx.send(SipEvent::MwiSubscribed { uri, expires });
            }
            401 | 407 => {
                let Some(sub) = self.mwi_subscriptions.get(&call_id) else { return };
                if sub.auth_retried {
                    let uri = self.mwi_subscriptions.remove(&call_id).map(|s| s.target_uri).unwrap_or_default();
                    let _ = self.event_tx.send(SipEvent::MwiSubscribeFailed {
                        uri, reason: format!("{status}"),
                    });
                    return;
                }
                let target_uri = sub.target_uri.clone();
                let hdr_name = if status == 407 { "Proxy-Authenticate" } else { "WWW-Authenticate" };
                let Some(challenge_raw) = msg.header(hdr_name) else {
                    let uri = self.mwi_subscriptions.remove(&call_id).map(|s| s.target_uri).unwrap_or_default();
                    let _ = self.event_tx.send(SipEvent::MwiSubscribeFailed {
                        uri, reason: "Missing auth challenge".into(),
                    });
                    return;
                };
                let Some(auth) = build_challenge_response(
                    &self.account.username, &self.account.password, "SUBSCRIBE", &target_uri, challenge_raw,
                ) else {
                    let uri = self.mwi_subscriptions.remove(&call_id).map(|s| s.target_uri).unwrap_or_default();
                    let _ = self.event_tx.send(SipEvent::MwiSubscribeFailed {
                        uri, reason: "Bad auth challenge".into(),
                    });
                    return;
                };
                let Some(sub) = self.mwi_subscriptions.get_mut(&call_id) else { return };
                sub.auth_retried = true;
                let cseq     = sub.next_local_cseq();
                let from_tag = sub.local_tag.clone();
                let retry = self.build_subscribe(&call_id, &from_tag, cseq, &target_uri, SUBSCRIBE_EXPIRES, Some(&auth), MWI_EVENT, MWI_ACCEPT);
                let _ = self.transport.send(retry.as_bytes(), self.server_addr).await;
            }
            c if c >= 300 => {
                let uri = self.mwi_subscriptions.remove(&call_id).map(|s| s.target_uri).unwrap_or_default();
                let _ = self.event_tx.send(SipEvent::MwiSubscribeFailed { uri, reason: format!("{c}") });
            }
            _ => {}
        }
    }

    pub(crate) async fn on_notify(&mut self, msg: SipMessage, from: SocketAddr) {
        let call_id = msg.call_id().map(str::to_string);
        let is_presence = call_id.as_deref().is_some_and(|id| self.subscriptions.contains_key(id));
        let is_mwi = call_id.as_deref().is_some_and(|id| self.mwi_subscriptions.contains_key(id));

        if is_presence {
            let call_id = call_id.clone().unwrap();
            let body = String::from_utf8_lossy(&msg.body).into_owned();

            if let Some(state) = parse_pidf_basic(&body) {
                if let Some(sub) = self.subscriptions.get_mut(&call_id) {
                    sub.state = state;
                    if sub.remote_tag.is_none() {
                        // First NOTIFY can race ahead of the SUBSCRIBE's own 200 OK.
                        sub.remote_tag = parse_tag(msg.header("From").unwrap_or(""));
                    }
                    let uri = sub.target_uri.clone();
                    let _ = self.event_tx.send(SipEvent::PresenceUpdate { uri, state });
                }
            }

            if let Some(sub_state) = msg.header("Subscription-State") {
                let (state_token, _) = parse_subscription_state(sub_state);
                if state_token.eq_ignore_ascii_case("terminated") {
                    self.subscriptions.remove(&call_id);
                }
            }
        } else if is_mwi {
            let call_id = call_id.unwrap();
            let body = String::from_utf8_lossy(&msg.body).into_owned();

            if let Some(state) = parse_mwi_summary(&body) {
                if let Some(sub) = self.mwi_subscriptions.get_mut(&call_id) {
                    sub.state = state;
                    if sub.remote_tag.is_none() {
                        sub.remote_tag = parse_tag(msg.header("From").unwrap_or(""));
                    }
                    let uri = sub.target_uri.clone();
                    let _ = self.event_tx.send(SipEvent::MwiUpdate { uri, state });
                }
            }

            if let Some(sub_state) = msg.header("Subscription-State") {
                let (state_token, _) = parse_subscription_state(sub_state);
                if state_token.eq_ignore_ascii_case("terminated") {
                    self.mwi_subscriptions.remove(&call_id);
                }
            }
        }

        // Non-presence/MWI NOTIFY (e.g. blind-transfer's sipfrag) falls
        // through to an unconditional blind-ack, unchanged from before
        // either of these subscription features existed.
        self.send_ok(&msg, from).await;
    }
}
