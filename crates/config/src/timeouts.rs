//! Shared network-timeout constants used by `sip-core`/`ui`. See
//! `docs/crates/config.md`'s Design decisions & invariants section for why
//! they live here (and why `nat`'s own STUN/TURN timeouts don't).

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
