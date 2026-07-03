use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::Context;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration, Instant, sleep_until};
use tracing::{debug, error, info, warn};

use deelip_config::{SipAccount, TransportProtocol};

use crate::{
    auth::build_challenge_response,
    dialog::{Dialog, DialogState},
    events::{SipCommand, SipEvent},
    message::{SipMessage, SipMethod, SipStartLine},
    presence::{parse_pidf_basic, parse_subscription_state, PresenceSubscription},
    transport::SipTransport,
    util::{local_ip_for, new_branch, new_call_id, new_tag},
};

const REG_EXPIRES:        u32      = 3600;
const REG_MARGIN:         u32      = 60;
const REG_RECV_TIMEOUT:   Duration = Duration::from_secs(10);
const MAX_RETRY:          Duration = Duration::from_secs(300);
const SUBSCRIBE_EXPIRES:  u32      = 3600;
const PRESENCE_TICK:      Duration = Duration::from_secs(30);

// ── Public handle ─────────────────────────────────────────────────────────────

pub struct SipHandle {
    pub event_rx:      mpsc::UnboundedReceiver<SipEvent>,
    pub cmd_tx:        mpsc::UnboundedSender<SipCommand>,
    /// IP advertised in Contact and SDP (may be external if STUN succeeded).
    pub advertised_ip: String,
    /// True when signaling runs over TLS — callers use this to decide whether to offer SRTP.
    pub secure: bool,
    /// The account's SIP domain (`account.server`) — used to resolve bare
    /// extension numbers typed into the dialer into a full SIP URI.
    pub domain: String,
}

impl SipHandle {
    pub fn make_call(&self, to: &str, local_sdp: String) {
        let _ = self.cmd_tx.send(SipCommand::MakeCall { to: to.to_string(), local_sdp });
    }
    pub fn accept_call(&self, call_id: &str, local_sdp: String) {
        let _ = self.cmd_tx.send(SipCommand::AcceptCall {
            call_id: call_id.to_string(), local_sdp,
        });
    }
    pub fn reject_call(&self, call_id: &str) {
        let _ = self.cmd_tx.send(SipCommand::RejectCall { call_id: call_id.to_string() });
    }
    pub fn hang_up(&self, call_id: &str) {
        let _ = self.cmd_tx.send(SipCommand::HangUp { call_id: call_id.to_string() });
    }
    pub fn hold_call(&self, call_id: &str, local_sdp: String) {
        let _ = self.cmd_tx.send(SipCommand::HoldCall {
            call_id: call_id.to_string(), local_sdp,
        });
    }
    pub fn resume_call(&self, call_id: &str, local_sdp: String) {
        let _ = self.cmd_tx.send(SipCommand::ResumeCall {
            call_id: call_id.to_string(), local_sdp,
        });
    }
    /// `target` must already be a fully-qualified SIP URI (e.g. from
    /// `normalize_target`) — it's placed verbatim into the Refer-To header.
    pub fn blind_transfer(&self, call_id: &str, target: String) {
        let _ = self.cmd_tx.send(SipCommand::BlindTransfer {
            call_id: call_id.to_string(), target,
        });
    }
    /// `target` must already be a fully-qualified SIP URI.
    pub fn redirect_call(&self, call_id: &str, target: String) {
        let _ = self.cmd_tx.send(SipCommand::RedirectCall {
            call_id: call_id.to_string(), target,
        });
    }
    /// Subscribe to a contact's presence. `target_uri` must already be a
    /// fully-qualified SIP URI (contacts store one directly, same as Call does).
    pub fn subscribe_presence(&self, target_uri: String) {
        let _ = self.cmd_tx.send(SipCommand::SubscribePresence { target_uri });
    }
    pub fn unsubscribe_presence(&self, target_uri: String) {
        let _ = self.cmd_tx.send(SipCommand::UnsubscribePresence { target_uri });
    }
    /// Attended-transfer `call_id` via REFER with a `Replaces` parameter
    /// referencing `consultation_call_id`'s dialog.
    pub fn attended_transfer(&self, call_id: &str, consultation_call_id: &str) {
        let _ = self.cmd_tx.send(SipCommand::AttendedTransfer {
            call_id: call_id.to_string(), consultation_call_id: consultation_call_id.to_string(),
        });
    }
}

