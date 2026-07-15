//! SDP construction combined with STUN/TURN/ICE endpoint resolution for one
//! call leg. Full picture: docs/crates/sip-core.md's "call::media_setup" section.

use std::net::SocketAddr;
use std::sync::Arc;

use deelip_config::SipAccount;
use deelip_config::timeouts::ICE_GATHER_TIMEOUT;
use deelip_nat::{IceConnection, IceGathered, TurnRelay};
use webrtc_util::Conn;

use crate::call::dialog::{CallMedia, VideoMedia};
use crate::events::{CallMediaReady, VideoMediaReady};
use crate::wire::sdp::{
    ALL_CODECS, AudioCodec, DtlsFingerprint, IceAttrs, ParsedSdp, Setup, SrtpParams, SrtpSession, VideoCodec,
    generate_dtls_cert,
};

/// Everything one call's DTLS-SRTP session (RFC 5763/5764) needs -- decided
/// once and reused for both the audio and (if negotiated) video `m=`
/// sections, since this codebase negotiates DTLS-SRTP at the call level,
/// never per media section (see `MediaEncryption::DtlsSrtp`'s doc comment).
/// Rides alongside `local_srtp`/`remote_srtp` through `CallMedia`/
/// `CallMediaReady` without interacting with `resolve_srtp_and_relay`'s
/// SDES/TURN/ICE resolution at all.
#[derive(Debug, Clone)]
pub struct DtlsCallParams {
    pub cert_der: Vec<u8>,
    pub private_key_der: Vec<u8>,
    pub local_fingerprint: DtlsFingerprint,
    /// Resolved once the offer/answer exchange completes -- `Active` (we
    /// send the DTLS ClientHello) or `Passive` (we wait for the peer's).
    /// `None` only transiently for the offerer, between generating our own
    /// cert/fingerprint (sent as `a=setup:actpass`) and the answer arriving;
    /// the answerer resolves this immediately upon seeing the offer.
    pub role: Option<Setup>,
    /// The remote's advertised fingerprint -- compared against the DTLS
    /// handshake's actual peer certificate once it completes
    /// (`deelip_media::dtls_srtp_session`). This is the real
    /// MITM-prevention check; must never be skipped.
    pub remote_fingerprint: Option<DtlsFingerprint>,
}

impl DtlsCallParams {
    /// Generates a fresh cert/fingerprint for a new call, with `role`/
    /// `remote_fingerprint` still unresolved (see their doc comments).
    pub fn generate() -> anyhow::Result<Self> {
        let (cert_der, private_key_der, local_fingerprint) = generate_dtls_cert()?;
        Ok(Self { cert_der, private_key_der, local_fingerprint, role: None, remote_fingerprint: None })
    }
}

/// Network settings a `SipStack` needs for call setup -- separate from
/// `SipAccount` since STUN/TURN server config is process-wide (Settings'
/// Network section), not per-account. Passed in once at `SipStack::spawn`
/// time; like every other Network setting in this app, changing it requires
/// a restart.
#[derive(Debug, Clone, Default)]
pub struct NetworkConfig {
    pub stun_server: Option<String>,
    pub turn_server: Option<String>,
    pub turn_username: String,
    pub turn_password: String,
    /// Global default for whether to attempt ICE -- `SipAccount::ice_enabled`
    /// can override this per account (see `SipAccount::wants_ice`), so
    /// `try_gather_ice`/`try_answer_with_ice` take the already-resolved
    /// decision as an explicit `enabled` parameter rather than reading this
    /// field directly.
    pub ice_enabled: bool,
    /// Restrict local RTP port allocation (`deelip_nat::alloc_rtp_port`) to
    /// this inclusive range instead of an OS-assigned ephemeral port --
    /// lets a firewall/NAT port-forward be pinned to a fixed range covering
    /// every call. `None`: today's ephemeral-port behavior.
    pub rtp_port_range: Option<(u16, u16)>,
    /// Override DNS server ("ip" or "ip:port", default port 53) used to
    /// resolve the SIP server host and, if `dns_srv_enabled`, SRV records --
    /// see `crate::wire::dns`. `None`: system resolver, either directly
    /// (`tokio::net::lookup_host`) or via `/etc/resolv.conf` if SRV lookup
    /// is enabled.
    pub custom_nameserver: Option<String>,
    /// Attempt a SIP SRV-record lookup (RFC 3263) for the configured server
    /// host before falling back to a plain A/AAAA lookup on host:port --
    /// see `crate::wire::dns::resolve_target`. Off by default, matching
    /// today's behavior.
    pub dns_srv_enabled: bool,
}

