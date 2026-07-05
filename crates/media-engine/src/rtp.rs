/// Minimal RTP packet implementation (RFC 3550).
/// Handles fixed 12-byte header; no CSRC, no extension.

pub const RTP_VERSION: u8 = 2;
pub const RTP_HEADER_SIZE: usize = 12;

#[derive(Debug, Clone)]
pub struct RtpPacket {
    pub payload_type: u8,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub marker: bool,
    pub payload: Vec<u8>,
}

impl RtpPacket {
    pub fn new(
        payload_type: u8,
        sequence: u16,
        timestamp: u32,
        ssrc: u32,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            payload_type,
            sequence,
            timestamp,
            ssrc,
            marker: false,
            payload,
        }
    }

    /// Encode to wire format.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(RTP_HEADER_SIZE + self.payload.len());

        // Byte 0: V=2, P=0, X=0, CC=0
        buf.push(RTP_VERSION << 6);
        // Byte 1: M bit + PT
        let marker_bit = if self.marker { 0x80u8 } else { 0u8 };
        buf.push(marker_bit | (self.payload_type & 0x7F));
        // Bytes 2-3: sequence number
        buf.push((self.sequence >> 8) as u8);
        buf.push(self.sequence as u8);
        // Bytes 4-7: timestamp
        buf.extend_from_slice(&self.timestamp.to_be_bytes());
        // Bytes 8-11: SSRC
        buf.extend_from_slice(&self.ssrc.to_be_bytes());
        // Payload
        buf.extend_from_slice(&self.payload);
        buf
    }

    /// Decode from wire bytes.  Returns `None` on malformed input.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < RTP_HEADER_SIZE {
            return None;
        }

        let version = (data[0] >> 6) & 0x03;
        if version != RTP_VERSION {
            return None;
        }

        let cc = (data[0] & 0x0F) as usize; // CSRC count
        let x_bit = (data[0] >> 4) & 0x01;
        let marker = (data[1] & 0x80) != 0;
        let pt = data[1] & 0x7F;
        let seq = u16::from_be_bytes([data[2], data[3]]);
        let ts = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let ssrc = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

        // Skip CSRC list
        let mut offset = RTP_HEADER_SIZE + cc * 4;
        // Skip extension header
        if x_bit != 0 && data.len() >= offset + 4 {
            let ext_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
            offset += 4 + ext_len * 4;
        }
        if offset > data.len() {
            return None;
        }

        Some(RtpPacket {
            payload_type: pt,
            sequence: seq,
            timestamp: ts,
            ssrc,
            marker,
            payload: data[offset..].to_vec(),
        })
    }
}

// ── Packet sender/receiver state ──────────────────────────────────────────────

pub struct RtpSender {
    pub payload_type: u8,
    pub ssrc: u32,
    pub sequence: u16,
    pub timestamp: u32,
    /// Timestamp increment per 20ms frame at 8000 Hz = 160 samples.
    pub ts_increment: u32,
}

impl RtpSender {
    /// `ts_increment` is the per-packet RTP timestamp step, in units of the
    /// codec's declared RTP clock rate (e.g. 160 for G.711 @8000 Hz/20ms,
    /// 960 for Opus @48000 Hz/20ms — the Opus RTP clock is always 48000
    /// regardless of the audio's actual sample rate, per RFC 7587).
    pub fn new(payload_type: u8, ts_increment: u32) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let ssrc = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        Self {
            payload_type,
            ssrc,
            sequence: 0,
            timestamp: 0,
            ts_increment,
        }
    }

    pub fn next_packet(&mut self, payload: Vec<u8>) -> RtpPacket {
        self.next_packet_with_pt(self.payload_type, payload)
    }

    /// Same as `next_packet`, but with an explicit payload-type override --
    /// used to send a comfort-noise (RFC 3389) packet on its own PT while
    /// still advancing this sender's shared sequence/timestamp state.
    pub fn next_packet_with_pt(&mut self, pt: u8, payload: Vec<u8>) -> RtpPacket {
        let pkt = RtpPacket::new(pt, self.sequence, self.timestamp, self.ssrc, payload);
        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(self.ts_increment);
        pkt
    }

    /// Advance the RTP clock by one tick (`ts_increment`) without sending a
    /// packet -- used during VAD-detected silence when neither a real audio
    /// nor a comfort-noise packet is being sent for this particular 20ms
    /// tick, so the timestamp on the next packet that IS sent still
    /// correctly reflects how much real time has elapsed. RFC 3550 requires
    /// the timestamp to track the sampling instant, not the count of
    /// packets actually transmitted -- sequence-number gaps during DTX are
    /// normal and receivers must tolerate them, but the timestamp must never
    /// silently fall behind.
    pub fn skip_tick(&mut self) {
        self.timestamp = self.timestamp.wrapping_add(self.ts_increment);
    }
}
