//! SDP construction combined with STUN/TURN/ICE endpoint resolution for one
//! call leg -- the actual call-setup "business logic" that used to live in
//! the `ui` crate (`ui/src/media.rs`), moved here so it runs inside
//! `SipStack`'s own async task instead of being `rt.block_on`'d from the
//! egui UI thread. That mattered: ICE gathering alone has a multi-second
//! timeout, so doing STUN/TURN/ICE synchronously on the UI thread froze the
//! whole window for the duration on every call.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use deelip_config::SipAccount;
use deelip_nat::{IceConnection, IceGathered, TurnRelay};
use webrtc_util::Conn;

use crate::call::dialog::CallMedia;
use crate::events::CallMediaReady;
use crate::wire::sdp::{AudioCodec, IceAttrs, ParsedSdp, SrtpParams, SrtpSession, ALL_CODECS};

/// Bounded wait for ICE candidate gathering (host candidates are instant;
/// server-reflexive/relay each cost one STUN/TURN round trip) -- generous
/// enough for a live network's worst case without stalling call setup for
/// long if a configured STUN/TURN server is simply unreachable.
const ICE_GATHER_TIMEOUT: Duration = Duration::from_secs(3);

/// Network settings a `SipStack` needs for call setup -- separate from
/// `SipAccount` since these are process-wide (Settings' Network section),
/// not per-account. Passed in once at `SipStack::spawn` time; like every
/// other Network setting in this app, changing it requires a restart.
#[derive(Debug, Clone, Default)]
pub struct NetworkConfig {
    pub stun_server: Option<String>,
    pub turn_server: Option<String>,
    pub turn_username: String,
    pub turn_password: String,
    pub ice_enabled: bool,
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

fn codec_from_str(s: &str) -> Option<AudioCodec> {
    match s {
        "opus" => Some(AudioCodec::Opus),
        "g722" => Some(AudioCodec::G722),
        "pcmu" => Some(AudioCodec::Pcmu),
        "pcma" => Some(AudioCodec::Pcma),
        "gsm"  => Some(AudioCodec::Gsm),
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
    network: &NetworkConfig,
    advertised_ip: &str,
    local_rtp: u16,
    relay: &mut Option<TurnRelay>,
) -> (String, u16) {
    if relay.is_none() {
        if let Some((server, username, password)) = network.turn() {
            match deelip_nat::allocate_relay(&server, &username, &password).await {
                Ok(r) => *relay = Some(r),
                Err(e) => tracing::warn!("TURN allocation failed ({e}), falling back to direct"),
            }
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
pub async fn try_gather_ice(network: &NetworkConfig, is_controlling: bool) -> Option<IceGathered> {
    if !network.ice_enabled { return None; }
    if network.stun_server.is_none() && network.turn_server.is_none() { return None; }
    let turn = network.turn();
    let turn_ref = turn.as_ref().map(|(s, u, p)| (s.as_str(), u.as_str(), p.as_str()));
    match deelip_nat::ice::gather(network.stun_server.as_deref(), turn_ref, is_controlling, ICE_GATHER_TIMEOUT).await {
        Ok(gathered) => Some(gathered),
        Err(e) => { tracing::warn!("ICE candidate gathering failed/timed out, falling back: {e}"); None }
    }
}

/// Finish an in-progress ICE negotiation once the remote's parsed SDP
/// (offer or answer, whichever direction) is known, running connectivity
/// checks and returning the winning connection. Only used for the *first*
/// SDP exchange of a new call -- `None` if `gathered` is `None` (ICE wasn't
/// attempted) or the remote's SDP didn't itself signal ICE support.
///
/// Note this is a *different* failure mode than `try_gather_ice` returning
/// `None`: by the time this runs, our own offer/answer has already been sent
/// committing the far end to our gathered default candidate's address
/// (that's the whole point of RFC 8445's mandated default-candidate-in-c=/m=
/// backwards-compatibility rule). If connectivity checks then fail here,
/// there's no clean way back to the plain `resolve_rtp_endpoint` path
/// post-commitment (it would bind a different socket than the address
/// already promised in the sent SDP) -- this is an inherent structural limit
/// of ICE's gather-then-commit shape, not a bug, and the call is simply left
/// without working media in that case, same as any other NAT-traversal
/// failure this codebase never protected against pre-ICE either.
pub async fn finish_ice_connect(gathered: Option<IceGathered>, is_controlling: bool, parsed: &ParsedSdp) -> Option<IceConnection> {
    let gathered = gathered?;
    let ufrag = parsed.ice_ufrag.clone()?;
    let pwd = parsed.ice_pwd.clone()?;
    if parsed.ice_candidates.is_empty() { return None; }
    match deelip_nat::ice::connect(gathered, is_controlling, &ufrag, &pwd, &parsed.ice_candidates).await {
        Ok(conn) => Some(conn),
        Err(e) => { tracing::warn!("ICE connectivity checks failed, falling back: {e}"); None }
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
pub async fn try_answer_with_ice(network: &NetworkConfig, offer: &ParsedSdp) -> Option<(IceAttrs, SocketAddr, IceConnection)> {
    if !network.ice_enabled { return None; }
    let ufrag = offer.ice_ufrag.as_deref()?;
    let pwd = offer.ice_pwd.as_deref()?;
    if offer.ice_candidates.is_empty() { return None; }
    let gathered = try_gather_ice(network, false).await?;
    let attrs = IceAttrs { ufrag: gathered.local_ufrag.clone(), pwd: gathered.local_pwd.clone(), candidates: gathered.candidates.clone() };
    let default_addr = gathered.default_addr;
    match deelip_nat::ice::connect(gathered, false, ufrag, pwd, &offer.ice_candidates).await {
        Ok(conn) => Some((attrs, default_addr, conn)),
        Err(e) => { tracing::warn!("ICE connectivity checks failed, falling back: {e}"); None }
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
#[allow(clippy::too_many_arguments)] // each param is a distinct, meaningfully-named
                                      // piece of one call leg's negotiated
                                      // media state -- bundling them into a
                                      // struct wouldn't reduce real complexity.
pub fn resolve_call_media(
    local_rtp:      u16,
    local_srtp:     Option<SrtpParams>,
    relay:          Option<TurnRelay>,
    ice:            Option<IceConnection>,
    codec:          AudioCodec,
    dtmf_type:      Option<u8>,
    remote_rtp:     SocketAddr,
    remote_srtp:    Option<SrtpParams>,
    account_secure: bool,
) -> (CallMedia, CallMediaReady) {
    let srtp_session = match (&local_srtp, &remote_srtp) {
        (Some(local), Some(remote)) => Some(SrtpSession { local: local.clone(), remote: remote.clone() }),
        _ => {
            if account_secure {
                tracing::warn!("TLS signaling active but remote SDP has no a=crypto -- falling back to plaintext RTP");
            }
            None
        }
    };
    let relay_conn: Option<Arc<dyn Conn + Send + Sync>> = ice.as_ref().map(|c| c.conn.clone())
        .or_else(|| relay.as_ref().map(|r| r.conn.clone()));

    let media = CallMedia { local_rtp, local_srtp, relay, ice, codec, dtmf_type };
    let ready = CallMediaReady { codec, dtmf_type, local_rtp, remote_rtp, srtp: srtp_session, relay: relay_conn };
    (media, ready)
}

#[cfg(test)]
#[path = "../../tests/unit/media_setup.rs"]
mod tests;