impl NetworkConfig {
    fn turn(&self) -> Option<(String, String, String)> {
        self.turn_server.clone().map(|s| (s, self.turn_username.clone(), self.turn_password.clone()))
    }
}

/// This account's enabled codecs in preference order, ready to hand to
/// `build_offer`/`parse_sdp`. Falls back to every known codec if the
/// configured list is empty or entirely unrecognized -- the Settings UI
/// itself refuses to let the last enabled codec be disabled, so this should
/// be unreachable in practice.
pub fn account_codecs(acc: &SipAccount) -> Vec<AudioCodec> {
    let codecs: Vec<AudioCodec> = acc.codec_order.iter().filter_map(|s| codec_from_str(s)).collect();
    if codecs.is_empty() { ALL_CODECS.to_vec() } else { codecs }
}

/// Parse one of `codec_order`'s canonical lowercase codec names (also used
/// by `SipAccount::force_incoming_codec`).
pub fn codec_from_str(s: &str) -> Option<AudioCodec> {
    match s {
        "opus" => Some(AudioCodec::Opus),
        "g722" => Some(AudioCodec::G722),
        "pcmu" => Some(AudioCodec::Pcmu),
        "pcma" => Some(AudioCodec::Pcma),
        "gsm" => Some(AudioCodec::Gsm),
        "ilbc" => Some(AudioCodec::Ilbc),
        _ => None,
    }
}

/// Resolve the (ip, port) to advertise in an SDP offer/answer, using
/// `advertised_ip` as the direct-path fallback. Allocates a TURN relay on
/// first use if one is configured, storing it into `relay` for reuse across
/// hold/resume within the same call (a held `TurnRelay` keeps refreshing its
/// allocation internally until dropped).
pub async fn resolve_rtp_endpoint(
    network: &NetworkConfig, advertised_ip: &str, local_rtp: u16, relay: &mut Option<TurnRelay>,
) -> (String, u16) {
    if relay.is_none()
        && let Some((server, username, password)) = network.turn()
    {
        match deelip_nat::allocate_relay(&server, &username, &password).await {
            Ok(r) => *relay = Some(r),
            Err(e) => tracing::warn!("TURN allocation failed ({e}), falling back to direct"),
        }
    }
    match relay {
        Some(r) => (r.relayed_addr.ip().to_string(), r.relayed_addr.port()),
        None => (advertised_ip.to_string(), local_rtp),
    }
}

/// Attempt to gather local ICE candidates for a new call, bounded by
/// `ICE_GATHER_TIMEOUT` -- returns `None` (never an error the caller must
/// handle) if ICE is disabled, no STUN/TURN server is configured, or
/// gathering fails/times out, so every call site can fall back to the
/// existing `resolve_rtp_endpoint` path exactly as if ICE didn't exist.
/// `enabled` is the caller's already-resolved decision (global
/// `AppConfig::ice_enabled` combined with any per-account override via
/// `SipAccount::wants_ice`) -- this function itself only knows about
/// process-wide STUN/TURN config, not any particular account.
pub async fn try_gather_ice(network: &NetworkConfig, enabled: bool, is_controlling: bool) -> Option<IceGathered> {
    if !enabled {
        return None;
    }
    if network.stun_server.is_none() && network.turn_server.is_none() {
        return None;
    }
    let turn = network.turn();
    let turn_ref = turn.as_ref().map(|(s, u, p)| (s.as_str(), u.as_str(), p.as_str()));
    match deelip_nat::ice::gather(network.stun_server.as_deref(), turn_ref, is_controlling, ICE_GATHER_TIMEOUT).await {
        Ok(gathered) => Some(gathered),
        Err(e) => {
            tracing::warn!("ICE candidate gathering failed/timed out, falling back: {e}");
            None
        }
    }
}

