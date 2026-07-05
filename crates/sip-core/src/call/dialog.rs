use deelip_nat::{IceConnection, IceGathered, TurnRelay};

use crate::wire::sdp::{AudioCodec, SrtpParams};

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
        }
    }

    pub fn new_incoming(
        call_id: String,
        local_tag: String,
        from_uri: String,
        from_tag: String,
        remote_cseq: u32,
        remote_sdp: String,
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
        }
    }

    pub fn next_local_cseq(&mut self) -> u32 {
        self.local_cseq += 1;
        self.local_cseq
    }
}
