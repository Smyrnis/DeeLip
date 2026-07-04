use tokio::sync::mpsc;

use crate::events::{SipCommand, SipEvent};

/// The public, cheaply-cloneable-by-reference command/event façade a caller
/// (the `ui` crate) holds for one registered SIP identity — the `SipStack`
/// itself lives entirely on its own background task.
pub struct SipHandle {
    pub event_rx:      mpsc::UnboundedReceiver<SipEvent>,
    pub cmd_tx:        mpsc::UnboundedSender<SipCommand>,
    /// IP advertised in Contact and SDP (may be external if STUN succeeded).
    pub advertised_ip: String,
    /// True when signaling runs over TLS — callers use this to decide whether to offer SRTP.
    pub secure: bool,
    /// The account's SIP domain (`account.server`) — used to resolve bare
    /// extension numbers typed into the dialer into a full SIP URI.
    pub domain: String,
}

impl SipHandle {
    pub fn make_call(&self, to: &str, local_sdp: String) {
        let _ = self.cmd_tx.send(SipCommand::MakeCall { to: to.to_string(), local_sdp });
    }
    pub fn accept_call(&self, call_id: &str, local_sdp: String) {
        let _ = self.cmd_tx.send(SipCommand::AcceptCall {
            call_id: call_id.to_string(), local_sdp,
        });
    }
    pub fn reject_call(&self, call_id: &str) {
        let _ = self.cmd_tx.send(SipCommand::RejectCall { call_id: call_id.to_string() });
    }
    pub fn hang_up(&self, call_id: &str) {
        let _ = self.cmd_tx.send(SipCommand::HangUp { call_id: call_id.to_string() });
    }
    pub fn hold_call(&self, call_id: &str, local_sdp: String) {
        let _ = self.cmd_tx.send(SipCommand::HoldCall {
            call_id: call_id.to_string(), local_sdp,
        });
    }
    pub fn resume_call(&self, call_id: &str, local_sdp: String) {
        let _ = self.cmd_tx.send(SipCommand::ResumeCall {
            call_id: call_id.to_string(), local_sdp,
        });
    }
    /// `target` must already be a fully-qualified SIP URI (e.g. from
    /// `normalize_target`) — it's placed verbatim into the Refer-To header.
    pub fn blind_transfer(&self, call_id: &str, target: String) {
        let _ = self.cmd_tx.send(SipCommand::BlindTransfer {
            call_id: call_id.to_string(), target,
        });
    }
    /// `target` must already be a fully-qualified SIP URI.
    pub fn redirect_call(&self, call_id: &str, target: String) {
        let _ = self.cmd_tx.send(SipCommand::RedirectCall {
            call_id: call_id.to_string(), target,
        });
    }
    /// Subscribe to a contact's presence. `target_uri` must already be a
    /// fully-qualified SIP URI (contacts store one directly, same as Call does).
    pub fn subscribe_presence(&self, target_uri: String) {
        let _ = self.cmd_tx.send(SipCommand::SubscribePresence { target_uri });
    }
    pub fn unsubscribe_presence(&self, target_uri: String) {
        let _ = self.cmd_tx.send(SipCommand::UnsubscribePresence { target_uri });
    }
    /// Subscribe to a mailbox's voicemail MWI state. `target_uri` must
    /// already be a fully-qualified SIP URI.
    pub fn subscribe_mwi(&self, target_uri: String) {
        let _ = self.cmd_tx.send(SipCommand::SubscribeMwi { target_uri });
    }
    /// Attended-transfer `call_id` via REFER with a `Replaces` parameter
    /// referencing `consultation_call_id`'s dialog.
    pub fn attended_transfer(&self, call_id: &str, consultation_call_id: &str) {
        let _ = self.cmd_tx.send(SipCommand::AttendedTransfer {
            call_id: call_id.to_string(), consultation_call_id: consultation_call_id.to_string(),
        });
    }
    /// Send one DTMF digit via SIP INFO instead of RFC 2833 RTP events.
    pub fn send_dtmf_info(&self, call_id: &str, digit: char) {
        let _ = self.cmd_tx.send(SipCommand::SendDtmfInfo {
            call_id: call_id.to_string(), digit,
        });
    }
    /// Send a standalone SIP MESSAGE (RFC 3428) to `to` (a full SIP URI).
    pub fn send_message(&self, to: &str, body: &str) {
        let _ = self.cmd_tx.send(SipCommand::SendMessage {
            to: to.to_string(), body: body.to_string(),
        });
    }
}