/// Finish an in-progress ICE negotiation once the remote's parsed SDP is
/// known, running connectivity checks and returning the winning connection.
/// `None` if ICE wasn't attempted or connectivity checks fail -- by this
/// point our own offer/answer already committed the far end to our gathered
/// candidate's address, so a failure here has no clean fallback (an
/// inherent limit of ICE's gather-then-commit shape, not a bug); the call is
/// simply left without working media in that case.
pub async fn finish_ice_connect(
    gathered: Option<IceGathered>, is_controlling: bool, parsed: &ParsedSdp,
) -> Option<IceConnection> {
    finish_ice_connect_raw(
        gathered,
        is_controlling,
        parsed.ice_ufrag.as_deref(),
        parsed.ice_pwd.as_deref(),
        &parsed.ice_candidates,
    )
    .await
}

/// Same as `finish_ice_connect`, but takes the three ICE parameters
/// directly instead of a whole `ParsedSdp` -- used for the video leg's
/// negotiation, whose parsed offer/answer is a `ParsedVideoMedia` (a
/// distinct type from `ParsedSdp`, deliberately -- see `wire::sdp`'s
/// module doc). `finish_ice_connect` is now a thin wrapper around this.
pub async fn finish_ice_connect_raw(
    gathered: Option<IceGathered>, is_controlling: bool, ufrag: Option<&str>, pwd: Option<&str>, candidates: &[String],
) -> Option<IceConnection> {
    let gathered = gathered?;
    let ufrag = ufrag?;
    let pwd = pwd?;
    if candidates.is_empty() {
        return None;
    }
    match deelip_nat::ice::connect(gathered, is_controlling, ufrag, pwd, candidates).await {
        Ok(conn) => Some(conn),
        Err(e) => {
            tracing::warn!("ICE connectivity checks failed, falling back: {e}");
            None
        }
    }
}

/// For the answerer side: if the incoming `offer` signaled ICE support (has
/// ufrag/pwd/candidates) and ICE is enabled locally, gather our own
/// candidates and run connectivity checks immediately -- unlike the offerer
/// side (`finish_ice_connect`), the answerer already knows the remote's ICE
/// parameters up front, straight from the offer, so there's no need to wait
/// for a later event. Returns our own `IceAttrs` for the answer SDP, our
/// default candidate's address for the plain c=/m= line, and the winning
/// connection -- all three or nothing.
pub async fn try_answer_with_ice(
    network: &NetworkConfig, enabled: bool, offer: &ParsedSdp,
) -> Option<(IceAttrs, SocketAddr, IceConnection)> {
    try_answer_with_ice_raw(
        network,
        enabled,
        offer.ice_ufrag.as_deref(),
        offer.ice_pwd.as_deref(),
        &offer.ice_candidates,
    )
    .await
}

/// Same as `try_answer_with_ice`, but takes the three ICE parameters
/// directly instead of a whole `ParsedSdp` -- same reasoning as
/// `finish_ice_connect_raw`. `try_answer_with_ice` is now a thin wrapper
/// around this.
pub async fn try_answer_with_ice_raw(
    network: &NetworkConfig, enabled: bool, ufrag: Option<&str>, pwd: Option<&str>, candidates: &[String],
) -> Option<(IceAttrs, SocketAddr, IceConnection)> {
    if !enabled {
        return None;
    }
    let ufrag = ufrag?;
    let pwd = pwd?;
    if candidates.is_empty() {
        return None;
    }
    let gathered = try_gather_ice(network, enabled, false).await?;
    let attrs = IceAttrs {
        ufrag: gathered.local_ufrag.clone(),
        pwd: gathered.local_pwd.clone(),
        candidates: gathered.candidates.clone(),
    };
    let default_addr = gathered.default_addr;
    match deelip_nat::ice::connect(gathered, false, ufrag, pwd, candidates).await {
        Ok(conn) => Some((attrs, default_addr, conn)),
        Err(e) => {
            tracing::warn!("ICE connectivity checks failed, falling back: {e}");
            None
        }
    }
}

