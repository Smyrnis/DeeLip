/// State of a SIP call dialog (simplified early/confirmed dialog).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogState {
    /// We sent INVITE or received INVITE; not yet confirmed.
    Calling,
    Ringing,
    Confirmed,
    Terminating,
    Terminated,
}

#[derive(Debug, Clone)]
pub struct Dialog {
    pub call_id:        String,
    pub local_tag:      String,
    pub remote_tag:     Option<String>,
    pub remote_uri:     String,
    pub remote_contact: Option<String>,
    pub local_cseq:     u32,
    pub remote_cseq:    Option<u32>,
    pub state:          DialogState,
    pub remote_sdp:     Option<String>,
    /// Last SDP we sent (needed to repeat in re-INVITE 200 OK).
    pub local_sdp:      Option<String>,
    /// Whether the call is currently on hold (our side initiated).
    pub is_held:        bool,
    /// Some(true) = hold re-INVITE pending; Some(false) = resume pending.
    pub hold_pending:   Option<bool>,
    /// Set once we've retried the initial INVITE with digest auth, so a second
    /// 401/407 (bad credentials) is treated as a final failure, not another retry.
    pub auth_retried:   bool,
}

impl Dialog {
    pub fn new_outgoing(call_id: String, local_tag: String, to_uri: String) -> Self {
        Self {
            call_id,
            local_tag,
            remote_tag:     None,
            remote_uri:     to_uri,
            remote_contact: None,
            local_cseq:     1,
            remote_cseq:    None,
            state:          DialogState::Calling,
            remote_sdp:     None,
            local_sdp:      None,
            is_held:        false,
            hold_pending:   None,
            auth_retried:   false,
        }
    }

    pub fn new_incoming(
        call_id:    String,
        local_tag:  String,
        from_uri:   String,
        from_tag:   String,
        remote_cseq: u32,
        remote_sdp: String,
    ) -> Self {
        Self {
            call_id,
            local_tag,
            remote_tag:     Some(from_tag),
            remote_uri:     from_uri,
            remote_contact: None,
            local_cseq:     0,
            remote_cseq:    Some(remote_cseq),
            state:          DialogState::Calling,
            remote_sdp:     Some(remote_sdp),
            local_sdp:      None,
            is_held:        false,
            hold_pending:   None,
            auth_retried:   false,
        }
    }

    pub fn next_local_cseq(&mut self) -> u32 {
        self.local_cseq += 1;
        self.local_cseq
    }
}
