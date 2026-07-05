use super::*;

#[test]
fn parse_xor_mapped_address() {
    // Craft a minimal Binding Response with XOR-MAPPED-ADDRESS for 203.0.113.1:12345
    let port_raw = 12345u16 ^ ((MAGIC_COOKIE >> 16) as u16);
    let ip0 = 203u8 ^ ((MAGIC_COOKIE >> 24) as u8);
    let ip1 = (MAGIC_COOKIE >> 16) as u8;
    let ip2 = 113u8 ^ ((MAGIC_COOKIE >> 8) as u8);
    let ip3 = 1u8 ^ MAGIC_COOKIE as u8;

    let mut buf = vec![0u8; 32];
    buf[0..2].copy_from_slice(&BINDING_SUCCESS.to_be_bytes());
    buf[2..4].copy_from_slice(&12u16.to_be_bytes()); // attr length total
    buf[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    // attribute: XOR-MAPPED-ADDRESS
    buf[20..22].copy_from_slice(&ATTR_XOR_MAPPED.to_be_bytes());
    buf[22..24].copy_from_slice(&8u16.to_be_bytes());
    buf[24] = 0x00;
    buf[25] = 0x01; // family = IPv4
    buf[26..28].copy_from_slice(&port_raw.to_be_bytes());
    buf[28] = ip0;
    buf[29] = ip1;
    buf[30] = ip2;
    buf[31] = ip3;

    let addr = parse_binding_response(&buf).unwrap();
    assert_eq!(addr.port(), 12345);
    assert_eq!(addr.ip().to_string(), "203.0.113.1");
}
