//! Full ICE (RFC 8445) via the `webrtc-ice` crate — same `webrtc-rs` family
//! (and exact version, 0.17.x) as `turn`/`webrtc-util` already used by
//! `turn_relay.rs`. This is additive to, not a replacement for, the plain
//! STUN-reflexive/TURN-unconditional path in `stun.rs`/`turn_relay.rs`: a
//! call falls back to that existing path if gathering fails or times out
//! (see `crates/ui/src/lib.rs`'s `try_gather_ice`).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use tokio::sync::mpsc;
use webrtc_ice::agent::agent_config::AgentConfig;
use webrtc_ice::agent::Agent;
use webrtc_ice::candidate::candidate_base::unmarshal_candidate;
use webrtc_ice::candidate::Candidate;
use webrtc_ice::network_type::NetworkType;
use webrtc_ice::rand::{generate_pwd, generate_ufrag};
use webrtc_ice::url::{ProtoType, SchemeType, Url};
use webrtc_util::{Conn, Error as UtilError};

/// The `Conn` returned by ICE connectivity checks (`AgentConn`, private to
/// `webrtc-ice`) only implements the "connected socket" half of `Conn` --
/// `send`/`recv`, always talking to whichever candidate pair won.
/// `send_to`/`recv_from` on `AgentConn` unconditionally return "Not
/// applicable", since there's no per-call destination argument once a pair
/// is selected. But `MediaEngine`'s `RtpSocket` abstraction (see
/// `crates/media-engine/src/engine.rs`) is built around `send_to`/`recv_from`
/// (shared with the TURN relay `Conn`, which *does* implement them properly)
/// -- this adapter bridges the gap by delegating to `send`/`recv`+
/// `remote_addr()`, so an ICE-selected `Conn` can be handed to
/// `MediaEngine::start`'s `relay` parameter completely unchanged.
struct ConnectedConn(Arc<dyn Conn + Send + Sync>);

#[async_trait]
impl Conn for ConnectedConn {
    async fn connect(&self, addr: SocketAddr) -> Result<(), UtilError> {
        self.0.connect(addr).await
    }
    async fn recv(&self, buf: &mut [u8]) -> Result<usize, UtilError> {
        self.0.recv(buf).await
    }
    async fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr), UtilError> {
        let n = self.0.recv(buf).await?;
        let addr = self.0.remote_addr()
            .ok_or_else(|| UtilError::Other("ICE: no candidate pair selected yet".into()))?;
        Ok((n, addr))
    }
    async fn send(&self, buf: &[u8]) -> Result<usize, UtilError> {
        self.0.send(buf).await
    }
    async fn send_to(&self, buf: &[u8], _target: SocketAddr) -> Result<usize, UtilError> {
        // `_target` is ignored -- ICE always sends to the selected pair
        // regardless of any address the caller thinks it's sending to,
        // which is always correct here since this Conn only ever serves
        // one call's single remote party.
        self.0.send(buf).await
    }
    fn local_addr(&self) -> Result<SocketAddr, UtilError> {
        self.0.local_addr()
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        self.0.remote_addr()
    }
    async fn close(&self) -> Result<(), UtilError> {
        self.0.close().await
    }
    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) {
        self
    }
}

/// Local ICE candidates gathered for one call leg, ready to embed in an SDP
/// offer/answer (see `deelip_sip::sdp::IceAttrs`) and later hand to
/// `connect()` once the remote's ICE parameters are known.
pub struct IceGathered {
    agent: Arc<Agent>,
    pub local_ufrag: String,
    pub local_pwd: String,
    /// `Candidate::marshal()`'d strings, ready to prefix with `a=candidate:`.
    pub candidates: Vec<String>,
    /// Best-priority candidate's address — used for the plain `c=`/`m=`
    /// line so a peer that ignores ICE entirely still gets a working address.
    pub default_addr: SocketAddr,
}

fn split_host_port(s: &str, default_port: u16) -> anyhow::Result<(String, u16)> {
    match s.rsplit_once(':') {
        Some((host, port)) => Ok((host.to_string(), port.parse().context("Parsing port")?)),
        None => Ok((s.to_string(), default_port)),
    }
}

