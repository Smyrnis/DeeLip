mod call;
pub mod client;
pub mod events;
mod handle;
mod message_method;
mod registration;
mod subscription;
pub mod transport;
mod wire;
pub mod zrtp;

pub use call::dialog;
pub use call::media_setup;
pub use call::media_setup::DtlsCallParams;
pub use client::SipStack;
pub use events::{CallMediaReady, SipCommand, SipEvent, VideoMediaReady};
pub use handle::SipHandle;
pub use subscription::mwi;
pub use subscription::mwi::MwiState;
pub use subscription::presence;
pub use subscription::presence::PresenceState;
pub use transport::SipTransport;
pub use wire::auth;
pub use wire::message;
pub use wire::message::{SipMessage, SipMethod, SipStartLine};
pub use wire::sdp;
pub use wire::sdp::{
    AudioCodec, DtlsFingerprint, IceAttrs, ParsedSdp, Setup, SrtpParams, SrtpSession, build_answer, build_hold_offer,
    build_offer, build_resume_offer, generate_dtls_cert, parse_sdp,
};
pub use wire::util;

/// The `User-Agent` sent on every outgoing SIP request/response, kept in
/// lockstep with the crate's own (workspace-inherited) version.
pub(crate) const USER_AGENT: &str = concat!("DeeLip/", env!("CARGO_PKG_VERSION"));
