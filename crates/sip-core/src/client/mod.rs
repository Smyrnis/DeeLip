//! Split from a single `client.rs` purely for file size (same precedent as
//! `views/settings/`, `views/dialer/`, `sip-core/src/call/lifecycle/`), not
//! a behavior/API change -- `SipStack` keeps every method it had, just
//! spread across `connect.rs` (transport setup + reconnect loop),
//! `run_loop.rs` (the main event loop + dispatchers), and `builders.rs`
//! (wire-format response/ACK builders), the same way its other methods
//! already live in `call::dialog`/`registration`/`subscription::*`/etc.
//! Every name re-exported below was already `pub`/`pub(crate)` at this same
//! `client::` path in the original file.

mod builders;
mod connect;
mod events;
mod run_loop;

pub use events::EventSender;
pub(crate) use builders::{build_contact, build_via};
pub(crate) use events::{IncomingVideoAnswer, OutgoingVideoConnected, StackEvent};

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

use tokio::sync::mpsc;

use deelip_config::{SipAccount, TransportProtocol};

use crate::{
    call::dialog::Dialog, call::media_setup::NetworkConfig, events::SipCommand, subscription::mwi::MwiSubscription,
    subscription::presence::PresenceSubscription, subscription::publish::PresencePublish, transport::SipTransport,
};

const REG_MARGIN: u32 = 60;
pub(crate) const REG_RECV_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RETRY: Duration = Duration::from_secs(300);
pub(crate) const SUBSCRIBE_EXPIRES: u32 = 3600;
const PRESENCE_TICK: Duration = Duration::from_secs(30);
pub(crate) const PRESENCE_EVENT: &str = "presence";
pub(crate) const PRESENCE_ACCEPT: &str = "application/pidf+xml";
pub(crate) const MWI_EVENT: &str = "message-summary";
pub(crate) const MWI_ACCEPT: &str = "application/simple-message-summary";
/// RFC 4028 Session Timers: the interval we propose on our own outgoing
/// INVITEs, and echo back (unmodified) when accepting an incoming one --
/// 1800s (30 min) is RFC 4028's own worked example and a common default
/// among real UAs.
pub(crate) const SESSION_EXPIRES_DEFAULT: u32 = 1800;
/// Floor we advertise via `Min-SE` -- RFC 4028's own suggested default.
pub(crate) const SESSION_MIN_SE: u32 = 90;

pub struct SipStack {
    pub(crate) transport: Arc<SipTransport>,
    pub(crate) account: SipAccount,
    pub(crate) network: NetworkConfig,
    pub(crate) local_ip: String,
    pub(crate) advertised_ip: String,
    pub(crate) local_port: u16,
    pub(crate) server_addr: SocketAddr,
    /// Host (`account.domain()`, or `local_ip:local_port` when that's empty
    /// -- only possible for `SipAccount::local_account`, which has no
    /// `server`/`domain` to fall back to) used to build this account's own
    /// From/Contact/To identity in outgoing requests. Computed once here
    /// instead of calling `account.domain()` fresh at each call site so a
    /// serverless account still gets a valid (non-empty) URI host.
    pub(crate) identity_host: String,
    /// The concrete transport actually in use -- identical to
    /// `account.transport` unless that's `TransportProtocol::Auto`, in
    /// which case this is whichever of Udp/Tcp/Tls `connect_transport`
    /// resolved it to. Everything that cares about "is this connection
    /// TLS/UDP/TCP" (via headers, SRTP-by-default, `SipHandle.secure`)
    /// reads this, never `account.transport` directly.
    pub(crate) resolved_transport: TransportProtocol,

    pub(crate) reg_call_id: String,
    pub(crate) reg_from_tag: String,
    pub(crate) reg_cseq: Arc<AtomicU32>,

    pub(crate) dialogs: HashMap<String, Dialog>,
    pub(crate) subscriptions: HashMap<String, PresenceSubscription>,
    pub(crate) mwi_subscriptions: HashMap<String, MwiSubscription>,
    /// This account's own outgoing presence PUBLISH state -- `None` until
    /// the first publish (see `subscription::publish`), regardless of
    /// whether `SipAccount::publish_presence` is even enabled.
    pub(crate) presence_publish: Option<PresencePublish>,
    /// Outstanding SIP MESSAGE requests awaiting their response, keyed by
    /// Call-ID -- MESSAGE (RFC 3428) is a standalone transaction, not part
    /// of any `Dialog`, so it can't be resolved via `dialogs`.
    pub(crate) pending_messages: HashMap<String, crate::message_method::PendingMessage>,
    pub(crate) event_tx: EventSender,
    pub(crate) cmd_rx: mpsc::UnboundedReceiver<SipCommand>,

    /// See `StackEvent`'s doc comment -- `internal_tx` is cloned into each
    /// background call-setup task; `internal_rx` is polled by `run()`'s own
    /// `select!` loop, right alongside `cmd_rx`/`transport.recv()`.
    pub(crate) internal_tx: mpsc::UnboundedSender<StackEvent>,
    internal_rx: mpsc::UnboundedReceiver<StackEvent>,
}

/// The command-receiving half survives across a reconnect (it's tied to the
/// `cmd_tx` held externally by `SipHandle`, which must transparently keep
/// working across a transport failure) -- both `SipStack::new` and `run`
/// hand it back on failure, via this alias, so `spawn`'s reconnect loop can
/// feed it into the next attempt instead of losing it.
pub(crate) type CmdRx = mpsc::UnboundedReceiver<SipCommand>;
