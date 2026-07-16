use deelip_nat::{IceConnection, IceGathered, TurnRelay};

use crate::call::media_setup::DtlsCallParams;
use crate::wire::sdp::{AudioCodec, SrtpParams, VideoCodec};

/// State of a SIP call dialog (simplified early/confirmed dialog).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogState {
    /// We sent INVITE or received INVITE; not yet confirmed.
    Calling,
    Ringing,
    Confirmed,
    Terminating,
    Terminated,
}

/// The negotiated media state for a confirmed call -- kept on the `Dialog`
/// so hold/resume can rebuild their re-INVITE SDP (same codec/SRTP key, and
/// the same TURN relay reused rather than re-allocated) without redoing any
/// STUN/TURN/ICE resolution or touching the remote SDP again.
///
/// Deliberately *not* `Clone`/`Debug`-derivable (neither is `Dialog` anymore,
/// for the same reason): `TurnRelay`/`IceConnection` hold live network
/// resources (an open relay allocation, a running ICE agent) that can't be
/// cloned and aren't meaningfully printable.
pub struct CallMedia {
    pub local_rtp: u16,
    pub local_srtp: Option<SrtpParams>,
    pub relay: Option<TurnRelay>,
    pub ice: Option<IceConnection>,
    pub codec: AudioCodec,
    pub dtmf_type: Option<u8>,
    /// Comfort-noise PT the remote signaled, if any -- see `ParsedSdp::cn_type`.
    pub cn_type: Option<u8>,
    /// Negotiated video leg, if `SipAccount::video_enabled` and the remote
    /// both offered/accepted one -- `None` for an audio-only call. See
    /// docs/crates/sip-core.md's "Video negotiation" section.
    pub video: Option<VideoMedia>,
    /// RFC 5763/5764 DTLS-SRTP session state, call-scoped (shared by
    /// `video` too, not duplicated into `VideoMedia`) -- see
    /// `DtlsCallParams`'s doc comment.
    pub local_dtls: Option<DtlsCallParams>,
}

/// Video counterpart of `CallMedia` -- no `dtmf_type`/`cn_type` (neither
/// applies to video).
pub struct VideoMedia {
    pub local_rtp: u16,
    pub local_srtp: Option<SrtpParams>,
    pub relay: Option<TurnRelay>,
    pub ice: Option<IceConnection>,
    pub codec: VideoCodec,
}

/// Offerer-side media state resolved before the INVITE was sent (local RTP
/// port, our own SRTP key, a TURN relay if one is in use) -- unlike the
/// answerer side, which knows the negotiated codec immediately (it's
/// choosing from the already-received offer), the offerer doesn't know the
/// codec until the answer arrives. Held here in the meantime; consumed
/// (taken) in `on_response`'s `Act::Connected` handling to build the final
/// `CallMedia` once the answer's codec is known.
pub struct PendingOfferMedia {
    pub local_rtp: u16,
    pub local_srtp: Option<SrtpParams>,
    pub relay: Option<TurnRelay>,
    /// See `DtlsCallParams`'s doc comment -- `role`/`remote_fingerprint`
    /// are still unresolved at this point, filled in once the answer
    /// arrives (`handle_connected`).
    pub local_dtls: Option<DtlsCallParams>,
}

/// Offerer-side video-leg state resolved before the INVITE was sent,
/// mirroring `PendingOfferMedia` -- kept separately (rather than folding
/// into `PendingOfferMedia`) since it's `None` whenever
/// `SipAccount::video_enabled` is off (every account today) or video setup
/// failed for this call, unlike the always-present audio leg. Also carries
/// its own `ice_gathered` since video's ICE gather runs independently of
/// audio's, alongside it in the same background task.
pub struct PendingVideoOffer {
    pub local_rtp: u16,
    pub local_srtp: Option<SrtpParams>,
    pub relay: Option<TurnRelay>,
    pub ice_gathered: Option<IceGathered>,
}

