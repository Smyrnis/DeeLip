//! Content-Length-based message boundary detection for stream transports (TCP/TLS).
//! UDP doesn't need this — one datagram is always exactly one SIP message.

pub(crate) struct MessageFramer {
    buf: Vec<u8>,
}

impl MessageFramer {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// Pops one complete raw message (headers+body) if fully buffered, advancing
    /// past it. Call in a loop after each `push` to drain pipelined messages.
    pub fn try_take_message(&mut self) -> Option<Vec<u8>> {
        let header_end = find_header_block_end(&self.buf)?;
        let header_text = std::str::from_utf8(&self.buf[..header_end]).ok()?;
        let content_length = extract_content_length(header_text);
        let total_len = header_end + content_length;
        if self.buf.len() < total_len {
            return None;
        }
        Some(self.buf.drain(..total_len).collect())
    }
}

/// Index just after the header/body-separating "\r\n\r\n", if present.
fn find_header_block_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

/// Scans a header block for Content-Length (or its compact form "l"), case-insensitively.
fn extract_content_length(header_block: &str) -> usize {
    for line in header_block.split("\r\n") {
        let lower = line.to_ascii_lowercase();
        let value = lower.strip_prefix("content-length:").or_else(|| lower.strip_prefix("l:"));
        if let Some(v) = value {
            if let Ok(n) = v.trim().parse::<usize>() {
                return n;
            }
        }
    }
    0
}

#[cfg(test)]
#[path = "../../tests/unit/framing.rs"]
mod tests;
