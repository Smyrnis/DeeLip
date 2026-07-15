use std::net::SocketAddr;
use std::sync::Arc;

use webrtc_util::Conn;

use crate::call::media_setup::DtlsCallParams;
use crate::subscription::mwi::MwiState;
use crate::subscription::presence::PresenceState;
use crate::wire::sdp::{AudioCodec, SrtpSession, VideoCodec};

/// Everything `deelip_media::MediaEngine::start` needs, ready to use --
/// SDP parsing, codec negotiation, and STUN/TURN/ICE endpoint resolution
/// have already happened inside `SipStack` by the time this is sent, so the
/// receiver (`ui`) never has to touch SDP/ICE/TURN itself. `Clone` (just an
/// `Arc` bump for `relay`) so `ui` can keep it on a held `CallSlot` and reuse
/// it verbatim to restart media on resume, rather than re-deriving it. Not
/// `Debug`: `Arc<dyn Conn>` isn't printable, and nothing needs to print a
/// `SipEvent` today.
#[derive(Clone)]
pub struct CallMediaReady {
    pub codec: AudioCodec,
    pub dtmf_type: Option<u8>,
    /// Comfort-noise PT the remote signaled, if any -- see `ParsedSdp::cn_type`.
    pub cn_type: Option<u8>,
    pub local_rtp: u16,
    pub remote_rtp: SocketAddr,
    pub srtp: Option<SrtpSession>,
    /// The connected transport to hand to `MediaEngine::start`'s `relay`
    /// param -- an ICE connection if one was negotiated, else a TURN relay
    /// if one is configured, else `None` (plain direct UDP).
    pub relay: Option<Arc<dyn Conn + Send + Sync>>,
    /// Negotiated video leg, if `SipAccount::video_enabled` and the remote
    /// both offered/accepted one -- `None` for an audio-only call. See
    /// `VideoMediaReady` and docs/crates/sip-core.md's "Video negotiation" section.
    pub video: Option<VideoMediaReady>,
    /// RFC 5763/5764 DTLS-SRTP session state to hand to
    /// `MediaEngineOptions.dtls_srtp` -- call-scoped, shared by `video` too
    /// (not duplicated into `VideoMediaReady`). See `DtlsCallParams`.
    pub local_dtls: Option<DtlsCallParams>,
}

/// Video counterpart of `CallMediaReady` -- no `dtmf_type`/`cn_type` (neither
/// applies to video).
#[derive(Clone)]
pub struct VideoMediaReady {
    pub codec: VideoCodec,
    pub local_rtp: u16,
    pub remote_rtp: SocketAddr,
    pub srtp: Option<SrtpSession>,
    pub relay: Option<Arc<dyn Conn + Send + Sync>>,
}

