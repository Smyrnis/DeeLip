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
    mwi::{parse_mwi_summary, MwiSubscription},
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
const PRESENCE_EVENT:  &str = "presence";
const PRESENCE_ACCEPT: &str = "application/pidf+xml";
const MWI_EVENT:  &str = "message-summary";
const MWI_ACCEPT: &str = "application/simple-message-summary";

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
    /// Subscribe to a mailbox's voicemail MWI state. `target_uri` must
    /// already be a fully-qualified SIP URI.
    pub fn subscribe_mwi(&self, target_uri: String) {
        let _ = self.cmd_tx.send(SipCommand::SubscribeMwi { target_uri });
    }
    /// Attended-transfer `call_id` via REFER with a `Replaces` parameter
    /// referencing `consultation_call_id`'s dialog.
    pub fn attended_transfer(&self, call_id: &str, consultation_call_id: &str) {
        let _ = self.cmd_tx.send(SipCommand::AttendedTransfer {
            call_id: call_id.to_string(), consultation_call_id: consultation_call_id.to_string(),
        });
    }
    /// Send one DTMF digit via SIP INFO instead of RFC 2833 RTP events.
    pub fn send_dtmf_info(&self, call_id: &str, digit: char) {
        let _ = self.cmd_tx.send(SipCommand::SendDtmfInfo {
            call_id: call_id.to_string(), digit,
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
    mwi_subscriptions: HashMap<String, MwiSubscription>,
    event_tx: mpsc::UnboundedSender<SipEvent>,
    cmd_rx:   mpsc::UnboundedReceiver<SipCommand>,
}

/// The command-receiving half survives across a reconnect (it's tied to the
/// `cmd_tx` held externally by `SipHandle`, which must transparently keep
/// working across a transport failure) -- both `SipStack::new` and `run`
/// hand it back on failure, via this alias, so `spawn`'s reconnect loop can
/// feed it into the next attempt instead of losing it.
type CmdRx = mpsc::UnboundedReceiver<SipCommand>;

impl SipStack {
    pub async fn new(
        account:      SipAccount,
        local_port:   u16,
        external_ip:  Option<String>,
        event_tx:     mpsc::UnboundedSender<SipEvent>,
        cmd_rx:       CmdRx,
    ) -> Result<Self, (anyhow::Error, CmdRx)> {
        let (transport, local_ip, advertised_ip, server_addr) =
            match Self::connect_transport(&account, local_port, &external_ip).await {
                Ok(c) => c,
                Err(e) => return Err((e, cmd_rx)),
            };

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
            mwi_subscriptions: HashMap::new(),
            event_tx,
            cmd_rx,
        })
    }

    /// Just the connection-establishing steps (DNS resolution, socket bind,
    /// transport connect) -- deliberately takes no ownership of `cmd_rx`/
    /// `event_tx` so a failure here (used both for the first connection and
    /// every later reconnect attempt) never loses the command-channel
    /// receiver `spawn`'s reconnect loop needs to keep retrying with.
    async fn connect_transport(
        account:     &SipAccount,
        local_port:  u16,
        external_ip: &Option<String>,
    ) -> anyhow::Result<(Arc<SipTransport>, String, String, SocketAddr)> {
        let local_ip = local_ip_for(&account.server, account.port)?;
        let advertised_ip = external_ip.clone().unwrap_or_else(|| local_ip.clone());

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

        Ok((transport, local_ip, advertised_ip, server_addr))
    }

    /// Spawns the background task that runs this account's SIP stack for
    /// the lifetime of the process. A transport failure (dropped TLS/TCP
    /// connection, etc.) doesn't kill the account permanently -- `run()`
    /// hands back the still-good `cmd_rx` on failure, and this loop
    /// reconnects with the same exponential backoff shape already used for
    /// registration retries, reusing the *same* `cmd_tx`/`event_rx` pair
    /// `SipHandle` was constructed with so the reconnect is transparent to
    /// callers (in-flight dialogs/subscriptions are necessarily lost across
    /// a transport replacement, same as they always were the moment a
    /// disconnect happened -- but the account itself now recovers instead
    /// of staying dead until the whole process is restarted).
    pub async fn spawn(
        account:     SipAccount,
        local_port:  u16,
        external_ip: Option<String>,
    ) -> anyhow::Result<SipHandle> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (cmd_tx,   cmd_rx)   = mpsc::unbounded_channel();
        let stack = SipStack::new(account.clone(), local_port, external_ip.clone(), event_tx.clone(), cmd_rx)
            .await
            .map_err(|(e, _)| e)?;
        let advertised_ip = stack.advertised_ip.clone();
        let secure = stack.account.transport == TransportProtocol::Tls;
        let domain = stack.account.server.clone();

        tokio::spawn(async move {
            let mut stack: Option<SipStack> = Some(stack);
            let mut pending_cmd_rx: Option<CmdRx> = None;
            let mut retry_delay = Duration::from_secs(5);

            loop {
                if stack.is_none() {
                    let cmd_rx = pending_cmd_rx.take()
                        .expect("no live stack means a previous attempt stashed its cmd_rx");
                    match SipStack::new(account.clone(), local_port, external_ip.clone(), event_tx.clone(), cmd_rx).await {
                        Ok(s) => {
                            info!("Reconnected");
                            stack = Some(s);
                            retry_delay = Duration::from_secs(5);
                        }
                        Err((e, cmd_rx)) => {
                            error!("Reconnect attempt failed ({e:#}), retrying in {retry_delay:?}");
                            pending_cmd_rx = Some(cmd_rx);
                            tokio::time::sleep(retry_delay).await;
                            retry_delay = (retry_delay * 2).min(MAX_RETRY);
                            continue;
                        }
                    }
                }

                match stack.take().unwrap().run().await {
                    // Only reachable if `run()` ever grows a deliberate
                    // graceful-shutdown path -- it doesn't today, but the
                    // shape should stay correct if that changes.
                    Ok(()) => break,
                    Err((e, cmd_rx)) => {
                        error!("SIP stack disconnected ({e:#}), reconnecting in {retry_delay:?}");
                        let _ = event_tx.send(SipEvent::RegistrationFailed {
                            reason: format!("Disconnected: {e:#}"),
                        });
                        pending_cmd_rx = Some(cmd_rx);
                        tokio::time::sleep(retry_delay).await;
                        retry_delay = (retry_delay * 2).min(MAX_RETRY);
                    }
                }
            }
        });
        Ok(SipHandle { event_rx, cmd_tx, advertised_ip, secure, domain })
    }

    // ── Main event loop ───────────────────────────────────────────────────────

    pub async fn run(mut self) -> Result<(), (anyhow::Error, CmdRx)> {
        let mut reregister_at = Instant::now();
        let mut retry_delay   = Duration::from_secs(5);
        let mut presence_tick = interval(PRESENCE_TICK);

        loop {
            tokio::select! {
                _ = presence_tick.tick() => {
                    self.refresh_presence_subscriptions().await;
                    self.refresh_mwi_subscriptions().await;
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
                            error!("Transport error: {e:#}");
                            return Err((e, self.cmd_rx));
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

    /// `;transport=...` URI parameter for our own `Contact:` header — empty
    /// for UDP (the default the far end assumes with no parameter at all),
    /// explicit otherwise so a peer sending a fresh request back to us
    /// (e.g. an Asterisk-originated INVITE) knows to reuse/re-establish
    /// TCP/TLS rather than defaulting to UDP on our registered port, which
    /// silently goes nowhere since we never bind a UDP listener there.
    fn contact_transport_param(&self) -> &'static str {
        match self.account.transport {
            TransportProtocol::Udp => "",
            TransportProtocol::Tcp => ";transport=tcp",
            TransportProtocol::Tls => ";transport=tls",
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
        let contact_transport = self.contact_transport_param();

        let mut msg = format!(
            "REGISTER sip:{server} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: \"{display}\" <sip:{username}@{server}>\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} REGISTER\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
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
            SipCommand::SendDtmfInfo { call_id, digit } => self.send_dtmf_info(&call_id, digit).await,
            SipCommand::SubscribeMwi { target_uri } => self.subscribe_mwi(&target_uri).await,
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
        let contact_transport = self.contact_transport_param();

        let refer = format!(
            "REFER {to_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id_s}\r\n\
             CSeq: {cseq} REFER\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
             Refer-To: <{target}>\r\n\
             User-Agent: DeeLip/0.1.0\r\n\
             Content-Length: 0\r\n\r\n"
        );
        debug!("→ REFER {to_uri} (Refer-To: {target})");
        let _ = self.transport.send(refer.as_bytes(), contact).await;
    }

    /// Send one DTMF digit via SIP INFO (`application/dtmf-relay`, the
    /// long-standing de facto format most PBXes/gateways that support this
    /// scheme at all expect) instead of an RFC 2833 RTP telephone-event
    /// burst. Mirrors `blind_transfer`'s header shape exactly.
    async fn send_dtmf_info(&mut self, call_id: &str, digit: char) {
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
        let contact_transport = self.contact_transport_param();

        let refer = format!(
            "REFER {to_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id_s}\r\n\
             CSeq: {cseq} REFER\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
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
    async fn unsubscribe_presence(&mut self, target_uri: &str) {
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
        let branch     = new_branch();
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

    async fn on_presence_subscribe_response(&mut self, msg: SipMessage, status: u16, call_id: String) {
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

    async fn subscribe_mwi(&mut self, target_uri: &str) {
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
    async fn refresh_mwi_subscriptions(&mut self) {
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

    async fn on_mwi_subscribe_response(&mut self, msg: SipMessage, status: u16, call_id: String) {
        match status {
            200 => {
                let expires = extract_expires(&msg).unwrap_or(SUBSCRIBE_EXPIRES);
                let uri = if let Some(sub) = self.mwi_subscriptions.get_mut(&call_id) {
                    if sub.remote_tag.is_none() {
                        sub.remote_tag = parse_tag(msg.header("To").unwrap_or(""));
                    }
                    sub.refresh_after = Instant::now() + Duration::from_secs((expires as u64 * 9) / 10);
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

    async fn on_notify(&mut self, msg: SipMessage, from: SocketAddr) {
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
        let contact_transport = self.contact_transport_param();

        format!(
            "ACK {to_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch}\r\n\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag_part}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} ACK\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
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
