//! Split from a single `account.rs` purely for file size (same precedent as
//! `views/settings/`, `views/dialer/`, `sip-core/src/call/lifecycle/`), not
//! a behavior/API change -- every item re-exported below was already `pub`
//! at this same `account::` (and, via `lib.rs`, `deelip_config::`) path in
//! the original file.

mod app_config;
mod db;
mod enums;
mod sip_account;

pub use app_config::AppConfig;
pub use enums::{
    DefaultListAction, DtmfMode, Language, MediaEncryption, RecordingFormat, TransportProtocol, UpdateCheckFrequency,
};
pub use sip_account::{AudioConfig, SipAccount};