/// Events emitted by the SIP stack to the application.
// `CallConnected`'s `media: CallMediaReady` now carries `DtlsCallParams`
// (cert/key DER bytes), making it noticeably larger than this enum's other
// variants -- deliberately not boxed, matching `EventSender::send`'s own
// established precedent (see its doc comment) of accepting this cost
// rather than adding indirection to every construction/match site.
#[allow(clippy::large_enum_variant)]
pub enum SipEvent {
    Registered {
        expires: u32,
    },
    RegistrationFailed {
        reason: String,
        /// `true` if retrying can never fix this (wrong credentials/unknown
        /// user) -- see `registration::PermanentRegError`. The reconnect
        /// loop stops re-registering once this fires; `false` covers every
        /// other case (network blips, 5xx, disconnects), which keep
        /// retrying with backoff exactly like before this field existed.
        permanent: bool,
    },
    /// Remote party is ringing (180 received on outgoing call).
    CallRinging {
        call_id: String,
    },
    /// Outgoing call answered, or our `AcceptCall` was confirmed -- either
    /// way, media is ready to start.
    CallConnected {
        call_id: String,
        media: CallMediaReady,
    },
    /// Incoming INVITE arrived.
    IncomingCall {
        call_id: String,
        from: String,
        /// `answer-after=N` from a `Call-Info` header, if present -- an
        /// intercom/paging-hardware convention (see
        /// `wire::util::parse_call_info_answer_after`). Only acted on by
        /// `ui` when the account's own `auto_answer_control_button`/
        /// `deny_incoming_control_button` opts in; otherwise ignored like
        /// today.
        remote_answer_after: Option<u32>,
    },
    CallEnded {
        call_id: String,
    },
    CallFailed {
        call_id: String,
        code: u16,
        reason: String,
    },
    /// Our hold re-INVITE was accepted — call is now on hold.
    CallHeld {
        call_id: String,
    },
    /// Our resume re-INVITE was accepted — call is active again.
    CallResumed {
        call_id: String,
    },
    /// Remote side put us on hold via re-INVITE.
    RemoteHeld {
        call_id: String,
    },
    /// Remote side resumed us via re-INVITE.
    RemoteResumed {
        call_id: String,
    },
    /// Our blind-transfer REFER was accepted (2xx) — the far end will
    /// typically send BYE on this dialog once the transferred call succeeds.
    TransferAccepted {
        call_id: String,
    },
    /// Our blind-transfer REFER was rejected.
    TransferFailed {
        call_id: String,
        reason: String,
    },
    /// Presence SUBSCRIBE accepted (200 OK); `expires` is the server-granted value.
    PresenceSubscribed {
        uri: String,
        expires: u32,
    },
    /// Presence SUBSCRIBE rejected.
    PresenceSubscribeFailed {
        uri: String,
        reason: String,
    },
    /// A NOTIFY updated a watched contact's presence state.
    PresenceUpdate {
        uri: String,
        state: PresenceState,
    },
    /// MWI SUBSCRIBE accepted (200 OK); `expires` is the server-granted value.
    MwiSubscribed {
        uri: String,
        expires: u32,
    },
    /// MWI SUBSCRIBE rejected.
    MwiSubscribeFailed {
        uri: String,
        reason: String,
    },
    /// A NOTIFY updated our mailbox's message-waiting state.
    MwiUpdate {
        uri: String,
        state: MwiState,
    },
    /// An incoming SIP MESSAGE (RFC 3428) arrived; already ack'd with 200 OK.
    MessageReceived {
        from: String,
        body: String,
    },
    /// Delivery result for a `SipCommand::SendMessage` -- `reason` is the
    /// status line/error text when `ok` is false.
    MessageSendResult {
        to: String,
        ok: bool,
        reason: Option<String>,
    },
}

/// Commands sent from the application into the SIP stack. SDP construction,
/// codec negotiation, and STUN/TURN/ICE resolution all happen inside
/// `SipStack` itself now -- callers only supply intent (see `CallMediaReady`'s
/// doc comment for why).
#[derive(Debug)]
pub enum SipCommand {
    /// `attempt_ice` lets a caller opt a specific call out of ICE even when
    /// it's enabled globally (the attended-transfer consultation call does
    /// this -- see `deelip_ui`'s `place_call`).
    MakeCall {
        to: String,
        attempt_ice: bool,
    },
    AcceptCall {
        call_id: String,
    },
    RejectCall {
        call_id: String,
    },
    HangUp {
        call_id: String,
    },
    /// Send a hold re-INVITE (a=sendonly).
    HoldCall {
        call_id: String,
    },
    /// Send a resume re-INVITE (a=sendrecv).
    ResumeCall {
        call_id: String,
    },
    /// Blind-transfer an active (Confirmed) call to `target` (a full SIP URI) via REFER.
    BlindTransfer {
        call_id: String,
        target: String,
    },
    /// Redirect a not-yet-answered incoming call via 302 Moved Temporarily.
    RedirectCall {
        call_id: String,
        target: String,
    },
    /// Subscribe to a contact's presence (`target_uri` is a full SIP URI).
    SubscribePresence {
        target_uri: String,
    },
    /// Unsubscribe from a contact's presence (sends SUBSCRIBE with Expires: 0).
    UnsubscribePresence {
        target_uri: String,
    },
    /// Attended-transfer `call_id` (the original call) via REFER with a
    /// `Replaces` parameter referencing `consultation_call_id`'s dialog.
    AttendedTransfer {
        call_id: String,
        consultation_call_id: String,
    },
    /// Send one DTMF digit via SIP INFO (`application/dtmf-relay` body)
    /// instead of RFC 2833 RTP telephone-events.
    SendDtmfInfo {
        call_id: String,
        digit: char,
    },
    /// Subscribe to a mailbox's MWI state (`target_uri` is a full SIP URI).
    SubscribeMwi {
        target_uri: String,
    },
    /// Send a standalone SIP MESSAGE (RFC 3428) to `to` (a full SIP URI).
    SendMessage {
        to: String,
        body: String,
    },
    /// (Re-)publish this account's own presence status (RFC 3903 PUBLISH) --
    /// sent when the user toggles DND while `SipAccount::publish_presence`
    /// is on. The initial publish after registration happens inside
    /// `SipStack` itself, with no command needed.
    PublishPresence {
        available: bool,
    },
}
