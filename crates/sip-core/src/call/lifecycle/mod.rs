//! Call dialog lifecycle: outgoing/incoming call setup, hold/resume,
//! session timers, and the response dispatcher -- split by concern across
//! `outgoing.rs`/`reinvite.rs`/`response.rs`/`incoming.rs`/`teardown.rs`.
//! These are all still just `impl SipStack` blocks; the split is purely
//! organizational (see `call/transfer.rs` for another file already
//! following this same multi-file-single-impl pattern -- cross-file
//! inherent-method calls like `self.build_invite(...)` work regardless of
//! which file defines the method).

mod incoming;
mod outgoing;
mod reinvite;
mod response;
mod teardown;

use std::net::SocketAddr;

use crate::{call::dialog::Dialog, client::SipStack, wire::sdp::VideoCodec};

/// Every video codec this account will accept -- H.264 only for now (see
/// `wire::sdp::VideoCodec`). A future multi-codec video story would thread
/// an account-configurable list through here, mirroring
/// `media_setup::account_codecs` for audio; not needed while there's only
/// one variant.
const VIDEO_CODECS: [VideoCodec; 1] = [VideoCodec::H264];

/// Fields derived from `&self` alone (account/identity), independent of any
/// particular dialog. Built once via `SipStack::stack_identity()` --
/// *before* taking a `self.dialogs.get_mut(...)` borrow, since calling a
/// `&self` method afterward would conflict with that outstanding `&mut
/// self.dialogs` borrow (the same class of borrow-splitting issue this
/// codebase has hit before -- see `media_setup::resolve_rtp_endpoint`,
/// which became an associated fn for the same reason).
pub(super) struct StackIdentity {
    pub(super) server: String,
    pub(super) username: String,
    pub(super) display: String,
    pub(super) adv_ip: String,
    pub(super) local_ip: String,
    pub(super) local_port: u16,
    pub(super) server_addr: SocketAddr,
    pub(super) via_proto: &'static str,
    pub(super) contact_transport: &'static str,
}

impl SipStack {
    pub(super) fn stack_identity(&self) -> StackIdentity {
        let username = self.account.username.clone();
        let display = self
            .account
            .display_name
            .clone()
            .unwrap_or_else(|| username.clone());
        StackIdentity {
            server: self.identity_host.clone(),
            username,
            display,
            adv_ip: self.advertised_ip.clone(),
            local_ip: self.local_ip.clone(),
            local_port: self.local_port,
            server_addr: self.server_addr,
            via_proto: self.via_proto(),
            contact_transport: self.contact_transport_param(),
        }
    }
}

/// Everything needed to interpolate a mid-dialog SIP message (a fresh
/// request like a re-INVITE/INFO/BYE, or a response like a 200 OK/486) for
/// one `Dialog` -- built from a `StackIdentity` (already owned, no `self`
/// involved) + `&Dialog`, so it's callable while a `&mut Dialog` borrowed
/// from `self.dialogs` is still alive. Replaces what used to be ~10 lines
/// of hand-repeated `.clone()`s at 6 separate call sites.
pub(super) struct DialogRequestContext {
    pub(super) server: String,
    pub(super) username: String,
    pub(super) display: String,
    pub(super) adv_ip: String,
    pub(super) local_ip: String,
    pub(super) local_port: u16,
    pub(super) call_id: String,
    pub(super) local_tag: String,
    pub(super) remote_uri: String,
    /// Pre-formatted `;tag=...` (or empty), ready to interpolate directly.
    pub(super) remote_tag_param: String,
    pub(super) remote_via: String,
    pub(super) contact: SocketAddr,
    pub(super) via_proto: &'static str,
    pub(super) contact_transport: &'static str,
}

impl DialogRequestContext {
    pub(super) fn new(identity: &StackIdentity, dialog: &Dialog) -> Self {
        Self {
            server: identity.server.clone(),
            username: identity.username.clone(),
            display: identity.display.clone(),
            adv_ip: identity.adv_ip.clone(),
            local_ip: identity.local_ip.clone(),
            local_port: identity.local_port,
            call_id: dialog.call_id.clone(),
            local_tag: dialog.local_tag.clone(),
            remote_uri: dialog.remote_uri.clone(),
            remote_tag_param: dialog
                .remote_tag
                .as_deref()
                .map(|t| format!(";tag={t}"))
                .unwrap_or_default(),
            remote_via: dialog.remote_via.clone(),
            contact: dialog
                .remote_contact
                .as_deref()
                .and_then(|s| s.parse().ok())
                .unwrap_or(identity.server_addr),
            via_proto: identity.via_proto,
            contact_transport: identity.contact_transport,
        }
    }
}
