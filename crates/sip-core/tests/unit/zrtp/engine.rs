use std::collections::VecDeque;

use super::*;
use crate::zrtp::cache::MemorySharedSecretStore;

fn client_id(tag: &[u8; 6]) -> [u8; 16] {
    let mut id = [b' '; 16];
    id[..6].copy_from_slice(tag);
    id
}

/// Pumps `Send` events between two engines until both reach `Secure` (or
/// one reports `Failed`), collecting each side's `SasReady`/`SecureOn`
/// along the way. This is the only thing actually verified this session --
/// two instances of *this* implementation completing a handshake and
/// agreeing on keys/SAS with each other, not interop with any other ZRTP
/// implementation (see `wire.rs`'s and `engine.rs`'s module docs).
struct Handshake {
    initiator: ZrtpEngine<MemorySharedSecretStore>,
    responder: ZrtpEngine<MemorySharedSecretStore>,
    initiator_sas: Option<String>,
    responder_sas: Option<String>,
    initiator_secure: Option<SecureSession>,
    responder_secure: Option<SecureSession>,
}

impl Handshake {
    fn new() -> Self {
        Self {
            initiator: ZrtpEngine::new(
                Role::Initiator,
                [1; 12],
                client_id(b"AliceD"),
                MemorySharedSecretStore::default(),
            ),
            responder: ZrtpEngine::new(
                Role::Responder,
                [2; 12],
                client_id(b"BobDee"),
                MemorySharedSecretStore::default(),
            ),
            initiator_sas: None,
            responder_sas: None,
            initiator_secure: None,
            responder_secure: None,
        }
    }

    fn drain_events(&mut self, from_initiator: bool, events: Vec<EngineEvent>, queue: &mut VecDeque<(bool, Vec<u8>)>) {
        for event in events {
            match event {
                EngineEvent::Send(msg) => queue.push_back((from_initiator, msg.encode())),
                EngineEvent::SasReady(sas) => {
                    if from_initiator {
                        self.initiator_sas = Some(sas);
                    } else {
                        self.responder_sas = Some(sas);
                    }
                }
                EngineEvent::SecureOn(session) => {
                    if from_initiator {
                        self.initiator_secure = Some(session);
                    } else {
                        self.responder_secure = Some(session);
                    }
                }
                EngineEvent::Failed(reason) => panic!("engine reported failure: {reason}"),
            }
        }
    }

    fn run(&mut self) {
        let mut queue: VecDeque<(bool, Vec<u8>)> = VecDeque::new();
        let initiator_start = self.initiator.start();
        self.drain_events(true, initiator_start, &mut queue);
        let responder_start = self.responder.start();
        self.drain_events(false, responder_start, &mut queue);

        let mut guard = 0;
        while let Some((from_initiator, bytes)) = queue.pop_front() {
            guard += 1;
            assert!(guard < 100, "handshake did not converge");
            // A message sent BY the initiator is received BY the responder, and vice versa.
            let events = if from_initiator {
                self.responder.receive(&bytes).expect("responder receive should succeed")
            } else {
                self.initiator.receive(&bytes).expect("initiator receive should succeed")
            };
            self.drain_events(!from_initiator, events, &mut queue);
        }
    }
}

#[test]
fn full_handshake_reaches_secure_with_matching_sas() {
    let mut hs = Handshake::new();
    hs.run();

    assert_eq!(hs.initiator.state(), HandshakeState::Secure);
    assert_eq!(hs.responder.state(), HandshakeState::Secure);

    let initiator_sas = hs.initiator_sas.expect("initiator should have computed a SAS");
    let responder_sas = hs.responder_sas.expect("responder should have computed a SAS");
    assert_eq!(initiator_sas, responder_sas);
    assert_eq!(initiator_sas.len(), 4);
}

#[test]
fn full_handshake_derives_matching_srtp_keys() {
    let mut hs = Handshake::new();
    hs.run();

    let initiator_keys = hs.initiator_secure.expect("initiator should be secure");
    let responder_keys = hs.responder_secure.expect("responder should be secure");

    // Both sides must agree on the *same* four keys, just from opposite
    // send/receive perspectives (see `SecureSession::role`'s doc comment).
    assert_eq!(initiator_keys.srtp_key_i, responder_keys.srtp_key_i);
    assert_eq!(initiator_keys.srtp_salt_i, responder_keys.srtp_salt_i);
    assert_eq!(initiator_keys.srtp_key_r, responder_keys.srtp_key_r);
    assert_eq!(initiator_keys.srtp_salt_r, responder_keys.srtp_salt_r);
    assert_eq!(initiator_keys.role, Role::Initiator);
    assert_eq!(responder_keys.role, Role::Responder);

    // Initiator and responder keys must actually differ from each other
    // (distinct send/receive directions), not just be equal-by-coincidence.
    assert_ne!(initiator_keys.srtp_key_i, initiator_keys.srtp_key_r);
}

#[test]
fn tampered_dhpart2_breaks_hvi_verification() {
    // A bit-flipped DHPart2 must be rejected by the responder rather than
    // silently accepted -- this is the check that binds Commit's `hvi` to
    // the actual DH exchange.
    let mut initiator =
        ZrtpEngine::new(Role::Initiator, [1; 12], client_id(b"AliceD"), MemorySharedSecretStore::default());
    let mut responder =
        ZrtpEngine::new(Role::Responder, [2; 12], client_id(b"BobDee"), MemorySharedSecretStore::default());

    let hello_i = pop_send(initiator.start());
    let hello_r = pop_send(responder.start());
    let commit = pop_send(initiator.receive(&hello_r).unwrap());
    let _ = responder.receive(&hello_i).unwrap();
    let dhpart1 = pop_send(responder.receive(&commit).unwrap());
    let mut dhpart2 = pop_send(initiator.receive(&dhpart1).unwrap());

    // Flip a byte squarely inside the `pv` (DH public value) field --
    // avoiding `h1` (would fail hash-chain verification instead) and the
    // `pv` length prefix just before it (would fail decoding/truncation
    // instead). Layout: preamble(2) + length(2) + type(8) + h1(32) +
    // rs1_id/rs2_id/aux_id/pbx_id(8 each) + pv_len(2) + pv(64) + mac(8) + crc(4).
    let pv_start = 2 + 2 + 8 + 32 + 8 * 4 + 2;
    dhpart2[pv_start + 10] ^= 0x01;
    // Recompute the CRC so this fails on hvi, not on the CRC check.
    let crc_at = dhpart2.len() - 4;
    let crc = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC).checksum(&dhpart2[..crc_at]);
    dhpart2[crc_at..].copy_from_slice(&crc.to_be_bytes());

    let err = responder.receive(&dhpart2).unwrap_err();
    assert!(matches!(err, EngineError::HviMismatch));
}

fn pop_send(events: Vec<EngineEvent>) -> Vec<u8> {
    events
        .into_iter()
        .find_map(|e| match e {
            EngineEvent::Send(msg) => Some(msg.encode()),
            _ => None,
        })
        .expect("expected a Send event")
}
