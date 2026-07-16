use anyhow::Context;
use std::sync::atomic::Ordering;
use tracing::{debug, info, warn};

use deelip_config::timeouts::REG_RECV_TIMEOUT;

use crate::{
    client::SipStack,
    wire::auth::build_challenge_response,
    wire::message::SipMessage,
    wire::util::{extract_expires, new_branch, parse_via_received},
};

/// Marks a REGISTER rejection that retrying will never fix on its own --
/// wrong credentials (403) or an unknown user/domain (404). Downcast out of
/// the `anyhow::Error` by `client::run_loop::run`'s reconnect loop to stop
/// silently retrying forever on an error that can never succeed, instead of
/// treating it the same as a transient network blip. Every other status
/// code (including 5xx, which genuinely can be transient) keeps today's
/// plain-string-error, keep-retrying behavior.
#[derive(Debug)]
pub(crate) struct PermanentRegError(pub(crate) u16);

impl std::fmt::Display for PermanentRegError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "REGISTER rejected: {} (permanent, won't retry)", self.0)
    }
}
impl std::error::Error for PermanentRegError {}

impl SipStack {
    pub(crate) async fn register_once(&mut self) -> anyhow::Result<u32> {
        self.send_register(None).await?;
        let resp = self.recv_reg_response().await?;

        match resp.status_code() {
            Some(200) => {
                info!("Registered (no auth)");
                self.maybe_rewrite_advertised_ip(&resp);
                return Ok(extract_expires(&resp).unwrap_or(self.account.register_expires));
            }
            Some(401) | Some(407) => {}
            Some(c @ (403 | 404)) => return Err(PermanentRegError(c).into()),
            Some(c) => return Err(anyhow::anyhow!("REGISTER rejected: {c}")),
            None => return Err(anyhow::anyhow!("Expected response")),
        }

        let hdr_name = if resp.status_code() == Some(407) { "Proxy-Authenticate" } else { "WWW-Authenticate" };
        let www_auth = resp.header(hdr_name).ok_or_else(|| anyhow::anyhow!("Missing {hdr_name}"))?.to_owned();

        let uri = format!("sip:{}", self.account.domain());
        let auth =
            build_challenge_response(self.account.auth_username(), &self.account.password, "REGISTER", &uri, &www_auth)
                .ok_or_else(|| anyhow::anyhow!("Bad challenge: {www_auth}"))?;

        self.send_register(Some(&auth)).await?;
        let resp2 = self.recv_reg_response().await?;
        match resp2.status_code() {
            Some(200) => {
                info!("Registered");
                self.maybe_rewrite_advertised_ip(&resp2);
                Ok(extract_expires(&resp2).unwrap_or(self.account.register_expires))
            }
            Some(c @ (403 | 404)) => Err(PermanentRegError(c).into()),
            Some(c) => Err(anyhow::anyhow!("REGISTER rejected: {c}")),
            None => Err(anyhow::anyhow!("Expected response")),
        }
    }

    /// `SipAccount::allow_ip_rewrite`: adopt the `received=` address the
    /// registrar's response reports on our own `Via:` header as the
    /// advertised Contact/SDP IP going forward -- a self-discovery
    /// alternative to a separate STUN server, re-checked on every
    /// (re-)registration so it also self-corrects if a NAT rebinding
    /// changes our observed address mid-session. A no-op if
    /// `public_address` is set (an explicit override always wins) or the
    /// response carries no `received=` param (e.g. no NAT in the path).
    fn maybe_rewrite_advertised_ip(&mut self, resp: &SipMessage) {
        if let Some(new_ip) = resolve_ip_rewrite(
            &self.advertised_ip,
            self.account.allow_ip_rewrite,
            self.account.public_address.is_some(),
            resp.header("Via"),
        ) {
            info!("Allow IP Rewrite: advertised address {} -> {new_ip}", self.advertised_ip);
            self.advertised_ip = new_ip;
        }
    }

    async fn send_register(&self, auth: Option<&str>) -> anyhow::Result<()> {
        let cseq = self.reg_cseq.fetch_add(1, Ordering::SeqCst);
        let branch = new_branch();
        let server = self.account.domain();
        let username = &self.account.username;
        let adv_ip = &self.advertised_ip;
        let local_ip = &self.local_ip;
        let local_port = self.local_port;
        let call_id = &self.reg_call_id;
        let from_tag = &self.reg_from_tag;
        let display = self.account.display_name.as_deref().unwrap_or(username);
        let via_proto = self.via_proto();
        let contact_transport = self.contact_transport_param();
        let via_line = crate::client::build_via(via_proto, local_ip, local_port, &branch);
        let contact_line = crate::client::build_contact(username, adv_ip, local_port, contact_transport);
        let expires = self.account.register_expires;
        let user_agent = crate::USER_AGENT;

        let mut msg = format!(
            "REGISTER sip:{server} SIP/2.0\r\n\
             {via_line}\
             Max-Forwards: 70\r\n\
             To: \"{display}\" <sip:{username}@{server}>\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} REGISTER\r\n\
             {contact_line}\
             Expires: {expires}\r\n\
             User-Agent: {user_agent}\r\n"
        );
        if let Some(a) = auth {
            msg.push_str(a);
            msg.push_str("\r\n");
        }
        msg.push_str("Content-Length: 0\r\n\r\n");

        debug!("→ REGISTER");
        self.transport.send(msg.as_bytes(), self.server_addr).await.context("Sending REGISTER")
    }

    async fn recv_reg_response(&self) -> anyhow::Result<SipMessage> {
        use tokio::time::timeout;
        loop {
            let (data, _from) = timeout(REG_RECV_TIMEOUT, self.transport.recv())
                .await
                .context("REGISTER response timeout")?
                .context("Transport error")?;

            let msg = match SipMessage::parse(&data) {
                Some(m) => m,
                None => {
                    warn!("Unparsable datagram during REGISTER");
                    continue;
                }
            };
            if matches!(msg.status_code(), Some(c) if c < 200) {
                continue;
            }
            if msg.method().is_some() {
                debug!("Ignoring request during REGISTER");
                continue;
            }
            if msg.call_id().is_some_and(|id| id != self.reg_call_id) {
                debug!("Ignoring response for different Call-ID");
                continue;
            }
            return Ok(msg);
        }
    }
}

/// Pure decision for `SipAccount::allow_ip_rewrite`'s NAT self-discovery:
/// given the currently advertised IP, the account's rewrite/override
/// settings, and a response's raw `Via:` header (if any), decide the new
/// advertised IP, if it should change. Split out of `maybe_rewrite_advertised_ip`
/// so this is directly testable without a live registrar round-trip/`SipStack`.
fn resolve_ip_rewrite(
    current_advertised_ip: &str, allow_ip_rewrite: bool, public_address_is_set: bool, via: Option<&str>,
) -> Option<String> {
    if !allow_ip_rewrite || public_address_is_set {
        return None;
    }
    let (received, _rport) = parse_via_received(via?);
    let ip = received?;
    if ip != current_advertised_ip { Some(ip) } else { None }
}

#[cfg(test)]
#[path = "../tests/unit/registration.rs"]
mod tests;
