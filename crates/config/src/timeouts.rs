//! Shared network-timeout constants -- previously each independently
//! defined (and independently named/valued in two cases) across
//! `crates/sip-core` and `crates/ui`. Consolidated here since both crates
//! already depend on `deelip-config` (a true leaf crate, safe to depend on
//! from anywhere) -- see `ARCHITECTURE_GAPS.md`'s "Reuse & file structure"
//! section for the dependency-direction reasoning. `crates/nat`'s own STUN/
//! TURN timeouts deliberately stay local instead of moving here: `nat` has
//! no other `deelip-*` dependency today, and pulling one in just for a
//! couple of numbers isn't worth the new coupling for that otherwise
//! standalone, WebRTC-only crate.

use std::time::Duration;

/// SIP REGISTER response wait (`sip-core/registration.rs`).
pub const REG_RECV_TIMEOUT: Duration = Duration::from_secs(10);
/// ICE candidate gathering (`sip-core/call/media_setup.rs`).
pub const ICE_GATHER_TIMEOUT: Duration = Duration::from_secs(3);
/// Post-connect probe REGISTER used by `TransportProtocol::Auto` to test a
/// candidate transport (`sip-core/client/connect.rs`).
pub const AUTO_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// DNS resolution, both the hand-rolled custom-nameserver path and the OS
/// resolver fallback (`sip-core/wire/dns.rs`).
pub const DNS_TIMEOUT: Duration = Duration::from_secs(3);
/// TCP connect / TLS handshake (`sip-core/transport.rs`).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// LDAP directory connect (`ui/views/directory.rs`) -- same value as
/// `CONNECT_TIMEOUT` above but a separate, unrelated constant (LDAP, not
/// SIP transport), coincidentally named the same before this
/// consolidation.
pub const DIRECTORY_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// LDAP directory search (`ui/views/directory.rs`).
pub const DIRECTORY_SEARCH_TIMEOUT: Duration = Duration::from_secs(10);
