use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use anyhow::Context;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration, Instant, sleep_until};
use tracing::{debug, error, info};

use deelip_config::{SipAccount, TransportProtocol};
use deelip_nat::{IceConnection, IceGathered, TurnRelay};

use crate::{
    call::dialog::{Dialog, PendingOfferMedia},
    call::media_setup::NetworkConfig,
    events::{SipCommand, SipEvent},
    handle::SipHandle,
    subscription::mwi::MwiSubscription,
    subscription::presence::PresenceSubscription,
    transport::SipTransport,
    wire::message::{SipMessage, SipMethod, SipStartLine},
    wire::sdp::{AudioCodec, ParsedSdp, SrtpParams},
    wire::util::local_ip_for,
};

const REG_MARGIN:         u32      = 60;
pub(crate) const REG_RECV_TIMEOUT:   Duration = Duration::from_secs(10);
const MAX_RETRY:          Duration = Duration::from_secs(300);
pub(crate) const SUBSCRIBE_EXPIRES:  u32      = 3600;
const PRESENCE_TICK:      Duration = Duration::from_secs(30);
pub(crate) const PRESENCE_EVENT: &str = "presence";
pub(crate) const PRESENCE_ACCEPT: &str = "application/pidf+xml";
pub(crate) const MWI_EVENT: &str = "message-summary";
pub(crate) const MWI_ACCEPT: &str = "application/simple-message-summary";

// ── Background call-setup results ─────────────────────────────────────────────

/// STUN/TURN/ICE resolution (`media_setup::try_gather_ice`/`try_answer_with_ice`/
/// `resolve_rtp_endpoint`/`finish_ice_connect`) is real network I/O bounded by
/// a multi-second timeout -- running it inline inside `initiate_call`/
/// `accept_call`/`on_response` would block this account's *entire* event loop
/// (every other call's BYE/hold-resume, incoming messages, re-registration)
/// for that whole time, since `run()`'s `select!` fully awaits one branch
/// before looping back. Each of those three call sites instead spawns the
/// resolution as its own task and reports the result back here, so `run()`
/// picks it up as just another `select!` branch (`internal_rx`) alongside
/// everything else, instead of awaiting it inline.
pub(crate) enum StackEvent {
    /// `initiate_call`'s offer is ready to send as the actual INVITE.
    OutgoingOfferReady {
        call_id:       String,
        from_tag:      String,
        branch:        String,
        to:            String,
        local_sdp:     String,
        pending_offer: PendingOfferMedia,
        ice_gathered:  Option<IceGathered>,
    },
    /// `accept_call`'s answer is ready to send as the 200 OK.
    IncomingAnswerReady {
        call_id:    String,
        parsed:     ParsedSdp,
        local_sdp:  String,
        local_rtp:  u16,
        local_srtp: Option<SrtpParams>,
        relay:      Option<TurnRelay>,
        ice:        Option<IceConnection>,
    },
    /// The offerer side's ICE connectivity checks (`on_response`'s answer
    /// handling) finished -- media is now fully resolved for an outgoing call.
    OutgoingConnected {
        call_id:     String,
        local_rtp:   u16,
        local_srtp:  Option<SrtpParams>,
        relay:       Option<TurnRelay>,
        ice:         Option<IceConnection>,
        codec:       AudioCodec,
        dtmf_type:   Option<u8>,
        cn_type:     Option<u8>,
        remote_rtp:  SocketAddr,
        remote_srtp: Option<SrtpParams>,
    },
}

// ── SIP Stack ─────────────────────────────────────────────────────────────────

pub struct SipStack {
    pub(crate) transport:     Arc<SipTransport>,
    pub(crate) account:       SipAccount,
    pub(crate) network:       NetworkConfig,
    pub(crate) local_ip:      String,
    pub(crate) advertised_ip: String,
    pub(crate) local_port:    u16,
    pub(crate) server_addr:   SocketAddr,
    /// The concrete transport actually in use -- identical to
    /// `account.transport` unless that's `TransportProtocol::Auto`, in
    /// which case this is whichever of Udp/Tcp/Tls `connect_transport`
    /// resolved it to. Everything that cares about "is this connection
    /// TLS/UDP/TCP" (via headers, SRTP-by-default, `SipHandle.secure`)
    /// reads this, never `account.transport` directly.
    pub(crate) resolved_transport: TransportProtocol,

    pub(crate) reg_call_id:  String,
    pub(crate) reg_from_tag: String,
    pub(crate) reg_cseq:     Arc<AtomicU32>,

