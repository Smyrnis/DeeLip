use super::*;

#[test]
fn build_query_encodes_name_and_qtype() {
    let packet = build_query(0x1234, "_sip._udp.example.com", QTYPE_SRV);
    assert_eq!(&packet[0..2], &[0x12, 0x34]); // ID
    assert_eq!(&packet[4..6], &[0, 1]); // QDCOUNT = 1
                                        // Question starts at byte 12: len-prefixed labels, then QTYPE/QCLASS.
    assert_eq!(packet[12], 4); // "_sip"
    assert_eq!(&packet[13..17], b"_sip");
    let qtype_offset = packet.len() - 4;
    assert_eq!(&packet[qtype_offset..qtype_offset + 2], &QTYPE_SRV.to_be_bytes());
    assert_eq!(&packet[qtype_offset + 2..], &1u16.to_be_bytes()); // QCLASS=IN
}

#[test]
fn parse_response_rejects_mismatched_id() {
    let mut resp = vec![0u8; 12];
    resp[0..2].copy_from_slice(&0xAAAAu16.to_be_bytes());
    assert!(parse_response(&resp, 0x1111, QTYPE_A).is_err());
}

#[test]
fn parse_response_extracts_a_record() {
    let mut resp = build_query(0x55, "host.example.com", QTYPE_A);
    // Flip QR bit to mark this as a response before appending the answer.
    resp[2] |= 0x80;
    resp[6..8].copy_from_slice(&1u16.to_be_bytes()); // ANCOUNT = 1
                                                     // Answer: pointer to the question's name (offset 12), TYPE=A, CLASS=IN, TTL, RDLENGTH=4, RDATA.
    resp.extend_from_slice(&[0xC0, 0x0C]);
    resp.extend_from_slice(&QTYPE_A.to_be_bytes());
    resp.extend_from_slice(&1u16.to_be_bytes());
    resp.extend_from_slice(&300u32.to_be_bytes());
    resp.extend_from_slice(&4u16.to_be_bytes());
    resp.extend_from_slice(&[203, 0, 113, 42]);

    let answers = parse_response(&resp, 0x55, QTYPE_A).unwrap();
    assert_eq!(answers.len(), 1);
    match &answers[0] {
        Answer::Addr(ip) => assert_eq!(*ip, IpAddr::from([203, 0, 113, 42])),
        _ => panic!("expected an A answer"),
    }
}

#[test]
fn parse_response_extracts_srv_record_with_compressed_target() {
    let mut resp = build_query(0x66, "_sip._udp.example.com", QTYPE_SRV);
    resp[2] |= 0x80;
    resp[6..8].copy_from_slice(&1u16.to_be_bytes());
    resp.extend_from_slice(&[0xC0, 0x0C]); // name: pointer to question
    resp.extend_from_slice(&QTYPE_SRV.to_be_bytes());
    resp.extend_from_slice(&1u16.to_be_bytes());
    resp.extend_from_slice(&300u32.to_be_bytes());
    // RDATA: priority=10, weight=0, port=5060, target="sip1" + pointer to "example.com" inside the question name.
    let target_label_offset = 12 + 1 + 4 + 1 + 4; // past "_sip"(4) and "_udp"(4) labels to "example"
    let mut rdata = Vec::new();
    rdata.extend_from_slice(&10u16.to_be_bytes());
    rdata.extend_from_slice(&0u16.to_be_bytes());
    rdata.extend_from_slice(&5060u16.to_be_bytes());
    rdata.push(4);
    rdata.extend_from_slice(b"sip1");
    rdata.extend_from_slice(&[0xC0, target_label_offset as u8]);
    resp.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
    resp.extend_from_slice(&rdata);

    let answers = parse_response(&resp, 0x66, QTYPE_SRV).unwrap();
    assert_eq!(answers.len(), 1);
    match &answers[0] {
        Answer::Srv(srv) => {
            assert_eq!(srv.priority, 10);
            assert_eq!(srv.port, 5060);
            assert_eq!(srv.target, "sip1.example.com");
        }
        _ => panic!("expected an SRV answer"),
    }
}

#[test]
fn srv_service_name_picks_transport_specific_prefix() {
    assert_eq!(srv_service_name("pbx.example.com", TransportProtocol::Udp), "_sip._udp.pbx.example.com");
    assert_eq!(srv_service_name("pbx.example.com", TransportProtocol::Tcp), "_sip._tcp.pbx.example.com");
    assert_eq!(srv_service_name("pbx.example.com", TransportProtocol::Tls), "_sips._tcp.pbx.example.com");
}

#[test]
fn parse_nameserver_accepts_bare_ip_and_socket_addr() {
    assert_eq!(parse_nameserver("1.1.1.1"), Some("1.1.1.1:53".parse().unwrap()));
    assert_eq!(parse_nameserver("9.9.9.9:5353"), Some("9.9.9.9:5353".parse().unwrap()));
    assert_eq!(parse_nameserver("not-an-ip"), None);
}
