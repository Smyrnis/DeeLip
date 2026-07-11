//! TURN relay client (RFC 5766) — an explicit, user-configured fallback for
//! media when direct/STUN connectivity can't traverse the NAT. No ICE: this
//! just allocates one relayed transport address and hands back a `Conn` that
//! behaves like a normal socket (send_to/recv_from), used unconditionally for
//! every call when a TURN server is configured (see `AppConfig::turn_server`).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use webrtc_util::Conn;

/// A live TURN allocation. Keeps the `Client` alive for the allocation's
/// lifetime (it manages the allocation refresh internally); dropping this
/// releases the relay.
pub struct TurnRelay {
    pub relayed_addr: SocketAddr,
    pub conn: Arc<dyn Conn + Send + Sync>,
    _client: turn::client::Client,
}

/// Allocate a relayed transport address on `turn_server` (e.g. "turn.example.com:3478")
/// using long-term credentials. The returned `TurnRelay::conn` is a drop-in
/// alternative to a raw UDP socket — `send_to`/`recv_from` handle TURN framing
/// and peer permissions internally.
pub async fn allocate_relay(turn_server: &str, username: &str, password: &str) -> anyhow::Result<TurnRelay> {
    let local_socket: Arc<dyn Conn + Send + Sync> = Arc::new(
        tokio::net::UdpSocket::bind("0.0.0.0:0").await.context("Binding local socket for TURN control channel")?,
    );

    let config = turn::client::ClientConfig {
        stun_serv_addr: turn_server.to_string(),
        turn_serv_addr: turn_server.to_string(),
        username: username.to_string(),
        password: password.to_string(),
        realm: String::new(),
        software: String::new(),
        rto_in_ms: 0,
        conn: local_socket,
        vnet: None,
    };

    let client = turn::client::Client::new(config).await.context("Creating TURN client")?;
    client.listen().await.context("Starting TURN client listener")?;

    let relay_conn: Arc<dyn Conn + Send + Sync> =
        Arc::new(client.allocate().await.context("TURN Allocate request failed")?);
    let relayed_addr = relay_conn.local_addr().context("Reading relayed address")?;

    tracing::info!("TURN allocated relay address {relayed_addr}");

    Ok(TurnRelay { relayed_addr, conn: relay_conn, _client: client })
}
