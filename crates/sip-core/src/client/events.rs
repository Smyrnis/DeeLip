//! Background call-setup event plumbing: `StackEvent` (fed into `run()`'s own
//! `select!` loop via `internal_rx`) and `EventSender` (the outward-facing
//! `SipEvent` channel wrapper).

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::mpsc;

use deelip_nat::{IceConnection, IceGathered, TurnRelay};

use crate::{
    call::dialog::{PendingOfferMedia, PendingVideoOffer},
    call::media_setup::DtlsCallParams,
    events::SipEvent,
    wire::sdp::{AudioCodec, ParsedSdp, ParsedVideoMedia, SrtpParams, VideoCodec},
};

/// Completed background call-setup results, fed back into `run()`'s own
/// `select!` loop via `internal_rx` -- see docs/crates/sip-core.md's "why call
/// setup is split into a background task" section for the full reasoning.
pub(crate) enum StackEvent {
    /// `initiate_call`'s offer is ready to send as the actual INVITE.
    OutgoingOfferReady {
        call_id: String,
        from_tag: String,
        branch: String,
        to: String,
        local_sdp: String,
        pending_offer: PendingOfferMedia,
        ice_gathered: Option<IceGathered>,
        /// Video counterpart of `pending_offer`/`ice_gathered` -- `None`
        /// whenever video wasn't offered (see `PendingVideoOffer`).
        video: Option<PendingVideoOffer>,
    },
    /// `accept_call`'s answer is ready to send as the 200 OK.
    IncomingAnswerReady {
        call_id: String,
        parsed: ParsedSdp,
        local_sdp: String,
        local_rtp: u16,
        local_srtp: Option<SrtpParams>,
        relay: Option<TurnRelay>,
        ice: Option<IceConnection>,
        /// See `DtlsCallParams` -- call-scoped, not duplicated per video.
        local_dtls: Option<DtlsCallParams>,
        /// Video counterpart of this event's audio fields, bundled into
        /// one struct -- `None` whenever the remote didn't offer video, we
        /// don't have it enabled, or video setup failed for this call.
        video: Option<IncomingVideoAnswer>,
    },
    /// The offerer side's ICE connectivity checks (`on_response`'s answer
    /// handling) finished -- media is now fully resolved for an outgoing call.
    OutgoingConnected {
        call_id: String,
        local_rtp: u16,
        local_srtp: Option<SrtpParams>,
        relay: Option<TurnRelay>,
        ice: Option<IceConnection>,
        codec: AudioCodec,
        dtmf_type: Option<u8>,
        cn_type: Option<u8>,
        remote_rtp: SocketAddr,
        remote_srtp: Option<SrtpParams>,
        /// See `DtlsCallParams` -- call-scoped, not duplicated per video.
        /// Fully resolved by now (`role`/`remote_fingerprint` both `Some`
        /// whenever this is `Some` at all), unlike `OutgoingOfferReady`'s
        /// copy inside `pending_offer`.
        local_dtls: Option<DtlsCallParams>,
        /// Video counterpart of this event's audio fields, bundled into
        /// one struct -- `None` whenever we didn't offer video, the remote
        /// didn't accept it, or video setup failed for this call.
        video: Option<OutgoingVideoConnected>,
    },
}

/// Bundles `accept_call`'s resolved video-answer state for
/// `StackEvent::IncomingAnswerReady` -- see that variant's doc comment.
pub(crate) struct IncomingVideoAnswer {
    pub parsed: ParsedVideoMedia,
    pub local_rtp: u16,
    pub local_srtp: Option<SrtpParams>,
    pub relay: Option<TurnRelay>,
    pub ice: Option<IceConnection>,
}

/// Bundles `on_response`'s resolved video-connected state for
/// `StackEvent::OutgoingConnected` -- see that variant's doc comment.
pub(crate) struct OutgoingVideoConnected {
    pub local_rtp: u16,
    pub local_srtp: Option<SrtpParams>,
    pub relay: Option<TurnRelay>,
    pub ice: Option<IceConnection>,
    pub codec: VideoCodec,
    pub remote_rtp: SocketAddr,
    pub remote_srtp: Option<SrtpParams>,
}

/// Wraps the raw event channel so every `event_tx.send(...)` call site in
/// this crate also wakes up whichever UI is consuming `SipEvent`s. See
/// docs/crates/sip-core.md's "EventSender" section.
#[derive(Clone)]
pub struct EventSender {
    inner: mpsc::UnboundedSender<SipEvent>,
    waker: Arc<dyn Fn() + Send + Sync>,
}

impl EventSender {
    pub(super) fn new(inner: mpsc::UnboundedSender<SipEvent>, waker: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self { inner, waker }
    }

    // Deliberately mirrors `UnboundedSender::send`'s exact signature -- see
    // this struct's doc comment / docs/crates/sip-core.md's "EventSender" section.
    #[allow(clippy::result_large_err)]
    pub fn send(&self, event: SipEvent) -> Result<(), mpsc::error::SendError<SipEvent>> {
        let result = self.inner.send(event);
        if result.is_ok() {
            (self.waker)();
        }
        result
    }
}
