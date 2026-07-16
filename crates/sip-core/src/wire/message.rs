/// A SIP request method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SipMethod {
    Register,
    Invite,
    Ack,
    Bye,
    Cancel,
    Options,
    Info,
    Notify,
    Subscribe,
    Refer,
    Message,
    Publish,
    Other(String),
}

impl SipMethod {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Register => "REGISTER",
            Self::Invite => "INVITE",
            Self::Ack => "ACK",
            Self::Bye => "BYE",
            Self::Cancel => "CANCEL",
            Self::Options => "OPTIONS",
            Self::Info => "INFO",
            Self::Notify => "NOTIFY",
            Self::Subscribe => "SUBSCRIBE",
            Self::Refer => "REFER",
            Self::Message => "MESSAGE",
            Self::Publish => "PUBLISH",
            Self::Other(s) => s.as_str(),
        }
    }
}

impl From<&str> for SipMethod {
    fn from(s: &str) -> Self {
        match s {
            "REGISTER" => Self::Register,
            "INVITE" => Self::Invite,
            "ACK" => Self::Ack,
            "BYE" => Self::Bye,
            "CANCEL" => Self::Cancel,
            "OPTIONS" => Self::Options,
            "INFO" => Self::Info,
            "NOTIFY" => Self::Notify,
            "SUBSCRIBE" => Self::Subscribe,
            "REFER" => Self::Refer,
            "MESSAGE" => Self::Message,
            "PUBLISH" => Self::Publish,
            other => Self::Other(other.to_string()),
        }
    }
}

/// First line of a SIP message.
#[derive(Debug, Clone)]
pub enum SipStartLine {
    Request { method: SipMethod, uri: String },
    Response { status: u16, reason: String },
}

/// A parsed SIP message (request or response).
#[derive(Debug, Clone)]
pub struct SipMessage {
    pub start_line: SipStartLine,
    /// Headers preserved in order; names are stored as-received (mixed case).
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl SipMessage {
    /// Parse raw bytes into a SipMessage. Returns `None` on malformed input.
    pub fn parse(data: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(data).ok()?;
        let mut lines = text.split("\r\n");

        // First line
        let first = lines.next()?;
        let start_line = if first.starts_with("SIP/2.0") {
            let mut parts = first.splitn(3, ' ');
            parts.next(); // "SIP/2.0"
            let status: u16 = parts.next()?.parse().ok()?;
            let reason = parts.next().unwrap_or("").to_string();
            SipStartLine::Response { status, reason }
        } else {
            let mut parts = first.splitn(3, ' ');
            let method: SipMethod = parts.next()?.into();
            let uri = parts.next()?.to_string();
            SipStartLine::Request { method, uri }
        };

        let mut headers: Vec<(String, String)> = Vec::new();
        let mut in_body = false;
        let mut body_parts: Vec<&str> = Vec::new();

        for line in lines {
            if in_body {
                body_parts.push(line);
                continue;
            }
            if line.is_empty() {
                in_body = true;
                continue;
            }
            // Handle header folding (RFC 3261 §7.3.1)
            if line.starts_with(' ') || line.starts_with('\t') {
                if let Some(last) = headers.last_mut() {
                    last.1.push(' ');
                    last.1.push_str(line.trim());
                }
                continue;
            }
            if let Some(pos) = line.find(':') {
                let name = line[..pos].trim().to_string();
                let value = line[pos + 1..].trim().to_string();
                headers.push((name, value));
            }
        }

        let body = body_parts.join("\r\n").into_bytes();
        Some(SipMessage { start_line, headers, body })
    }

    // ── Header accessors ──────────────────────────────────────────────────────

    /// Case-insensitive header lookup; returns the first match.
    pub fn header(&self, name: &str) -> Option<&str> {
        let lower = name.to_ascii_lowercase();
        self.headers.iter().find(|(k, _)| k.to_ascii_lowercase() == lower).map(|(_, v)| v.as_str())
    }

    /// All values for a header name (some headers repeat).
    pub fn headers_all(&self, name: &str) -> Vec<&str> {
        let lower = name.to_ascii_lowercase();
        self.headers.iter().filter(|(k, _)| k.to_ascii_lowercase() == lower).map(|(_, v)| v.as_str()).collect()
    }

    // ── Convenience getters ───────────────────────────────────────────────────

    pub fn status_code(&self) -> Option<u16> {
        match &self.start_line {
            SipStartLine::Response { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// The status line's reason phrase (e.g. "Not Acceptable Here" from
    /// `SIP/2.0 488 Not Acceptable Here`) — distinct from, and far more
    /// commonly populated than, the optional RFC 3326 `Reason:` header.
    pub fn reason_phrase(&self) -> Option<&str> {
        match &self.start_line {
            SipStartLine::Response { reason, .. } => Some(reason.as_str()),
            _ => None,
        }
    }

    pub fn method(&self) -> Option<&SipMethod> {
        match &self.start_line {
            SipStartLine::Request { method, .. } => Some(method),
            _ => None,
        }
    }

    pub fn call_id(&self) -> Option<&str> {
        self.header("Call-ID").or_else(|| self.header("i"))
    }

    pub fn cseq(&self) -> Option<(u32, SipMethod)> {
        let v = self.header("CSeq")?;
        let mut parts = v.splitn(2, ' ');
        let seq: u32 = parts.next()?.trim().parse().ok()?;
        let method: SipMethod = parts.next()?.trim().into();
        Some((seq, method))
    }
}

#[cfg(test)]
#[path = "../../tests/unit/message.rs"]
mod tests;