/// Combine the raw negotiated pieces -- local RTP port/SRTP key, a TURN
/// relay and/or ICE connection if either was used, the negotiated codec, and
/// the remote's SRTP key if any -- into the two derived shapes every
/// call-setup path ends with: `CallMedia` (kept on the `Dialog` so hold/
/// resume can rebuild their SDP later without redoing any of this) and
/// `CallMediaReady` (handed to `ui` via `SipEvent::CallConnected`). Shared by
/// `accept_call`'s and the background-task result handlers for
/// `initiate_call`/`on_response`'s answer path, so the SRTP-session/relay-
/// selection logic isn't duplicated three ways.
/// Shared by `resolve_call_media`/`resolve_video_media`: derive the actual
/// SRTP session (both sides' keys, if both offered one) and the connected
/// transport to hand to `MediaEngine::start` (an ICE connection if one was
/// negotiated, else a TURN relay if configured, else `None` for plain
/// direct UDP) -- codec-agnostic, so identical for audio and video legs.
fn resolve_srtp_and_relay(
    local_srtp: &Option<SrtpParams>, remote_srtp: &Option<SrtpParams>, relay: &Option<TurnRelay>,
    ice: &Option<IceConnection>, wants_srtp: bool,
) -> (Option<SrtpSession>, Option<Arc<dyn Conn + Send + Sync>>) {
    let srtp_session = match (local_srtp, remote_srtp) {
        (Some(local), Some(remote)) => Some(SrtpSession { local: local.clone(), remote: remote.clone() }),
        _ => {
            if wants_srtp {
                tracing::warn!("SRTP requested but remote SDP has no a=crypto -- falling back to plaintext RTP");
            }
            None
        }
    };
    let relay_conn: Option<Arc<dyn Conn + Send + Sync>> =
        ice.as_ref().map(|c| c.conn.clone()).or_else(|| relay.as_ref().map(|r| r.conn.clone()));
    (srtp_session, relay_conn)
}

#[allow(clippy::too_many_arguments)] // each param is a distinct, meaningfully-named
// piece of one call leg's negotiated
// media state -- bundling them into a
// struct wouldn't reduce real complexity.
pub fn resolve_call_media(
    local_rtp: u16, local_srtp: Option<SrtpParams>, relay: Option<TurnRelay>, ice: Option<IceConnection>,
    codec: AudioCodec, dtmf_type: Option<u8>, cn_type: Option<u8>, remote_rtp: SocketAddr,
    remote_srtp: Option<SrtpParams>, wants_srtp: bool, local_dtls: Option<DtlsCallParams>,
) -> (CallMedia, CallMediaReady) {
    let (srtp_session, relay_conn) = resolve_srtp_and_relay(&local_srtp, &remote_srtp, &relay, &ice, wants_srtp);

    let media = CallMedia {
        local_rtp,
        local_srtp,
        relay,
        ice,
        codec,
        dtmf_type,
        cn_type,
        video: None,
        local_dtls: local_dtls.clone(),
    };
    let ready = CallMediaReady {
        codec,
        dtmf_type,
        cn_type,
        local_rtp,
        remote_rtp,
        srtp: srtp_session,
        relay: relay_conn,
        video: None,
        local_dtls,
    };
    (media, ready)
}

/// Video counterpart of `resolve_call_media` -- same shared SRTP/relay
/// derivation, no `dtmf_type`/`cn_type` (neither applies to video). A
/// sibling function rather than growing `resolve_call_media`'s already-long
/// argument list further, matching this file's existing convention (see
/// that function's own `#[allow(clippy::too_many_arguments)]` comment).
#[allow(clippy::too_many_arguments)]
pub fn resolve_video_media(
    local_rtp: u16, local_srtp: Option<SrtpParams>, relay: Option<TurnRelay>, ice: Option<IceConnection>,
    codec: VideoCodec, remote_rtp: SocketAddr, remote_srtp: Option<SrtpParams>, wants_srtp: bool,
) -> (VideoMedia, VideoMediaReady) {
    let (srtp_session, relay_conn) = resolve_srtp_and_relay(&local_srtp, &remote_srtp, &relay, &ice, wants_srtp);

    let media = VideoMedia { local_rtp, local_srtp, relay, ice, codec };
    let ready = VideoMediaReady { codec, local_rtp, remote_rtp, srtp: srtp_session, relay: relay_conn };
    (media, ready)
}

#[cfg(test)]
#[path = "../../tests/unit/media_setup.rs"]
mod tests;
