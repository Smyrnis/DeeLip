use anyhow::Context;
use std::sync::atomic::Ordering;
use tracing::{debug, info, warn};

use crate::{
    client::{SipStack, REG_RECV_TIMEOUT},
    wire::auth::build_challenge_response,
    wire::message::SipMessage,
    wire::util::{extract_expires, new_branch, parse_via_received},
};

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
        if !self.account.allow_ip_rewrite || self.account.public_address.is_some() {
            return;
        }
        let Some(via) = resp.header("Via") else {
            return;
        };
        let (received, _rport) = parse_via_received(via);
        let Some(ip) = received else {
            return;
        };
        if ip != self.advertised_ip {
            info!("Allow IP Rewrite: advertised address {} -> {ip}", self.advertised_ip);
            self.advertised_ip = ip;
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
        let expires = self.account.register_expires;
        let user_agent = crate::USER_AGENT;

        let mut msg = format!(
            "REGISTER sip:{server} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: \"{display}\" <sip:{username}@{server}>\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} REGISTER\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
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