    pub(crate) dialogs:       HashMap<String, Dialog>,
    pub(crate) subscriptions: HashMap<String, PresenceSubscription>,
    pub(crate) mwi_subscriptions: HashMap<String, MwiSubscription>,
    /// Outstanding SIP MESSAGE requests awaiting their response, keyed by
    /// Call-ID -- MESSAGE (RFC 3428) is a standalone transaction, not part
    /// of any `Dialog`, so it can't be resolved via `dialogs`.
    pub(crate) pending_messages: HashMap<String, crate::message_method::PendingMessage>,
    pub(crate) event_tx: mpsc::UnboundedSender<SipEvent>,
    pub(crate) cmd_rx:   mpsc::UnboundedReceiver<SipCommand>,

    /// See `StackEvent`'s doc comment -- `internal_tx` is cloned into each
    /// background call-setup task; `internal_rx` is polled by `run()`'s own
    /// `select!` loop, right alongside `cmd_rx`/`transport.recv()`.
    pub(crate) internal_tx: mpsc::UnboundedSender<StackEvent>,
    internal_rx: mpsc::UnboundedReceiver<StackEvent>,
}

/// The command-receiving half survives across a reconnect (it's tied to the
/// `cmd_tx` held externally by `SipHandle`, which must transparently keep
/// working across a transport failure) -- both `SipStack::new` and `run`
/// hand it back on failure, via this alias, so `spawn`'s reconnect loop can
/// feed it into the next attempt instead of losing it.
pub(crate) type CmdRx = mpsc::UnboundedReceiver<SipCommand>;

impl SipStack {
    pub async fn new(
        account:      SipAccount,
        network:      NetworkConfig,
        local_port:   u16,
        external_ip:  Option<String>,
        event_tx:     mpsc::UnboundedSender<SipEvent>,
        cmd_rx:       CmdRx,
    ) -> Result<Self, (anyhow::Error, CmdRx)> {
        let (transport, local_ip, advertised_ip, server_addr, resolved_transport) =
            match Self::connect_transport(&account, local_port, &external_ip).await {
                Ok(c) => c,
                Err(e) => return Err((e, cmd_rx)),
            };

        let reg_call_id  = crate::wire::util::new_call_id(&local_ip);
        let reg_from_tag = crate::wire::util::new_tag();
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();

        Ok(Self {
            transport,
            account,
            network,
            local_ip,
            advertised_ip,
            local_port,
            server_addr,
            resolved_transport,
            reg_call_id,
            reg_from_tag,
            reg_cseq:  Arc::new(AtomicU32::new(1)),
            dialogs:   HashMap::new(),
            subscriptions: HashMap::new(),
            mwi_subscriptions: HashMap::new(),
            pending_messages: HashMap::new(),
            event_tx,
            cmd_rx,
            internal_tx,
            internal_rx,
        })
    }

    /// Dispatches to either a single concrete connect (`connect_transport_concrete`)
    /// or, for `TransportProtocol::Auto`, a probing attempt across all three
    /// candidates (`connect_transport_auto`) -- deliberately takes no
    /// ownership of `cmd_rx`/`event_tx` so a failure here (used both for the
    /// first connection and every later reconnect attempt) never loses the
    /// command-channel receiver `spawn`'s reconnect loop needs to keep
    /// retrying with.
    async fn connect_transport(
        account:     &SipAccount,
        local_port:  u16,
        external_ip: &Option<String>,
    ) -> anyhow::Result<(Arc<SipTransport>, String, String, SocketAddr, TransportProtocol)> {
        if account.transport == TransportProtocol::Auto {
            Self::connect_transport_auto(account, local_port, external_ip).await
        } else {
            let proto = account.transport;
            let (transport, local_ip, advertised_ip, server_addr) =
                Self::connect_transport_concrete(account, proto, local_port, external_ip).await?;
            Ok((transport, local_ip, advertised_ip, server_addr, proto))
        }
    }

    /// Just the connection-establishing steps (DNS resolution, socket bind,
    /// transport connect) for one concrete transport -- shared by the
    /// direct (non-`Auto`) path and each candidate `connect_transport_auto` tries.
    async fn connect_transport_concrete(
        account:     &SipAccount,
        proto:       TransportProtocol,
        local_port:  u16,
        external_ip: &Option<String>,
    ) -> anyhow::Result<(Arc<SipTransport>, String, String, SocketAddr)> {
        let (connect_host, connect_port) = account.connect_target();
        let local_ip = local_ip_for(&connect_host, connect_port)?;
        let advertised_ip = account
            .public_address
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .or_else(|| external_ip.clone())
            .unwrap_or_else(|| local_ip.clone());

        let server_addr = tokio::net::lookup_host(format!("{connect_host}:{connect_port}"))
            .await?
            .next()
            .ok_or_else(|| anyhow::anyhow!("DNS lookup failed for {connect_host}"))?;

        let bind_addr: SocketAddr = format!("0.0.0.0:{local_port}")
            .parse()
            .context("Invalid bind address")?;
        let transport = Arc::new(
            SipTransport::connect(
                proto,
                bind_addr,
                server_addr,
                &connect_host,
                account.tls_insecure_skip_verify,
            )
            .await?,
        );

        info!(
            local   = %format!("{local_ip}:{local_port}"),
            advertised = %advertised_ip,
            server  = %server_addr,
            transport = ?proto,
            "SIP stack ready"
        );

        Ok((transport, local_ip, advertised_ip, server_addr))
    }

