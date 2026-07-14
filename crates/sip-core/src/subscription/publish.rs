//! Outgoing presence PUBLISH (RFC 3903) -- the mirror image of
//! `presence.rs`'s SUBSCRIBE/NOTIFY: instead of watching *someone else's*
//! status, this publishes *our own* to the server, gated behind
//! `SipAccount::publish_presence`. A standalone request/response
//! transaction refreshed on its own timer, same shape as
//! `PresenceSubscription`/`MwiSubscription`, but with an `etag` instead of
//! a remote dialog tag (RFC 3903's `SIP-ETag`/`SIP-If-Match` identify which
//! published event state a request refers to).

use tokio::time::{Duration, Instant};
use tracing::{debug, error};

use crate::{
    client::{PRESENCE_EVENT, SipStack},
    wire::auth::build_challenge_response,
    wire::message::SipMessage,
    wire::util::{extract_expires, new_branch, new_call_id, new_tag},
};

pub(crate) const PUBLISH_EXPIRES: u32 = 3600;

pub struct PresencePublish {
    call_id: String,
    local_tag: String,
    local_cseq: u32,
    /// `SIP-ETag` from the last successful PUBLISH -- `None` until the
    /// first 200 OK, at which point every subsequent PUBLISH (refresh or
    /// state change) must carry it back as `SIP-If-Match`.
    etag: Option<String>,
    auth_retried: bool,
    refresh_after: Instant,
    available: bool,
}

fn own_pidf(entity: &str, available: bool) -> String {
    let basic = if available { "open" } else { "closed" };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n\
         <presence xmlns=\"urn:ietf:params:xml:ns:pidf\" entity=\"{entity}\">\r\n\
         <tuple id=\"deelip\">\r\n\
         <status><basic>{basic}</basic></status>\r\n\
         </tuple>\r\n\
         </presence>"
    )
}

impl SipStack {
    pub(crate) async fn publish_own_presence(&mut self, available: bool) {
        let entity = format!("sip:{}@{}", self.account.username, self.account.domain());
        let (call_id, from_tag, cseq, etag) = match &self.presence_publish {
            Some(p) => (p.call_id.clone(), p.local_tag.clone(), p.local_cseq + 1, p.etag.clone()),
            None => (new_call_id(&self.local_ip), new_tag(), 1, None),
        };
        let body = own_pidf(&entity, available);
        let msg = self.build_publish(&entity, &call_id, &from_tag, cseq, etag.as_deref(), None, &body);
        debug!("→ PUBLISH {entity} (presence, {})", if available { "open" } else { "closed" });
        if let Err(e) = self.transport.send(msg.as_bytes(), self.server_addr).await {
            error!("Failed to send PUBLISH: {e}");
            return;
        }
        self.presence_publish = Some(PresencePublish {
            call_id,
            local_tag: from_tag,
            local_cseq: cseq,
            etag,
            auth_retried: false,
            // Placeholder until the 200 OK's real `Expires` is known -- same
            // idiom as `PresenceSubscription::new`'s doc comment: a periodic
            // scan only acts on entries already past due, so a fresh
            // in-flight publish just isn't touched again until it resolves.
            refresh_after: Instant::now() + Duration::from_secs((PUBLISH_EXPIRES as u64 * 9) / 10),
            available,
        });
    }

    /// Re-PUBLISH if the last one's `refresh_after` has passed -- called
    /// from the same 30s tick as `refresh_presence_subscriptions`/
    /// `refresh_mwi_subscriptions`.
    pub(crate) async fn refresh_presence_publish(&mut self) {
        let Some(pub_state) = &self.presence_publish else {
            return;
        };
        if pub_state.refresh_after > Instant::now() {
            return;
        }
        self.publish_own_presence(pub_state.available).await;
    }

    #[allow(clippy::too_many_arguments)] // matches `build_subscribe`'s reasoning in
    // `handlers.rs` -- each param is a distinct,
    // meaningfully-named piece of the request.
    fn build_publish(
        &self, entity: &str, call_id: &str, from_tag: &str, cseq: u32, etag: Option<&str>, auth: Option<&str>,
        body: &str,
    ) -> String {
        let branch = new_branch();
        let server = self.account.domain();
        let username = &self.account.username;
        let local_ip = &self.local_ip;
        let local_port = self.local_port;
        let display = self.account.display_name.as_deref().unwrap_or(username);
        let via_proto = self.via_proto();
        let via_line = crate::client::build_via(via_proto, local_ip, local_port, &branch);
        let body_len = body.len();
        let user_agent = crate::USER_AGENT;

        let mut msg = format!(
            "PUBLISH {entity} SIP/2.0\r\n\
             {via_line}\
             Max-Forwards: 70\r\n\
             To: <{entity}>\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} PUBLISH\r\n\
             Event: {PRESENCE_EVENT}\r\n\
             Expires: {PUBLISH_EXPIRES}\r\n"
        );
        if let Some(t) = etag {
            msg.push_str(&format!("SIP-If-Match: {t}\r\n"));
        }
        if let Some(a) = auth {
            msg.push_str(a);
            msg.push_str("\r\n");
        }
        msg.push_str(&format!(
            "Content-Type: application/pidf+xml\r\nUser-Agent: {user_agent}\r\nContent-Length: {body_len}\r\n\r\n{body}"
        ));
        msg
    }

    pub(crate) async fn on_publish_response(&mut self, msg: SipMessage, status: u16, call_id: String) {
        let is_ours = self.presence_publish.as_ref().is_some_and(|p| p.call_id == call_id);
        if !is_ours {
            return;
        }

        match status {
            200..=299 => {
                let expires = extract_expires(&msg).unwrap_or(PUBLISH_EXPIRES);
                let etag = msg.header("SIP-ETag").map(str::to_string);
                if let Some(p) = &mut self.presence_publish {
                    p.etag = etag;
                    p.auth_retried = false;
                    p.refresh_after = Instant::now() + Duration::from_secs((expires as u64 * 9) / 10);
                }
            }
            401 | 407 => {
                let Some((from_tag, cseq, etag, available, auth_retried)) = self
                    .presence_publish
                    .as_ref()
                    .map(|p| (p.local_tag.clone(), p.local_cseq, p.etag.clone(), p.available, p.auth_retried))
                else {
                    return;
                };
                if auth_retried {
                    // Give up -- the next explicit trigger (DND toggle,
                    // re-registration) starts a fresh publish from scratch.
                    self.presence_publish = None;
                    return;
                }
                let entity = format!("sip:{}@{}", self.account.username, self.account.domain());
                let hdr_name = if status == 407 { "Proxy-Authenticate" } else { "WWW-Authenticate" };
                let Some(challenge_raw) = msg.header(hdr_name) else {
                    self.presence_publish = None;
                    return;
                };
                let Some(auth) = build_challenge_response(
                    self.account.auth_username(),
                    &self.account.password,
                    "PUBLISH",
                    &entity,
                    challenge_raw,
                ) else {
                    self.presence_publish = None;
                    return;
                };
                let new_cseq = cseq + 1;
                let body = own_pidf(&entity, available);
                let retry =
                    self.build_publish(&entity, &call_id, &from_tag, new_cseq, etag.as_deref(), Some(&auth), &body);
                debug!("→ PUBLISH {entity} (authenticated)");
                let _ = self.transport.send(retry.as_bytes(), self.server_addr).await;
                if let Some(p) = &mut self.presence_publish {
                    p.auth_retried = true;
                    p.local_cseq = new_cseq;
                }
            }
            _ => {
                self.presence_publish = None;
            }
        }
    }
}