// ── SIP Stack ─────────────────────────────────────────────────────────────────

pub struct SipStack {
    transport:     Arc<SipTransport>,
    account:       SipAccount,
    local_ip:      String,
    advertised_ip: String,
    local_port:    u16,
    server_addr:   SocketAddr,

    reg_call_id:  String,
    reg_from_tag: String,
    reg_cseq:     Arc<AtomicU32>,

    dialogs:       HashMap<String, Dialog>,
    subscriptions: HashMap<String, PresenceSubscription>,
    event_tx: mpsc::UnboundedSender<SipEvent>,
    cmd_rx:   mpsc::UnboundedReceiver<SipCommand>,
}

impl SipStack {
    pub async fn new(
        account:      SipAccount,
        local_port:   u16,
        external_ip:  Option<String>,
        event_tx:     mpsc::UnboundedSender<SipEvent>,
        cmd_rx:       mpsc::UnboundedReceiver<SipCommand>,
    ) -> anyhow::Result<Self> {
        let local_ip = local_ip_for(&account.server, account.port)?;
        let advertised_ip = external_ip.unwrap_or_else(|| local_ip.clone());

        let server_addr = tokio::net::lookup_host(format!("{}:{}", account.server, account.port))
            .await?
            .next()
            .ok_or_else(|| anyhow::anyhow!("DNS lookup failed for {}", account.server))?;

        let bind_addr: SocketAddr = format!("0.0.0.0:{local_port}")
            .parse()
            .context("Invalid bind address")?;
        let transport = Arc::new(
            SipTransport::connect(
                account.transport.clone(),
                bind_addr,
                server_addr,
                &account.server,
                account.tls_insecure_skip_verify,
            )
            .await?,
        );

        info!(
            local   = %format!("{local_ip}:{local_port}"),
            advertised = %advertised_ip,
            server  = %server_addr,
            "SIP stack ready"
        );

        let reg_call_id  = new_call_id(&local_ip);
        let reg_from_tag = new_tag();

        Ok(Self {
            transport,
            account,
            local_ip,
            advertised_ip,
            local_port,
            server_addr,
            reg_call_id,
            reg_from_tag,
            reg_cseq:  Arc::new(AtomicU32::new(1)),
            dialogs:   HashMap::new(),
            subscriptions: HashMap::new(),
            event_tx,
            cmd_rx,
        })
    }