pub struct Dialog {
    pub call_id: String,
    pub local_tag: String,
    pub remote_tag: Option<String>,
    pub remote_uri: String,
    pub remote_contact: Option<String>,
    /// The inbound INVITE's verbatim `Via` header -- responses to that
    /// INVITE (200 OK, 486, etc.) must echo it back unchanged (including its
    /// `branch`) for the sender to match the response to its transaction;
    /// synthesizing a fresh Via/branch here gets silently ignored by at
    /// least Asterisk/pjproject, which just keeps waiting for a real
    /// response until its own timeout fires.
    pub remote_via: String,
    /// The branch we put on our own most recent outgoing INVITE (initial or
    /// re-INVITE). A non-2xx response's ACK must reuse this exact branch
    /// (RFC 3261 §17.1.1.3) -- only a 2xx ACK is a new transaction with its
    /// own fresh branch.
    pub invite_branch: String,
    pub local_cseq: u32,
    pub remote_cseq: Option<u32>,
    pub state: DialogState,
    pub remote_sdp: Option<String>,
    /// Last SDP we sent (needed to repeat in re-INVITE 200 OK).
    pub local_sdp: Option<String>,
    /// Whether the call is currently on hold (our side initiated).
    pub is_held: bool,
    /// Some(true) = hold re-INVITE pending; Some(false) = resume pending.
    pub hold_pending: Option<bool>,
    /// Set once we've retried the initial INVITE with digest auth, so a second
    /// 401/407 (bad credentials) is treated as a final failure, not another retry.
    pub auth_retried: bool,
    /// Negotiated media state, populated once the call is confirmed --
    /// `None` before then (or after `Dialog` is otherwise ready but the
    /// call hasn't connected/been accepted yet).
    pub media: Option<CallMedia>,
    /// Offerer-side ICE candidates gathered before the INVITE was sent,
    /// held here until the answer arrives (`on_response`) so they can be
    /// fed into `media_setup::finish_ice_connect` -- `None` throughout if
    /// ICE wasn't attempted for this call. Consumed (taken) once the answer
    /// arrives, same one-shot idiom as `hold_pending`.
    pub ice_gathered: Option<IceGathered>,
    /// Offerer-side media state resolved before the INVITE was sent --
    /// `None` after the call is confirmed (folded into `media` by then) or
    /// for an incoming call (the answerer path populates `media` directly,
    /// never this). See `PendingOfferMedia`'s doc comment.
    pub pending_offer: Option<PendingOfferMedia>,
    /// Video counterpart of `pending_offer` -- `None` whenever video wasn't
    /// offered at all (the common case today). See `PendingVideoOffer`.
    pub pending_offer_video: Option<PendingVideoOffer>,
    /// RFC 4028 Session Timers: the negotiated refresh interval in seconds
    /// -- `None` if either side doesn't support/want them (no
    /// `Session-Expires` ended up negotiated).
    pub session_expires: Option<u32>,
    /// Whether *we* are responsible for sending the periodic refresh
    /// re-INVITE (`refresher=uac` when we placed the call and proposed it,
    /// `refresher=uas` when we accepted an incoming call that asked us to
    /// refresh) -- meaningless if `session_expires` is `None`.
    pub we_are_refresher: bool,
    /// Deadline for our next refresh re-INVITE (`we_are_refresher` only,
    /// scheduled at half `session_expires`, matching common UA behavior).
    /// The periodic scan in `client::run` only acts on entries already
    /// past due, same idiom as `PresenceSubscription::refresh_after`.
    pub session_refresh_at: Option<tokio::time::Instant>,
    /// Set right before sending a session-refresh re-INVITE so the
    /// Confirmed-state re-INVITE-response handler in `on_response` doesn't
    /// mistake it for a hold/resume ack (`hold_pending`) -- taken (reset to
    /// `false`) once that response arrives.
    pub session_refresh_pending: bool,
    /// Set once we've retried our own initial INVITE with a larger
    /// `Session-Expires` after a 422 (Session Interval Too Small) response,
    /// so a second 422 is treated as a final failure rather than retried
    /// forever -- mirrors `auth_retried`'s shape for the 401/407 path.
    pub session_expires_retried: bool,
    /// Parsed from an incoming INVITE's own `Session-Expires`/`refresher=`
    /// (RFC 4028), held here between `on_invite` (parses it) and
    /// `on_incoming_answer_ready` (echoes it back in the 200 OK and folds
    /// it into `session_expires`/`we_are_refresher` above) -- `None` if the
    /// incoming INVITE didn't propose session timers at all.
    pub incoming_session_expires: Option<(u32, Option<String>)>,
    /// Whether *we* sent the original INVITE for this dialog (`true`) or
    /// received it (`false`) -- fixed for the dialog's lifetime, unlike
    /// who currently sends the periodic refresh (`we_are_refresher`). RFC
    /// 4028's `refresher=uac`/`refresher=uas` param always refers to these
    /// original roles, not to whoever happens to send a given re-INVITE, so
    /// `send_session_refresh` needs this to pick the right literal value.
    pub original_role_is_uac: bool,
}