    /// `TransportProtocol::Auto`: try UDP, then TCP, then TLS in that order,
    /// each bounded by `AUTO_PROBE_TIMEOUT` -- keeps the first candidate
    /// that both connects *and* gets an actual response (any status code)
    /// to a one-shot unauthenticated REGISTER probe, since UDP alone always
    /// "connects" (it's just a local bind) and so can't be told apart from
    /// a genuinely unreachable server without a real round-trip. Once one
    /// candidate succeeds, the rest of the stack treats the connection
    /// exactly as if that concrete transport had been configured directly
    /// (see `resolved_transport`).
    async fn connect_transport_auto(
        account:     &SipAccount,
        local_port:  u16,
        external_ip: &Option<String>,
    ) -> anyhow::Result<(Arc<SipTransport>, String, String, SocketAddr, TransportProtocol)> {
        const CANDIDATES: [TransportProtocol; 3] = [
            TransportProtocol::Udp,
            TransportProtocol::Tcp,
            TransportProtocol::Tls,
        ];
        let mut last_err: Option<anyhow::Error> = None;

        for proto in CANDIDATES {
            let connected =
                Self::connect_transport_concrete(account, proto, local_port, external_ip).await;
            let (transport, local_ip, advertised_ip, server_addr) = match connected {
                Ok(c) => c,
                Err(e) => {
                    debug!("Auto-transport: {proto:?} failed to connect ({e:#})");
                    last_err = Some(e);
                    continue;
                }
            };

            if probe_register(&transport, proto, account, &local_ip, &advertised_ip, local_port, server_addr).await {
                info!("Auto-transport: resolved to {proto:?}");
                return Ok((transport, local_ip, advertised_ip, server_addr, proto));
            }
            debug!("Auto-transport: {proto:?} connected but didn't respond to probe REGISTER");
        }

        Err(last_err.unwrap_or_else(|| {
            anyhow::anyhow!("Auto-transport: no candidate transport reached a live server")
        }))
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
        network:     NetworkConfig,
        local_port:  u16,
        external_ip: Option<String>,
    ) -> anyhow::Result<SipHandle> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (cmd_tx,   cmd_rx)   = mpsc::unbounded_channel();
        let stack = SipStack::new(account.clone(), network.clone(), local_port, external_ip.clone(), event_tx.clone(), cmd_rx)
            .await
            .map_err(|(e, _)| e)?;
        let advertised_ip = stack.advertised_ip.clone();
        let secure = stack.resolved_transport == TransportProtocol::Tls;
        let domain = stack.account.domain().to_string();

