//! Call dialog lifecycle: the `Dialog` state itself, the `SipStack` methods
//! that drive it over the wire (INVITE/BYE/CANCEL/re-INVITE), and REFER-based
//! transfer.

pub mod dialog;
mod lifecycle;
mod transfer;
