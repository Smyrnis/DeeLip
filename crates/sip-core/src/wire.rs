//! Low-level SIP wire protocol: message parsing/building, SDP, digest auth,
//! and stream framing. Zero dependency on call dialogs or subscriptions --
//! everything else in this crate is built on top of this layer.

pub mod auth;
pub mod message;
pub mod sdp;
pub mod util;
pub(crate) mod framing;
