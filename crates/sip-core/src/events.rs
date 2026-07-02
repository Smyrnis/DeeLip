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
}
