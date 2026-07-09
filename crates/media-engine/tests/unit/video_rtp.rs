use super::*;
use crate::video_codec::{H264Decoder, H264Encoder, Yuv420Frame};

/// Build a synthetic NAL unit: F=0, NRI=3 (0x60), the given type in the low
/// 5 bits, followed by `payload_len` bytes of non-repeating filler (so a
/// fragmentation bug that duplicates/drops/reorders bytes shows up as a
/// mismatch rather than accidentally still comparing equal).
fn make_nal(nal_type: u8, payload_len: usize) -> Vec<u8> {
    let mut nal = Vec::with_capacity(1 + payload_len);
    nal.push(0x60 | (nal_type & 0x1F));
    nal.extend((0..payload_len).map(|i| (i % 256) as u8));
    nal
}

fn annex_b(nals: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for nal in nals {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    }
    out
}

#[test]
fn small_nal_becomes_single_packet_with_no_fu_a_header() {
    let nal = make_nal(5, 50); // small IDR-slice-shaped NAL
    let bitstream = annex_b(std::slice::from_ref(&nal));
    let packets = fragment_nal_units(&bitstream, 1200);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0], nal);
    assert_ne!(packets[0][0] & 0x1F, FU_A_TYPE);
}

#[test]
fn large_nal_fragments_into_fu_a_and_reassembles_byte_identical() {
    let nal = make_nal(5, 5000); // bigger than any reasonable MTU
    let bitstream = annex_b(std::slice::from_ref(&nal));
    let packets = fragment_nal_units(&bitstream, 1200);
    assert!(packets.len() > 1, "expected multiple FU-A fragments");

    for p in &packets {
        assert_eq!(p[0] & 0x1F, FU_A_TYPE, "every fragment must be FU-A");
    }
    let first = packets.first().unwrap();
    let last = packets.last().unwrap();
    assert_ne!(first[1] & 0x80, 0, "first fragment must have the Start bit set");
    assert_eq!(first[1] & 0x40, 0, "first fragment must not have the End bit set");
    assert_eq!(last[1] & 0x80, 0, "last fragment must not have the Start bit set");
    assert_ne!(last[1] & 0x40, 0, "last fragment must have the End bit set");

    let reassembled = reassemble_nal_units(&packets);
    assert_eq!(reassembled, bitstream);
}

#[test]
fn multi_nal_buffer_preserves_order_and_boundaries() {
    let sps = make_nal(7, 20);
    let pps = make_nal(8, 10);
    let idr = make_nal(5, 3000); // forces FU-A for this one NAL only
    let bitstream = annex_b(&[sps.clone(), pps.clone(), idr.clone()]);

    let packets = fragment_nal_units(&bitstream, 1200);
    let reassembled = reassemble_nal_units(&packets);
    assert_eq!(reassembled, annex_b(&[sps, pps, idr]));
}

#[test]
fn encode_fragment_reassemble_decode_round_trip() {
    let mut encoder = H264Encoder::new(500_000).unwrap();
    let mut decoder = H264Decoder::new().unwrap();
    let frame = Yuv420Frame::solid_color(64, 64, 128, 128, 128);

    let bitstream = encoder.encode(&frame).unwrap();
    assert!(!bitstream.is_empty());

    let packets = fragment_nal_units(&bitstream, 1200);
    assert!(!packets.is_empty());
    let reassembled = reassemble_nal_units(&packets);
    assert_eq!(
        reassembled, bitstream,
        "fragment+reassemble must exactly reproduce the encoder's own bitstream"
    );

    // Decoding the reassembled bitstream must not error -- proves
    // `video_rtp`'s output is still valid H.264 that `video_codec`'s
    // decoder accepts, not just byte-identical to itself.
    decoder.decode(&reassembled).unwrap();
}
