//! Transport connection establishment: `SipStack::new`'s connect step (direct
//! or `TransportProtocol::Auto` probing), plus `spawn`'s reconnect-with-backoff
//! loop.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

use anyhow::Context;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::{debug, error, info};

use deelip_config::{SipAccount, TransportProtocol};

use super::builders::{contact_transport_param_str, via_proto_str};
use super::events::EventSender;
use super::{CmdRx, MAX_RETRY, SipStack};
use crate::call::media_setup::NetworkConfig;
use crate::events::SipEvent;
use crate::handle::SipHandle;
use crate::transport::SipTransport;
use crate::wire::message::SipMessage;
use crate::wire::util::local_ip_for;

impl SipStack {
    pub async fn new(
        account: SipAccount, network: NetworkConfig, local_port: u16, external_ip: Option<String>,
        event_tx: EventSender, cmd_rx: CmdRx,
    ) -> Result<Self, (anyhow::Error, CmdRx)> {
        let (transport, local_ip, advertised_ip, server_addr, resolved_transport) =
            match Self::connect_transport(&account, &network, local_port, &external_ip).await {
                Ok(c) => c,
                Err(e) => return Err((e, cmd_rx)),
            };

        let reg_call_id = crate::wire::util::new_call_id(&local_ip);
        let reg_from_tag = crate::wire::util::new_tag();
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();
        let identity_host =
            if account.domain().is_empty() { format!("{local_ip}:{local_port}") } else { account.domain().to_string() };

        Ok(Self {
            transport,
            account,
            network,
            local_ip,
            advertised_ip,
            local_port,
            server_addr,
            identity_host,
            resolved_transport,
            reg_call_id,
            reg_from_tag,
            reg_cseq: Arc::new(AtomicU32::new(1)),
            dialogs: HashMap::new(),
            subscriptions: HashMap::new(),
            mwi_subscriptions: HashMap::new(),
            presence_publish: None,
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
        account: &SipAccount, network: &NetworkConfig, local_port: u16, external_ip: &Option<String>,
    ) -> anyhow::Result<(Arc<SipTransport>, String, String, SocketAddr, TransportProtocol)> {
        if account.local_account {
            Self::connect_local(account, local_port, external_ip).await
        } else if account.transport == TransportProtocol::Auto {
            Self::connect_transport_auto(account, network, local_port, external_ip).await
        } else {
            let proto = account.transport;
            let (transport, local_ip, advertised_ip, server_addr) =
                Self::connect_transport_concrete(account, network, proto, local_port, external_ip).await?;
            Ok((transport, local_ip, advertised_ip, server_addr, proto))
        }
    }

    /// `SipAccount::local_account` (MicroSIP's "Local Account"/serverless
    /// mode) -- see docs/crates/sip-core.md's "SipAccount::local_account" section.
    async fn connect_local(
        account: &SipAccount, local_port: u16, external_ip: &Option<String>,
    ) -> anyhow::Result<(Arc<SipTransport>, String, String, SocketAddr, TransportProtocol)> {
        // Ask the OS routing table which local IP it would use to reach the
        // public internet -- purely a local routing-table lookup (a UDP
        // `connect()` sends no packet), used here only as a stand-in for
        // "this machine's own outbound-facing address" since there's no
        // registrar to ask instead.
        let local_ip = local_ip_for("8.8.8.8", 80).unwrap_or_else(|_| "127.0.0.1".to_string());
        let advertised_ip = account
            .public_address
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .or_else(|| external_ip.clone())
            .unwrap_or_else(|| local_ip.clone());

        let bind_addr: SocketAddr = format!("0.0.0.0:{local_port}").parse().context("Invalid bind address")?;
        let server_addr: SocketAddr = "0.0.0.0:0".parse().unwrap();
        let transport =
            Arc::new(SipTransport::connect(TransportProtocol::Udp, bind_addr, server_addr, "", false).await?);

        info!(
            local = %format!("{local_ip}:{local_port}"),
            advertised = %advertised_ip,
            "Local Account (serverless) SIP stack ready"
        );

        Ok((transport, local_ip, advertised_ip, server_addr, TransportProtocol::Udp))
    }

    /// Just the connection-establishing steps (DNS resolution, socket bind,
    /// transport connect) for one concrete transport -- shared by the
    /// direct (non-`Auto`) path and each candidate `connect_transport_auto` tries.
    async fn connect_transport_concrete(
        account: &SipAccount, network: &NetworkConfig, proto: TransportProtocol, local_port: u16,
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

        let server_addr = crate::wire::dns::resolve_target(
            &connect_host,
            connect_port,
            proto,
            network.custom_nameserver.as_deref(),
            network.dns_srv_enabled,
        )
        .await?;

        let bind_addr: SocketAddr = format!("0.0.0.0:{local_port}").parse().context("Invalid bind address")?;
        let transport = Arc::new(
            SipTransport::connect(proto, bind_addr, server_addr, &connect_host, account.tls_insecure_skip_verify)
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

    /// `TransportProtocol::Auto`: try UDP, then TCP, then TLS, each bounded
    /// by `AUTO_PROBE_TIMEOUT`. See docs/crates/sip-core.md's "connect_transport_auto"
    /// section for why a probe REGISTER is needed rather than just connecting.
    async fn connect_transport_auto(
        account: &SipAccount, network: &NetworkConfig, local_port: u16, external_ip: &Option<String>,
    ) -> anyhow::Result<(Arc<SipTransport>, String, String, SocketAddr, TransportProtocol)> {
        const CANDIDATES: [TransportProtocol; 3] =
            [TransportProtocol::Udp, TransportProtocol::Tcp, TransportProtocol::Tls];
        let mut last_err: Option<anyhow::Error> = None;

        for proto in CANDIDATES {
            let connected = Self::connect_transport_concrete(account, network, proto, local_port, external_ip).await;
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

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Auto-transport: no candidate transport reached a live server")))
    }

    /// Spawns the background task that runs this account's SIP stack for
    /// the lifetime of the process, reconnecting with exponential backoff on
    /// transport failure. See docs/crates/sip-core.md's "SipStack::spawn's reconnect
    /// loop" section.
    pub async fn spawn(
        account: SipAccount, network: NetworkConfig, local_port: u16, external_ip: Option<String>,
        waker: Arc<dyn Fn() + Send + Sync>,
    ) -> anyhow::Result<SipHandle> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let event_tx = EventSender::new(event_tx, waker);
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let stack =
            SipStack::new(account.clone(), network.clone(), local_port, external_ip.clone(), event_tx.clone(), cmd_rx)
                .await
                .map_err(|(e, _)| e)?;
        let advertised_ip = stack.advertised_ip.clone();
        let secure = stack.resolved_transport == TransportProtocol::Tls;
        let domain = stack.identity_host.clone();

        tokio::spawn(async move {
            let mut stack: Option<SipStack> = Some(stack);
            let mut pending_cmd_rx: Option<CmdRx> = None;
            let mut retry_delay = Duration::from_secs(5);

            loop {
                if stack.is_none() {
                    let cmd_rx =
                        pending_cmd_rx.take().expect("no live stack means a previous attempt stashed its cmd_rx");
                    match SipStack::new(
                        account.clone(),
                        network.clone(),
                        local_port,
                        external_ip.clone(),
                        event_tx.clone(),
                        cmd_rx,
                    )
                    .await
                    {
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
                        let _ = event_tx.send(SipEvent::RegistrationFailed { reason: format!("Disconnected: {e:#}") });
                        pending_cmd_rx = Some(cmd_rx);
                        tokio::time::sleep(retry_delay).await;
                        retry_delay = (retry_delay * 2).min(MAX_RETRY);
                    }
                }
            }
        });
        Ok(SipHandle { event_rx, cmd_tx, advertised_ip, secure, domain })
    }
}

const AUTO_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// One-shot, unauthenticated `REGISTER` used only to test whether a
/// just-connected transport candidate in `connect_transport_auto` actually
/// reaches a live server. Free-standing rather than `self.register_once()`
/// since a fully-constructed `SipStack` doesn't exist yet this early in
/// connection setup.
async fn probe_register(
    transport: &SipTransport, proto: TransportProtocol, account: &SipAccount, local_ip: &str, advertised_ip: &str,
    local_port: u16, server_addr: SocketAddr,
) -> bool {
    let call_id = crate::wire::util::new_call_id(local_ip);
    let branch = crate::wire::util::new_branch();
    let from_tag = crate::wire::util::new_tag();
    let username = &account.username;
    let server = account.domain();
    let display = account.display_name.as_deref().unwrap_or(username);
    let via_proto = via_proto_str(proto);
    let contact_transport = contact_transport_param_str(proto);

    let user_agent = crate::USER_AGENT;
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
         User-Agent: {user_agent}\r\n\
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
        if let Some(resp) = SipMessage::parse(&data)
            && resp.call_id().is_some_and(|id| id == call_id)
            && resp.status_code().is_some()
        {
            return true;
        }
        // Unrelated datagram (retransmit noise, another in-flight
        // transaction) -- keep waiting until the deadline.
    }
}
