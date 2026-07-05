//! SUBSCRIBE/NOTIFY event packages: presence (RFC 3856) and voicemail MWI
//! (RFC 3842) subscription state/parsing, plus the `SipStack` methods that
//! send/refresh/handle both over the wire.

mod handlers;
pub mod mwi;
pub mod presence;