/// Gather local ICE candidates (host + server-reflexive via `stun_server` +
/// relay via `turn`, if given). Bounded by `timeout` — the crate's own
/// internal STUN-gather timeout is 5s per candidate type, so this outer
/// bound should be a little more generous than that when a TURN server is
/// also configured (both gather concurrently, but worth headroom).
pub async fn gather(
    stun_server: Option<&str>,
    turn: Option<(&str, &str, &str)>, // (host:port, username, password)
    is_controlling: bool,
    timeout: Duration,
) -> anyhow::Result<IceGathered> {
    let mut urls = Vec::new();
    if let Some(s) = stun_server {
        let (host, port) = split_host_port(s, 3478)?;
        urls.push(Url { scheme: SchemeType::Stun, host, port, username: String::new(), password: String::new(), proto: ProtoType::Udp });
    }
    if let Some((addr, username, password)) = turn {
        let (host, port) = split_host_port(addr, 3478)?;
        urls.push(Url {
            scheme: SchemeType::Turn, host, port,
            username: username.to_string(), password: password.to_string(),
            proto: ProtoType::Udp,
        });
    }
    if urls.is_empty() {
        anyhow::bail!("No STUN/TURN server configured -- nothing for ICE to gather beyond host candidates alone");
    }

    let local_ufrag = generate_ufrag();
    let local_pwd = generate_pwd();

    let config = AgentConfig {
        urls,
        network_types: vec![NetworkType::Udp4],
        is_controlling,
        local_ufrag: local_ufrag.clone(),
        local_pwd: local_pwd.clone(),
        ..Default::default()
    };
    let agent = Arc::new(Agent::new(config).await.context("Creating ICE agent")?);

    let (cand_tx, mut cand_rx) = mpsc::unbounded_channel();
    agent.on_candidate(Box::new(move |c| {
        let cand_tx = cand_tx.clone();
        Box::pin(async move {
            let _ = cand_tx.send(c);
        })
    }));
    agent.gather_candidates().context("Starting ICE candidate gathering")?;

    let gather_all = async {
        let mut candidates = Vec::new();
        while let Some(maybe_candidate) = cand_rx.recv().await {
            match maybe_candidate {
                Some(c) => candidates.push(c),
                None => break, // gathering complete signal
            }
        }
        candidates
    };
    let candidates = tokio::time::timeout(timeout, gather_all)
        .await
        .context("ICE candidate gathering timed out")?;

    let best = candidates.iter().max_by_key(|c| c.priority())
        .ok_or_else(|| anyhow::anyhow!("ICE gathering produced no candidates"))?;
    let default_addr = best.addr();
    let marshaled: Vec<String> = candidates.iter().map(|c| c.marshal()).collect();

    Ok(IceGathered { agent, local_ufrag, local_pwd, candidates: marshaled, default_addr })
}

/// The winning `Conn` from a completed ICE connectivity check, plus the
/// `Agent` that produced it. The `Agent` is kept alive alongside the `Conn`
/// deliberately -- `AgentConn`'s own docs don't guarantee it keeps working
/// independent of its parent `Agent`'s lifetime, so rather than assume
/// independence, both are held together for as long as the call's media is
/// active (mirrors `TurnRelay` keeping its `turn::client::Client` alive
/// alongside its `conn` for the same reason).
pub struct IceConnection {
    pub _agent: Arc<Agent>,
    pub conn: Arc<dyn Conn + Send + Sync>,
}

/// Feed the remote's ICE parameters (parsed from their SDP) into the agent
/// and run connectivity checks, returning the winning connection — its
/// `conn` field is a drop-in replacement for `MediaEngine::start`'s `relay`
/// parameter, exactly like `TurnRelay::conn` already is (both are
/// `webrtc_util::Conn` trait objects).
pub async fn connect(
    gathered: IceGathered,
    is_controlling: bool,
    remote_ufrag: &str,
    remote_pwd: &str,
    remote_candidates: &[String],
) -> anyhow::Result<IceConnection> {
    for raw in remote_candidates {
        let candidate = unmarshal_candidate(raw).context("Parsing remote ICE candidate")?;
        let candidate: Arc<dyn Candidate + Send + Sync> = Arc::new(candidate);
        gathered.agent.add_remote_candidate(&candidate).context("Adding remote ICE candidate")?;
    }

    // Never actually cancelled -- kept alive for the duration of the call so
    // `cancel_rx.recv()` doesn't resolve immediately (a closed channel's recv
    // returns `None` right away, which `dial`/`accept` would treat as a
    // cancel request).
    let (_cancel_tx, cancel_rx) = mpsc::channel::<()>(1);

    let raw_conn: Arc<dyn Conn + Send + Sync> = if is_controlling {
        gathered.agent.dial(cancel_rx, remote_ufrag.to_string(), remote_pwd.to_string())
            .await.context("ICE connectivity checks failed (dial)")?
    } else {
        gathered.agent.accept(cancel_rx, remote_ufrag.to_string(), remote_pwd.to_string())
            .await.context("ICE connectivity checks failed (accept)")?
    };
    let conn: Arc<dyn Conn + Send + Sync> = Arc::new(ConnectedConn(raw_conn));
    Ok(IceConnection { _agent: gathered.agent, conn })
}