impl Dialog {
    pub fn new_outgoing(call_id: String, local_tag: String, to_uri: String) -> Self {
        Self {
            call_id,
            local_tag,
            remote_tag: None,
            remote_uri: to_uri,
            remote_contact: None,
            remote_via: String::new(),
            invite_branch: String::new(),
            local_cseq: 1,
            remote_cseq: None,
            state: DialogState::Calling,
            remote_sdp: None,
            local_sdp: None,
            is_held: false,
            hold_pending: None,
            auth_retried: false,
            media: None,
            ice_gathered: None,
            pending_offer: None,
            pending_offer_video: None,
            session_expires: None,
            we_are_refresher: false,
            session_refresh_at: None,
            session_refresh_pending: false,
            session_expires_retried: false,
            incoming_session_expires: None,
            original_role_is_uac: true,
        }
    }

    pub fn new_incoming(
        call_id: String, local_tag: String, from_uri: String, from_tag: String, remote_cseq: u32, remote_sdp: String,
        remote_via: String,
    ) -> Self {
        Self {
            call_id,
            local_tag,
            remote_tag: Some(from_tag),
            remote_uri: from_uri,
            remote_contact: None,
            remote_via,
            invite_branch: String::new(),
            local_cseq: 0,
            remote_cseq: Some(remote_cseq),
            state: DialogState::Calling,
            remote_sdp: Some(remote_sdp),
            local_sdp: None,
            is_held: false,
            hold_pending: None,
            auth_retried: false,
            media: None,
            ice_gathered: None,
            pending_offer: None,
            pending_offer_video: None,
            session_expires: None,
            we_are_refresher: false,
            session_refresh_at: None,
            session_refresh_pending: false,
            session_expires_retried: false,
            incoming_session_expires: None,
            original_role_is_uac: false,
        }
    }

    pub fn next_local_cseq(&mut self) -> u32 {
        self.local_cseq += 1;
        self.local_cseq
    }

    /// Extract `host:port` from a caller-side 200 OK's own `Contact:` header
    /// value, for `Dialog::remote_contact` -- split out of `on_response`'s
    /// `Act::Connected` handling so the caller-side capture (see
    /// docs/crates/sip-core.md's "Dialog::remote_contact must be populated on
    /// the caller side too" note -- a real bug this project hit live) is
    /// directly testable without a live `SipStack`. Mirrors what
    /// `incoming.rs::on_invite` already captures from the source address on
    /// the callee side.
    pub(crate) fn parse_remote_contact(contact_header: Option<&str>) -> Option<String> {
        contact_header
            .and_then(crate::wire::util::parse_uri)
            .and_then(|uri| crate::wire::util::uri_host_port(&uri))
            .map(|(host, port)| format!("{host}:{port}"))
    }
}

#[cfg(test)]
#[path = "../../tests/unit/dialog.rs"]
mod tests;
