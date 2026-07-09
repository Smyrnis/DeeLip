//! ZRTP (RFC 6189) media encryption -- a from-scratch implementation (see
//! `wire.rs`'s doc comment for scope/provenance caveats). Runs in-band on
//! the RTP socket, entirely independent of SIP/SDP signaling: the only
//! thing a call needs to decide is whether to attempt it at all
//! (`SipAccount::wants_zrtp`) and which side is the DH `Role` (mapped from
//! the SIP caller/callee role) -- everything else is negotiated over the
//! ZRTP messages themselves.

pub mod cache;
pub mod crypto;
pub mod engine;
pub mod wire;

pub use cache::{CacheEntry, MemorySharedSecretStore, RetainedSecrets, SharedSecretStore};
pub use engine::{EngineEvent, HandshakeState, Role, ZrtpEngine};
pub use wire::{is_zrtp_packet, Message, Packet};