        tokio::spawn(async move {
            let mut stack: Option<SipStack> = Some(stack);
            let mut pending_cmd_rx: Option<CmdRx> = None;
            let mut retry_delay = Duration::from_secs(5);

            loop {
                if stack.is_none() {
                    let cmd_rx = pending_cmd_rx.take()
                        .expect("no live stack means a previous attempt stashed its cmd_rx");
                    match SipStack::new(account.clone(), network.clone(), local_port, external_ip.clone(), event_tx.clone(), cmd_rx).await {
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
        // NAT/firewall keepalive -- only ticks when the account has one
        // configured; `if keepalive_tick.is_some()` below guards the whole
        // branch, so an unset value just never sends anything (today's
        // behavior, unchanged).
        let mut keepalive_tick = self
            .account
            .keepalive_secs
            .filter(|&s| s > 0)
            .map(|s| interval(Duration::from_secs(s as u64)));

        loop {
            tokio::select! {
                _ = presence_tick.tick() => {
                    self.refresh_presence_subscriptions().await;
                    self.refresh_mwi_subscriptions().await;
                }
                _ = async { keepalive_tick.as_mut().unwrap().tick().await }, if keepalive_tick.is_some() => {
                    self.send_keepalive().await;
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
                            // Any call whose dialog was live at the moment the
                            // transport died is otherwise left to hang from the
                            // UI's perspective indefinitely (or until Asterisk's
                            // own retransmit timers eventually give up and send
                            // a BYE/CANCEL we happen to still be around to
                            // receive, which can take 20+ seconds) -- the
                            // in-memory dialog itself is gone the moment
                            // `spawn`'s reconnect loop rebuilds a fresh
                            // `SipStack`, so there's no way to recover it either
                            // way. Fail them immediately instead so the UI
                            // reflects reality right away.
                            for call_id in self.dialogs.keys().cloned().collect::<Vec<_>>() {
                                let _ = self.event_tx.send(SipEvent::CallFailed {
                                    call_id, code: 0, reason: "Connection lost".into(),
                                });
                            }
                            return Err((e, self.cmd_rx));
                        }
                    }
                }
                Some(cmd) = self.cmd_rx.recv() => {
                    self.handle_command(cmd).await;
                }
                Some(ev) = self.internal_rx.recv() => {
                    self.handle_stack_event(ev).await;
                }
            }
        }
    }

    /// Send a lone CRLF-CRLF datagram to the registrar to hold a NAT/
    /// firewall's outbound UDP binding (or TCP/TLS connection) open between
    /// registrations -- RFC 2617-style auth and dialog state don't apply to
    /// this, it's purely traffic to keep the path alive, so failures are
    /// logged and otherwise ignored (the next tick tries again regardless).
    async fn send_keepalive(&self) {
        if let Err(e) = self.transport.send(b"\r\n\r\n", self.server_addr).await {
            debug!("Keepalive send failed: {e:#}");
        }
    }

    /// Dispatch a completed background call-setup result -- see
    /// `StackEvent`'s doc comment. Handlers live in `call::lifecycle`
    /// alongside the rest of the call-establishment logic they finish.
    async fn handle_stack_event(&mut self, ev: StackEvent) {
        match ev {
            ev @ StackEvent::OutgoingOfferReady { .. }  => self.on_outgoing_offer_ready(ev).await,
            ev @ StackEvent::IncomingAnswerReady { .. } => self.on_incoming_answer_ready(ev).await,
            ev @ StackEvent::OutgoingConnected { .. }   => self.on_outgoing_connected(ev).await,
        }
    }

    pub(crate) fn via_proto(&self) -> &'static str {
        via_proto_str(self.resolved_transport)
    }

    /// `;transport=...` URI parameter for our own `Contact:` header — empty
    /// for UDP (the default the far end assumes with no parameter at all),
    /// explicit otherwise so a peer sending a fresh request back to us
    /// (e.g. an Asterisk-originated INVITE) knows to reuse/re-establish
    /// TCP/TLS rather than defaulting to UDP on our registered port, which
    /// silently goes nowhere since we never bind a UDP listener there.
    pub(crate) fn contact_transport_param(&self) -> &'static str {
        contact_transport_param_str(self.resolved_transport)
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
                    // A peer's own INFO (e.g. Asterisk echoing DTMF back once
                    // `dtmf_mode=info` is set) doesn't carry anything DeeLip
                    // needs to act on today, but RFC 6086 still expects a
                    // response -- leaving it unanswered just makes the sender
                    // retransmit it several times before giving up, which is
                    // exactly what was observed live (three "unhandled
                    // request" log lines for what was really 1-2 messages).
                    SipMethod::Info    => self.send_ok(&msg, from).await,
                    SipMethod::Message => self.on_message(msg, from).await,
                    _                  => debug!(?method, "Ignoring unhandled request"),
                }
            }
            SipStartLine::Response { status, .. } => {
                self.on_response(msg, status, from).await;
            }
        }
    }

    // ── Command handler ───────────────────────────────────────────────────────

    async fn handle_command(&mut self, cmd: SipCommand) {
        match cmd {
            SipCommand::MakeCall   { to, attempt_ice }   => self.initiate_call(&to, attempt_ice).await,
            SipCommand::AcceptCall { call_id }             => self.accept_call(&call_id).await,
            SipCommand::RejectCall { call_id }             => self.reject_call(&call_id).await,
            SipCommand::HangUp     { call_id }             => self.hang_up(&call_id).await,
            SipCommand::HoldCall   { call_id }             => self.send_reinvite(&call_id, true).await,
            SipCommand::ResumeCall { call_id }             => self.send_reinvite(&call_id, false).await,
            SipCommand::BlindTransfer { call_id, target }  => self.blind_transfer(&call_id, &target).await,
            SipCommand::RedirectCall  { call_id, target }  => self.redirect_call(&call_id, &target).await,
            SipCommand::SubscribePresence   { target_uri } => self.subscribe_presence(&target_uri).await,
            SipCommand::UnsubscribePresence { target_uri } => self.unsubscribe_presence(&target_uri).await,
            SipCommand::AttendedTransfer { call_id, consultation_call_id } =>
                self.attended_transfer(&call_id, &consultation_call_id).await,
            SipCommand::SendDtmfInfo { call_id, digit } => self.send_dtmf_info(&call_id, digit).await,
            SipCommand::SubscribeMwi { target_uri } => self.subscribe_mwi(&target_uri).await,
            SipCommand::SendMessage { to, body } => self.send_message(&to, &body).await,
        }
    }

    // ── Shared response helpers ────────────────────────────────────────────────

    pub(crate) async fn send_ok(&self, req: &SipMessage, from: SocketAddr) {
        let ok = self.build_response(req, 200, "OK", "", "");
        let _ = self.transport.send(ok.as_bytes(), from).await;
    }

    pub(crate) fn build_response(&self, req: &SipMessage, code: u16, phrase: &str, to_tag: &str, body: &str) -> String {
        self.build_response_with_body(req, code, phrase, to_tag, body)
    }

    pub(crate) fn build_response_with_body(&self, req: &SipMessage, code: u16, phrase: &str, to_tag: &str, body: &str) -> String {
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

    pub(crate) fn build_ack(
        &self,
        call_id:  &str,
        from_tag: &str,
        to_uri:   &str,
        to_tag:   Option<&str>,
        cseq:     u32,
        branch:   &str,
    ) -> String {
        let server      = self.account.domain();
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
}

fn via_proto_str(proto: TransportProtocol) -> &'static str {
    match proto {
        TransportProtocol::Udp => "UDP",
        TransportProtocol::Tcp => "TCP",
        TransportProtocol::Tls => "TLS",
        TransportProtocol::Auto => unreachable!("resolved_transport is never Auto"),
    }
}

