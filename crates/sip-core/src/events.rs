use crate::presence::PresenceState;

/// Events emitted by the SIP stack to the application.
#[derive(Debug)]
pub enum SipEvent {
    Registered { expires: u32 },
    RegistrationFailed { reason: String },
    /// Remote party is ringing (180 received on outgoing call).
    CallRinging { call_id: String },
    /// Outgoing call answered; `remote_sdp` is the remote party's SDP answer.
    CallConnected { call_id: String, remote_sdp: String },
    /// Incoming INVITE arrived.
    IncomingCall { call_id: String, from: String, remote_sdp: String },
    CallEnded { call_id: String },
    CallFailed { call_id: String, code: u16, reason: String },
    /// Our hold re-INVITE was accepted — call is now on hold.
    CallHeld { call_id: String },
    /// Our resume re-INVITE was accepted — call is active again.
    CallResumed { call_id: String },
    /// Remote side put us on hold via re-INVITE.
    RemoteHeld { call_id: String },
    /// Remote side resumed us via re-INVITE.
    RemoteResumed { call_id: String },
    /// Our blind-transfer REFER was accepted (2xx) — the far end will
    /// typically send BYE on this dialog once the transferred call succeeds.
    TransferAccepted { call_id: String },
    /// Our blind-transfer REFER was rejected.
    TransferFailed { call_id: String, reason: String },
    /// Presence SUBSCRIBE accepted (200 OK); `expires` is the server-granted value.
    PresenceSubscribed { uri: String, expires: u32 },
    /// Presence SUBSCRIBE rejected.
    PresenceSubscribeFailed { uri: String, reason: String },
    /// A NOTIFY updated a watched contact's presence state.
    PresenceUpdate { uri: String, state: PresenceState },
}

/// Commands sent from the application into the SIP stack.
#[derive(Debug)]
pub enum SipCommand {
    MakeCall { to: String, local_sdp: String },
    AcceptCall { call_id: String, local_sdp: String },
    RejectCall { call_id: String },
    HangUp { call_id: String },
    /// Send a hold re-INVITE (a=sendonly).
    HoldCall { call_id: String, local_sdp: String },
    /// Send a resume re-INVITE (a=sendrecv).
    ResumeCall { call_id: String, local_sdp: String },
    /// Blind-transfer an active (Confirmed) call to `target` (a full SIP URI) via REFER.
    BlindTransfer { call_id: String, target: String },
    /// Redirect a not-yet-answered incoming call via 302 Moved Temporarily.
    RedirectCall { call_id: String, target: String },
    /// Subscribe to a contact's presence (`target_uri` is a full SIP URI).
    SubscribePresence { target_uri: String },
    /// Unsubscribe from a contact's presence (sends SUBSCRIBE with Expires: 0).
    UnsubscribePresence { target_uri: String },
}