    pub async fn spawn(
        account:     SipAccount,
        local_port:  u16,
        external_ip: Option<String>,
    ) -> anyhow::Result<SipHandle> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (cmd_tx,   cmd_rx)   = mpsc::unbounded_channel();
        let stack = SipStack::new(account, local_port, external_ip, event_tx, cmd_rx).await?;
        let advertised_ip = stack.advertised_ip.clone();
        let secure = stack.account.transport == TransportProtocol::Tls;
        let domain = stack.account.server.clone();
        tokio::spawn(async move {
            if let Err(e) = stack.run().await {
                error!("SIP stack crashed: {e}");
            }
        });
        Ok(SipHandle { event_rx, cmd_tx, advertised_ip, secure, domain })
    }

    // ── Main event loop ───────────────────────────────────────────────────────

    pub async fn run(mut self) -> anyhow::Result<()> {
        let mut reregister_at = Instant::now();
        let mut retry_delay   = Duration::from_secs(5);
        let mut presence_tick = interval(PRESENCE_TICK);

        loop {
            tokio::select! {
                _ = presence_tick.tick() => {
                    self.refresh_presence_subscriptions().await;
                }
                _ = sleep_until(reregister_at) => {
                    match self.register_once().await {
                        Ok(expires) => {
                            retry_delay   = Duration::from_secs(5);
                            reregister_at = Instant::now()
                                + Duration::from_secs(expires.saturating_sub(REG_MARGIN) as u64);
                            let _ = self.event_tx.send(SipEvent::Registered { expires });
                        }
                        Err(e) => {
                            error!("Registration failed: {e}");
                            let _ = self.event_tx.send(SipEvent::RegistrationFailed {
                                reason: e.to_string(),
                            });
                            reregister_at = Instant::now() + retry_delay;
                            retry_delay   = (retry_delay * 2).min(MAX_RETRY);
                        }
                    }
                }
                result = self.transport.recv() => {
                    match result {
                        Ok((data, from)) => {
                            if let Some(msg) = SipMessage::parse(&data) {
                                self.dispatch(msg, from).await;
                            }
                        }
                        Err(e) => {
                            error!("Transport error: {e}");
                            return Err(e);
                        }
                    }
                }
                Some(cmd) = self.cmd_rx.recv() => {
                    self.handle_command(cmd).await;
                }
            }
        }
    }

    fn via_proto(&self) -> &'static str {
        match self.account.transport {
            TransportProtocol::Udp => "UDP",
            TransportProtocol::Tcp => "TCP",
            TransportProtocol::Tls => "TLS",
        }
    }

    // ── Message dispatcher ────────────────────────────────────────────────────

    async fn dispatch(&mut self, msg: SipMessage, from: SocketAddr) {
        match msg.start_line.clone() {
            SipStartLine::Request { method, .. } => {
                match method {
                    SipMethod::Invite  => self.on_invite(msg, from).await,
                    SipMethod::Bye     => self.on_bye(msg, from).await,
                    SipMethod::Ack     => self.on_ack(msg),
                    SipMethod::Cancel  => self.on_cancel(msg, from).await,
                    SipMethod::Notify  => self.on_notify(msg, from).await,
                    SipMethod::Options => self.send_ok(&msg, from).await,
                    _                  => debug!(?method, "Ignoring unhandled request"),
                }
            }
            SipStartLine::Response { status, .. } => {
                self.on_response(msg, status, from).await;
            }
        }
    }

    // ── Registration ──────────────────────────────────────────────────────────

    async fn register_once(&mut self) -> anyhow::Result<u32> {
        self.send_register(None).await?;
        let resp = self.recv_reg_response().await?;

        match resp.status_code() {
            Some(200) => {
                info!("Registered (no auth)");
                return Ok(extract_expires(&resp).unwrap_or(REG_EXPIRES));
            }
            Some(401) | Some(407) => {}
            Some(c) => return Err(anyhow::anyhow!("REGISTER rejected: {c}")),
            None    => return Err(anyhow::anyhow!("Expected response")),
        }

        let hdr_name = if resp.status_code() == Some(407) {
            "Proxy-Authenticate"
        } else {
            "WWW-Authenticate"
        };
        let www_auth = resp
            .header(hdr_name)
            .ok_or_else(|| anyhow::anyhow!("Missing {hdr_name}"))?
            .to_owned();

        let uri = format!("sip:{}", self.account.server);
        let auth = build_challenge_response(
            &self.account.username, &self.account.password, "REGISTER", &uri, &www_auth,
        ).ok_or_else(|| anyhow::anyhow!("Bad challenge: {www_auth}"))?;

        self.send_register(Some(&auth)).await?;
        let resp2 = self.recv_reg_response().await?;
        match resp2.status_code() {
            Some(200) => {
                info!("Registered");
                Ok(extract_expires(&resp2).unwrap_or(REG_EXPIRES))
            }
            Some(c) => Err(anyhow::anyhow!("REGISTER rejected: {c}")),
            None    => Err(anyhow::anyhow!("Expected response")),
        }
    }

    async fn send_register(&self, auth: Option<&str>) -> anyhow::Result<()> {
        let cseq       = self.reg_cseq.fetch_add(1, Ordering::SeqCst);
        let branch     = new_branch();
        let server     = &self.account.server;
        let username   = &self.account.username;
        let adv_ip     = &self.advertised_ip;
        let local_ip   = &self.local_ip;
        let local_port = self.local_port;
        let call_id    = &self.reg_call_id;
        let from_tag   = &self.reg_from_tag;
        let display    = self.account.display_name.as_deref().unwrap_or(username);
        let via_proto  = self.via_proto();

        let mut msg = format!(
            "REGISTER sip:{server} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: \"{display}\" <sip:{username}@{server}>\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} REGISTER\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}>\r\n\
             Expires: {REG_EXPIRES}\r\n\
             User-Agent: DeeLip/0.1.0\r\n"
        );
        if let Some(a) = auth { msg.push_str(a); msg.push_str("\r\n"); }
        msg.push_str("Content-Length: 0\r\n\r\n");

        debug!("→ REGISTER");
        self.transport.send(msg.as_bytes(), self.server_addr).await
            .context("Sending REGISTER")
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
                None    => { warn!("Unparsable datagram during REGISTER"); continue; }
            };
            if matches!(msg.status_code(), Some(c) if c < 200) { continue; }
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

    // ── Command handler ───────────────────────────────────────────────────────

    async fn handle_command(&mut self, cmd: SipCommand) {
        match cmd {
            SipCommand::MakeCall   { to, local_sdp }     => self.initiate_call(&to, &local_sdp).await,
            SipCommand::AcceptCall { call_id, local_sdp } => self.accept_call(&call_id, &local_sdp).await,
            SipCommand::RejectCall { call_id }             => self.reject_call(&call_id).await,
            SipCommand::HangUp     { call_id }             => self.hang_up(&call_id).await,
            SipCommand::HoldCall   { call_id, local_sdp } => self.send_reinvite(&call_id, &local_sdp, true).await,
            SipCommand::ResumeCall { call_id, local_sdp } => self.send_reinvite(&call_id, &local_sdp, false).await,
            SipCommand::BlindTransfer { call_id, target }  => self.blind_transfer(&call_id, &target).await,
            SipCommand::RedirectCall  { call_id, target }  => self.redirect_call(&call_id, &target).await,
            SipCommand::SubscribePresence   { target_uri } => self.subscribe_presence(&target_uri).await,
            SipCommand::UnsubscribePresence { target_uri } => self.unsubscribe_presence(&target_uri).await,
            SipCommand::AttendedTransfer { call_id, consultation_call_id } =>
                self.attended_transfer(&call_id, &consultation_call_id).await,
        }
    }

    // ── Outgoing call ─────────────────────────────────────────────────────────

    async fn initiate_call(&mut self, to: &str, local_sdp: &str) {
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

        let mut msg = format!(
            "INVITE {to} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{to}>\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} INVITE\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}>\r\n\
             Content-Type: application/sdp\r\n\
             User-Agent: DeeLip/0.1.0\r\n"
        );
        if let Some(a) = auth { msg.push_str(a); msg.push_str("\r\n"); }
        msg.push_str(&format!("Content-Length: {body_len}\r\n\r\n{sdp}"));
        msg
    }

    // ── Hold / Resume (re-INVITE) ─────────────────────────────────────────────

    async fn send_reinvite(&mut self, call_id: &str, local_sdp: &str, hold: bool) {
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

        let reinvite = format!(
            "INVITE {to_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id_s}\r\n\
             CSeq: {cseq} INVITE\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}>\r\n\
             Content-Type: application/sdp\r\n\
             User-Agent: DeeLip/0.1.0\r\n\
             Content-Length: {body_len}\r\n\r\n\
             {local_sdp}"
        );
        debug!("→ re-INVITE ({})", if hold { "hold" } else { "resume" });
        let _ = self.transport.send(reinvite.as_bytes(), contact).await;
    }

    // ── Blind transfer (REFER) ────────────────────────────────────────────────

    /// Blind-transfer an active call via REFER. `target` must already be a
    /// fully-qualified SIP URI. Fire-and-forget beyond the REFER response
    /// itself (see `SipEvent::TransferAccepted`/`TransferFailed`) — no NOTIFY
    /// sipfrag progress tracking; the far end normally sends BYE on this
    /// dialog once the transferred call succeeds.
    async fn blind_transfer(&mut self, call_id: &str, target: &str) {
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

        let refer = format!(
            "REFER {to_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id_s}\r\n\
             CSeq: {cseq} REFER\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}>\r\n\
             Refer-To: <{target}>\r\n\
             User-Agent: DeeLip/0.1.0\r\n\
             Content-Length: 0\r\n\r\n"
        );
        debug!("→ REFER {to_uri} (Refer-To: {target})");
        let _ = self.transport.send(refer.as_bytes(), contact).await;
    }

    /// Attended transfer: sends REFER on the ORIGINAL call's dialog with a
    /// `Replaces` parameter (RFC 3891) referencing the CONSULTATION call's
    /// dialog identity, so the transferee re-INVITEs the consultation
    /// target directly instead of dialing fresh. Mirrors `blind_transfer`'s
    /// header shape exactly, differing only in the `Refer-To` value.
    async fn attended_transfer(&mut self, call_id: &str, consultation_call_id: &str) {
        let (target, replaces) = {
            let Some(consult) = self.dialogs.get(consultation_call_id) else { return };
            let replaces = format!(
                "{};to-tag={};from-tag={}",
                consult.call_id,
                consult.remote_tag.as_deref().unwrap_or(""),
                consult.local_tag,
            );
            (consult.remote_uri.clone(), replaces)
        };
        let refer_to = format!("{target}?Replaces={}", encode_replaces_param(&replaces));

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

        let refer = format!(
            "REFER {to_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id_s}\r\n\
             CSeq: {cseq} REFER\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}>\r\n\
             Refer-To: <{refer_to}>\r\n\
             User-Agent: DeeLip/0.1.0\r\n\
             Content-Length: 0\r\n\r\n"
        );
        debug!("→ REFER {to_uri} (attended transfer, Replaces: {replaces})");
        let _ = self.transport.send(refer.as_bytes(), contact).await;
    }

    /// Redirect a not-yet-answered incoming call via 302 Moved Temporarily —
    /// `target` must already be a fully-qualified SIP URI. Used for the
    /// no-answer-forward timeout; removes the dialog like `reject_call` does.
    async fn redirect_call(&mut self, call_id: &str, target: &str) {
        if let Some(dialog) = self.dialogs.remove(call_id) {
            let contact: SocketAddr = dialog.remote_contact
                .as_deref()
                .and_then(|s| s.parse().ok())
                .unwrap_or(self.server_addr);
            let branch     = new_branch();
            let local_ip   = &self.local_ip;
            let local_port = self.local_port;
            let username   = &self.account.username;
            let server     = &self.account.server;
            let display    = self.account.display_name.as_deref().unwrap_or(username);
            let local_tag  = &dialog.local_tag;
            let remote_uri = &dialog.remote_uri;
            let from_tag   = dialog.remote_tag.as_deref()
                .map(|t| format!(";tag={t}")).unwrap_or_default();
            let cseq_n = dialog.remote_cseq.unwrap_or(1);
            let via_proto = self.via_proto();

            let redirect = format!(
                "SIP/2.0 302 Moved Temporarily\r\n\
                 Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch}\r\n\
                 To: \"{display}\" <sip:{username}@{server}>;tag={local_tag}\r\n\
                 From: <{remote_uri}>{from_tag}\r\n\
                 Call-ID: {call_id}\r\n\
                 CSeq: {cseq_n} INVITE\r\n\
                 Contact: <{target}>\r\n\
                 Content-Length: 0\r\n\r\n"
            );
            debug!("→ 302 Moved Temporarily {call_id} (Contact: {target})");
            let _ = self.transport.send(redirect.as_bytes(), contact).await;
        }
    }

    // ── Presence (SUBSCRIBE/NOTIFY, Event: presence) ─────────────────────────

    async fn subscribe_presence(&mut self, target_uri: &str) {
        let call_id  = new_call_id(&self.local_ip);
        let from_tag = new_tag();
        let sub = PresenceSubscription::new(call_id.clone(), from_tag.clone(), target_uri.to_string());

        let msg = self.build_subscribe(&call_id, &from_tag, 1, target_uri, SUBSCRIBE_EXPIRES, None);
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
    async fn unsubscribe_presence(&mut self, target_uri: &str) {
        let matching: Vec<String> = self.subscriptions.iter()
            .filter(|(_, s)| s.target_uri == target_uri)
            .map(|(id, _)| id.clone())
            .collect();

        for call_id in matching {
            if let Some(sub) = self.subscriptions.get_mut(&call_id) {
                let cseq     = sub.next_local_cseq();
                let from_tag = sub.local_tag.clone();
                let msg = self.build_subscribe(&call_id, &from_tag, cseq, target_uri, 0, None);
                debug!("→ SUBSCRIBE {target_uri} (Expires: 0, unsubscribe)");
                let _ = self.transport.send(msg.as_bytes(), self.server_addr).await;
            }
            self.subscriptions.remove(&call_id);
        }
    }

    /// Re-SUBSCRIBE any subscription whose `refresh_after` has passed —
    /// called from a coarse 30s tick in `run()` rather than a precise
    /// per-subscription deadline, which is plenty for hour-scale expiries.
    async fn refresh_presence_subscriptions(&mut self) {
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
            let msg = self.build_subscribe(&call_id, &from_tag, cseq, &target_uri, SUBSCRIBE_EXPIRES, None);
            debug!("→ SUBSCRIBE {target_uri} (refresh)");
            let _ = self.transport.send(msg.as_bytes(), self.server_addr).await;
        }
    }

    fn build_subscribe(
        &self,
        call_id:    &str,
        from_tag:   &str,
        cseq:       u32,
        target_uri: &str,
        expires:    u32,
        auth:       Option<&str>,
    ) -> String {
        let branch     = new_branch();
        let server     = &self.account.server;
        let username   = &self.account.username;
        let adv_ip     = &self.advertised_ip;
        let local_ip   = &self.local_ip;
        let local_port = self.local_port;
        let display    = self.account.display_name.as_deref().unwrap_or(username);
        let via_proto  = self.via_proto();

        let mut msg = format!(
            "SUBSCRIBE {target_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{target_uri}>\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} SUBSCRIBE\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}>\r\n\
             Event: presence\r\n\
             Accept: application/pidf+xml\r\n\
             Expires: {expires}\r\n\
             User-Agent: DeeLip/0.1.0\r\n"
        );
        if let Some(a) = auth { msg.push_str(a); msg.push_str("\r\n"); }
        msg.push_str("Content-Length: 0\r\n\r\n");
        msg
    }

    async fn on_subscribe_response(&mut self, msg: SipMessage, status: u16, call_id: String) {
        match status {
            200 => {
                let expires = extract_expires(&msg).unwrap_or(SUBSCRIBE_EXPIRES);
                let uri = if let Some(sub) = self.subscriptions.get_mut(&call_id) {
                    if sub.remote_tag.is_none() {
                        sub.remote_tag = parse_tag(msg.header("To").unwrap_or(""));
                    }
                    sub.refresh_after = Instant::now() + Duration::from_secs((expires as u64 * 9) / 10);
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
                let retry = self.build_subscribe(&call_id, &from_tag, cseq, &target_uri, SUBSCRIBE_EXPIRES, Some(&auth));
                let _ = self.transport.send(retry.as_bytes(), self.server_addr).await;
            }
            c if c >= 300 => {
                let uri = self.subscriptions.remove(&call_id).map(|s| s.target_uri).unwrap_or_default();
                let _ = self.event_tx.send(SipEvent::PresenceSubscribeFailed { uri, reason: format!("{c}") });
            }
            _ => {}
        }
    }

    async fn on_notify(&mut self, msg: SipMessage, from: SocketAddr) {
        let call_id = msg.call_id().map(str::to_string);
        let is_presence = call_id.as_deref().is_some_and(|id| self.subscriptions.contains_key(id));

        if is_presence {
            let call_id = call_id.unwrap();
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
        }

        // Non-presence NOTIFY (e.g. blind-transfer's sipfrag) falls through to
        // an unconditional blind-ack, unchanged from before this feature.
        self.send_ok(&msg, from).await;
    }

    // ── Response handler ──────────────────────────────────────────────────────

    async fn on_response(&mut self, msg: SipMessage, status: u16, _from: SocketAddr) {
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
        // call dialogs at all -- handled entirely against `self.subscriptions`.
        if matches!(msg.cseq(), Some((_, SipMethod::Subscribe))) {
            self.on_subscribe_response(msg, status, call_id).await;
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
                    reason:  msg.header("Reason").unwrap_or("").to_string(),
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

    fn build_ack(
        &self,
        call_id:  &str,
        from_tag: &str,
        to_uri:   &str,
        to_tag:   Option<&str>,
        cseq:     u32,
    ) -> String {
        let branch      = new_branch();
        let server      = &self.account.server;
        let username    = &self.account.username;
        let adv_ip      = &self.advertised_ip;
        let local_ip    = &self.local_ip;
        let local_port  = self.local_port;
        let to_tag_part = to_tag.map(|t| format!(";tag={t}")).unwrap_or_default();
        let display     = self.account.display_name.as_deref().unwrap_or(username);
        let via_proto   = self.via_proto();

        format!(
            "ACK {to_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch}\r\n\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag_part}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} ACK\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}>\r\n\
             Content-Length: 0\r\n\r\n"
        )
    }

    // ── Incoming INVITE ───────────────────────────────────────────────────────

    async fn on_invite(&mut self, msg: SipMessage, from: SocketAddr) {
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
        let local_tag  = new_tag();

        let trying  = self.build_response(&msg, 100, "Trying",  &local_tag, "");
        let ringing = self.build_response(&msg, 180, "Ringing", &local_tag, "");
        let _ = self.transport.send(trying.as_bytes(),  from).await;
        let _ = self.transport.send(ringing.as_bytes(), from).await;

        let mut dialog = Dialog::new_incoming(
            call_id.clone(), local_tag, from_uri.clone(),
            from_tag, cseq_n, remote_sdp.clone(),
        );
        dialog.remote_contact = Some(from.to_string());
        self.dialogs.insert(call_id.clone(), dialog);

        let _ = self.event_tx.send(SipEvent::IncomingCall { call_id, from: from_uri, remote_sdp });
    }

    async fn accept_call(&mut self, call_id: &str, local_sdp: &str) {
        let via_proto = self.via_proto();
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
        let adv_ip       = self.advertised_ip.clone();
        let local_ip     = self.local_ip.clone();
        let local_port   = self.local_port;
        let username     = self.account.username.clone();
        let server       = self.account.server.clone();
        let display      = self.account.display_name.clone()
            .unwrap_or_else(|| username.clone());
        let body_len     = local_sdp.len();
        let branch       = new_branch();

        let from_tag_part = remote_tag.as_deref()
            .map(|t| format!(";tag={t}"))
            .unwrap_or_default();

        let ok_msg = format!(
            "SIP/2.0 200 OK\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch}\r\n\
             To: \"{display}\" <sip:{username}@{server}>;tag={local_tag}\r\n\
             From: <{remote_uri}>{from_tag_part}\r\n\
             Call-ID: {call_id_str}\r\n\
             CSeq: {cseq_n} INVITE\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}>\r\n\
             Content-Type: application/sdp\r\n\
             User-Agent: DeeLip/0.1.0\r\n\
             Content-Length: {body_len}\r\n\r\n\
             {local_sdp}"
        );

        let _ = self.transport.send(ok_msg.as_bytes(), contact).await;
        dialog.state     = DialogState::Confirmed;
        dialog.local_sdp = Some(local_sdp.to_string());
    }

    async fn reject_call(&mut self, call_id: &str) {
        if let Some(dialog) = self.dialogs.remove(call_id) {
            let contact: SocketAddr = dialog.remote_contact
                .as_deref()
                .and_then(|s| s.parse().ok())
                .unwrap_or(self.server_addr);
            let branch     = new_branch();
            let local_ip   = &self.local_ip;
            let local_port = self.local_port;
            let username   = &self.account.username;
            let server     = &self.account.server;
            let display    = self.account.display_name.as_deref().unwrap_or(username);
            let local_tag  = &dialog.local_tag;
            let remote_uri = &dialog.remote_uri;
            let from_tag   = dialog.remote_tag.as_deref()
                .map(|t| format!(";tag={t}")).unwrap_or_default();
            let cseq_n = dialog.remote_cseq.unwrap_or(1);
            let via_proto = self.via_proto();

            let decline = format!(
                "SIP/2.0 486 Busy Here\r\n\
                 Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch}\r\n\
                 To: \"{display}\" <sip:{username}@{server}>;tag={local_tag}\r\n\
                 From: <{remote_uri}>{from_tag}\r\n\
                 Call-ID: {call_id}\r\n\
                 CSeq: {cseq_n} INVITE\r\n\
                 Content-Length: 0\r\n\r\n"
            );
            let _ = self.transport.send(decline.as_bytes(), contact).await;
        }
    }

    async fn hang_up(&mut self, call_id: &str) {
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

        let bye = format!(
            "BYE {to_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id_s}\r\n\
             CSeq: {cseq} BYE\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}>\r\n\
             User-Agent: DeeLip/0.1.0\r\n\
             Content-Length: 0\r\n\r\n"
        );
        let _ = self.transport.send(bye.as_bytes(), contact).await;
    }

    // ── Incoming BYE / ACK / CANCEL ──────────────────────────────────────────

    async fn on_bye(&mut self, msg: SipMessage, from: SocketAddr) {
        let call_id = match msg.call_id() {
            Some(id) => id.to_string(),
            None     => return,
        };
        let ok = self.build_response(&msg, 200, "OK", "", "");
        let _ = self.transport.send(ok.as_bytes(), from).await;
        if let Some(mut dialog) = self.dialogs.remove(&call_id) {
            dialog.state = DialogState::Terminated;
        }
        let _ = self.event_tx.send(SipEvent::CallEnded { call_id });
    }

    fn on_ack(&mut self, msg: SipMessage) {
        if let Some(id) = msg.call_id().map(str::to_string) {
            if let Some(d) = self.dialogs.get_mut(&id) {
                if d.state == DialogState::Calling {
                    d.state = DialogState::Confirmed;
                }
            }
        }
    }

    async fn on_cancel(&mut self, msg: SipMessage, from: SocketAddr) {
        let ok = self.build_response(&msg, 200, "OK", "", "");
        let _ = self.transport.send(ok.as_bytes(), from).await;
        if let Some(call_id) = msg.call_id() {
            let call_id = call_id.to_string();
            self.dialogs.remove(&call_id);
            let _ = self.event_tx.send(SipEvent::CallEnded { call_id });
        }
    }

    async fn send_ok(&self, req: &SipMessage, from: SocketAddr) {
        let ok = self.build_response(req, 200, "OK", "", "");
        let _ = self.transport.send(ok.as_bytes(), from).await;
    }

    // ── Response builders ─────────────────────────────────────────────────────

    fn build_response(&self, req: &SipMessage, code: u16, phrase: &str, to_tag: &str, body: &str) -> String {
        self.build_response_with_body(req, code, phrase, to_tag, body)
    }

    fn build_response_with_body(&self, req: &SipMessage, code: u16, phrase: &str, to_tag: &str, body: &str) -> String {
        let via      = req.header("Via").unwrap_or("");
        let from     = req.header("From").unwrap_or("");
        let to       = req.header("To").unwrap_or("");
        let call_id  = req.header("Call-ID").unwrap_or("");
        let cseq     = req.header("CSeq").unwrap_or("");
        let body_len = body.len();

        let to_line = if !to_tag.is_empty() && !to.contains(";tag=") {
            format!("{to};tag={to_tag}")
        } else {
            to.to_string()
        };

        let ct_header = if !body.is_empty() {
            "Content-Type: application/sdp\r\n"
        } else {
            ""
        };

        let mut resp = format!(
            "SIP/2.0 {code} {phrase}\r\n\
             Via: {via}\r\n\
             To: {to_line}\r\n\
             From: {from}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq}\r\n\
             User-Agent: DeeLip/0.1.0\r\n\
             {ct_header}\
             Content-Length: {body_len}\r\n\r\n"
        );
        if !body.is_empty() { resp.push_str(body); }
        resp
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn extract_expires(msg: &SipMessage) -> Option<u32> {
    if let Some(v) = msg.header("Expires") {
        if let Ok(n) = v.trim().parse::<u32>() { return Some(n); }
    }
    if let Some(contact) = msg.header("Contact") {
        for param in contact.split(';') {
            if let Some(v) = param.trim().strip_prefix("expires=") {
                if let Ok(n) = v.trim_matches('"').parse::<u32>() { return Some(n); }
            }
        }
    }
    None
}

fn parse_tag(header: &str) -> Option<String> {
    for part in header.split(';') {
        if let Some(v) = part.trim().strip_prefix("tag=") {
            return Some(v.to_string());
        }
    }
    None
}

fn parse_uri(header: &str) -> Option<String> {
    if let Some(start) = header.find('<') {
        if let Some(end) = header.find('>') {
            return Some(header[start + 1..end].to_string());
        }
    }
    Some(header.split(';').next()?.trim().to_string())
}

/// Percent-encode a `Replaces` value (RFC 3891) for embedding as a URI
/// parameter. Our own generated call-ids/tags are plain hex and never
/// actually contain these characters, but this is correct regardless.
/// `%` must be encoded first to avoid double-encoding the others' output.
fn encode_replaces_param(s: &str) -> String {
    s.replace('%', "%25")
        .replace(';', "%3B")
        .replace('=', "%3D")
        .replace(',', "%2C")
        .replace('@', "%40")
}