fn contact_transport_param_str(proto: TransportProtocol) -> &'static str {
    match proto {
        TransportProtocol::Udp => "",
        TransportProtocol::Tcp => ";transport=tcp",
        TransportProtocol::Tls => ";transport=tls",
        TransportProtocol::Auto => unreachable!("resolved_transport is never Auto"),
    }
}

const AUTO_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// One-shot, unauthenticated `REGISTER` (`Expires: 0`, so a compliant
/// registrar treats it as a no-op rather than actually registering)
/// used only to test whether a just-connected transport candidate in
/// `connect_transport_auto` actually reaches a live server -- any response
/// at all (2xx, 401/407 challenge, even a rejection) proves the path
/// works; a timeout with nothing back means it doesn't. Deliberately
/// free-standing rather than `self.register_once()`/`send_register()` in
/// `registration.rs`, since those need a fully-constructed `SipStack`
/// (`reg_call_id`/`reg_from_tag`/`reg_cseq` etc.) that doesn't exist yet
/// this early in connection setup.
async fn probe_register(
    transport:     &SipTransport,
    proto:         TransportProtocol,
    account:       &SipAccount,
    local_ip:      &str,
    advertised_ip: &str,
    local_port:    u16,
    server_addr:   SocketAddr,
) -> bool {
    let call_id = crate::wire::util::new_call_id(local_ip);
    let branch = crate::wire::util::new_branch();
    let from_tag = crate::wire::util::new_tag();
    let username = &account.username;
    let server = account.domain();
    let display = account.display_name.as_deref().unwrap_or(username);
    let via_proto = via_proto_str(proto);
    let contact_transport = contact_transport_param_str(proto);

    let msg = format!(
        "REGISTER sip:{server} SIP/2.0\r\n\
         Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
         Max-Forwards: 70\r\n\
         To: \"{display}\" <sip:{username}@{server}>\r\n\
         From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 REGISTER\r\n\
         Contact: <sip:{username}@{advertised_ip}:{local_port}{contact_transport}>\r\n\
         Expires: 0\r\n\
         User-Agent: DeeLip/0.1.0\r\n\
         Content-Length: 0\r\n\r\n"
    );
    if transport.send(msg.as_bytes(), server_addr).await.is_err() {
        return false;
    }

    let deadline = Instant::now() + AUTO_PROBE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        let Ok(Ok((data, _from))) = tokio::time::timeout(remaining, transport.recv()).await else {
            return false;
        };
        if let Some(resp) = SipMessage::parse(&data) {
            if resp.call_id().is_some_and(|id| id == call_id) && resp.status_code().is_some() {
                return true;
            }
        }
        // Unrelated datagram (retransmit noise, another in-flight
        // transaction) -- keep waiting until the deadline.
    }
}
