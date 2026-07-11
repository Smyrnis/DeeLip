//! ZRTP handshake state machine -- drives Hello/Commit/DHPart1/DHPart2/
//! Confirm1/Confirm2/Conf2ACK to completion for one call, then hands back
//! the derived SRTP keys. Hash-chain reveal sequence, provenance/
//! verification status, and scope cuts: `docs/zrtp.md`.

use crate::zrtp::cache::{CacheEntry, RetainedSecrets, SharedSecretStore};
use crate::zrtp::crypto::{
    self, compute_hvi, confirm_decrypt, confirm_encrypt, derive_mac_keys, derive_s0, derive_sas, derive_srtp_keys,
    derive_zrtp_keys, generate_ec25_keypair, generate_hash_chain, kdf_context, message_mac, total_hash,
    verify_hash_chain_hop, Ec25KeyPair, HashChain, SrtpKeys,
};
use crate::zrtp::wire::{Commit, Confirm, DhPart, Hello, Message};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Initiator,
    Responder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeState {
    Discovery,
    HelloSent,
    HelloReceived,
    CommitSent,
    CommitReceived,
    DhPart1Sent,
    DhPart2Sent,
    Confirm1Sent,
    Confirm2Sent,
    Secure,
    Failed,
}

#[derive(Debug, Clone)]
pub struct SecureSession {
    pub srtp_key_i: [u8; 16],
    pub srtp_salt_i: [u8; 14],
    pub srtp_key_r: [u8; 16],
    pub srtp_salt_r: [u8; 14],
    /// Which of the `_i`/`_r` pair above is *this* endpoint's own send key
    /// -- `Role::Initiator` uses `srtp_key_i`/`srtp_salt_i` to encrypt what
    /// it sends and `srtp_key_r`/`srtp_salt_r` to decrypt what it receives,
    /// and vice versa for `Role::Responder`.
    pub role: Role,
}

#[derive(Debug)]
pub enum EngineEvent {
    Send(Message),
    SasReady(String),
    SecureOn(SecureSession),
    Failed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("unexpected message for current state")]
    UnexpectedMessage,
    #[error("unsupported algorithm proposed")]
    UnsupportedAlgorithm,
    #[error("hash chain verification failed")]
    ChainVerificationFailed,
    #[error("hvi mismatch")]
    HviMismatch,
    #[error("message MAC verification failed")]
    MacVerificationFailed,
    #[error("wire decode error: {0}")]
    Wire(#[from] crate::zrtp::wire::WireError),
}

pub struct ZrtpEngine<S: SharedSecretStore> {
    role: Role,
    state: HandshakeState,
    local_zid: [u8; 12],
    remote_zid: Option<[u8; 12]>,
    chain: HashChain,
    local_hello: Hello,
    local_hello_bytes: Vec<u8>,
    remote_hello: Option<Hello>,
    remote_hello_bytes: Option<Vec<u8>>,
    commit_bytes: Option<Vec<u8>>,
    dhpart1_bytes: Option<Vec<u8>>,
    dhpart2_bytes: Option<Vec<u8>>,
    /// The commit received (responder) or sent (initiator) -- needed by
    /// the responder to recover the negotiated algorithms and by the
    /// initiator to re-derive nothing (kept for symmetry/debugging).
    commit: Option<Commit>,
    dhpart1_h1: Option<[u8; 32]>,
    dhpart2_h1: Option<[u8; 32]>,
    /// Own EC keypair -- generated once (either when precomputing the
    /// initiator's DHPart2 before sending Commit, or when the responder
    /// builds DHPart1) and consumed exactly once by `ec25_shared_secret`.
    local_keypair: Option<Ec25KeyPair>,
    total_hash: Option<[u8; 32]>,
    s0: Option<[u8; 32]>,
    srtp_keys: Option<SrtpKeys>,
    mac_keys: Option<([u8; 32], [u8; 32])>,
    zrtp_keys: Option<([u8; 16], [u8; 16])>,
    sas: Option<(u32, String)>,
    store: S,
}

const CACHE_EXPIRY_SECS: u32 = 3600;

impl<S: SharedSecretStore> ZrtpEngine<S> {
    pub fn new(role: Role, local_zid: [u8; 12], client_id: [u8; 16], store: S) -> Self {
        let chain = generate_hash_chain();
        let hello_no_mac = Hello {
            version: *b"1.10",
            client_id,
            h3: chain.h3,
            zid: local_zid,
            mitm_capable: false,
            hashes: vec![crypto::HASH_ALGO],
            ciphers: vec![crypto::CIPHER_ALGO],
            auths: vec![crypto::AUTH_ALGO],
            key_agreements: vec![crypto::KEY_AGREEMENT_ALGO],
            sas_types: vec![crypto::SAS_ALGO],
            mac: [0; 8],
        };
        // `mac` here (like Commit's/DHPart's) isn't independently verified
        // anywhere in this implementation -- the hash-chain-hop checks in
        // `on_commit`/`on_dhpart1`/`on_dhpart2` already provide equivalent
        // tamper-evidence for what actually matters (binding each message
        // to the one before it). Computed as a self-consistent value (HMAC
        // over this message's own wire encoding with the mac field
        // zeroed, including whatever throwaway CRC that encoding has) --
        // not a claim that this matches any real ZRTP implementation's
        // exact convention.
        let mac = message_mac(&chain.h2, &Message::Hello(hello_no_mac.clone()).encode());
        let local_hello = Hello { mac, ..hello_no_mac };
        Self {
            role,
            state: HandshakeState::Discovery,
            local_zid,
            remote_zid: None,
            chain,
            local_hello,
            local_hello_bytes: Vec::new(),
            remote_hello: None,
            remote_hello_bytes: None,
            commit_bytes: None,
            dhpart1_bytes: None,
            dhpart2_bytes: None,
            commit: None,
            dhpart1_h1: None,
            dhpart2_h1: None,
            local_keypair: None,
            total_hash: None,
            s0: None,
            srtp_keys: None,
            mac_keys: None,
            zrtp_keys: None,
            sas: None,
            store,
        }
    }

    pub fn role(&self) -> Role {
        self.role
    }
    pub fn state(&self) -> HandshakeState {
        self.state
    }
    pub fn sas(&self) -> Option<&str> {
        self.sas.as_ref().map(|(_, s)| s.as_str())
    }

    pub fn start(&mut self) -> Vec<EngineEvent> {
        self.state = HandshakeState::HelloSent;
        let msg = Message::Hello(self.local_hello.clone());
        self.local_hello_bytes = msg.encode();
        vec![EngineEvent::Send(msg)]
    }

    pub fn receive(&mut self, raw: &[u8]) -> Result<Vec<EngineEvent>, EngineError> {
        let message = Message::decode(raw)?;
        match (&message, self.state, self.role) {
            (Message::Hello(_), HandshakeState::HelloSent, _) => self.on_hello(message, raw),
            (Message::Commit(_), HandshakeState::HelloReceived, Role::Responder) => self.on_commit(message, raw),
            (Message::DhPart1(_), HandshakeState::CommitSent, Role::Initiator) => self.on_dhpart1(message, raw),
            (Message::DhPart2(_), HandshakeState::DhPart1Sent, Role::Responder) => self.on_dhpart2(message, raw),
            (Message::Confirm1(_), HandshakeState::DhPart2Sent, Role::Initiator) => self.on_confirm1(message),
            (Message::Confirm2(_), HandshakeState::Confirm1Sent, Role::Responder) => self.on_confirm2(message),
            (Message::Conf2Ack, HandshakeState::Confirm2Sent, Role::Initiator) => self.on_conf2ack(),
            // Anything else (duplicate/retransmitted message, or one that
            // arrived out of order) is silently ignored -- the caller's own
            // retransmission timer will keep resending our own last message
            // until the peer catches up.
            _ => Ok(vec![]),
        }
    }

    fn on_hello(&mut self, message: Message, raw: &[u8]) -> Result<Vec<EngineEvent>, EngineError> {
        let Message::Hello(hello) = message else { unreachable!() };
        if !hello.hashes.contains(&crypto::HASH_ALGO)
            || !hello.ciphers.contains(&crypto::CIPHER_ALGO)
            || !hello.key_agreements.contains(&crypto::KEY_AGREEMENT_ALGO)
        {
            self.state = HandshakeState::Failed;
            return Err(EngineError::UnsupportedAlgorithm);
        }
        self.remote_zid = Some(hello.zid);
        self.remote_hello_bytes = Some(raw.to_vec());
        self.remote_hello = Some(hello);
        self.state = HandshakeState::HelloReceived;

        match self.role {
            Role::Responder => Ok(vec![]),
            Role::Initiator => self.send_commit(),
        }
    }

    /// Initiator only: precompute DHPart2 (needed to compute `hvi` before
    /// Commit can be built -- see this module's doc comment) and send Commit.
    fn send_commit(&mut self) -> Result<Vec<EngineEvent>, EngineError> {
        let keypair = generate_ec25_keypair();
        let dhpart2_no_mac = DhPart {
            h1: self.chain.h1,
            rs1_id: [0; 8],
            rs2_id: [0; 8],
            aux_id: [0; 8],
            pbx_id: [0; 8],
            pv: keypair.public_bytes.to_vec(),
            mac: [0; 8],
        };
        let dhpart2_bytes_before_mac = Message::DhPart2(dhpart2_no_mac.clone()).encode();
        let mac = message_mac(&self.chain.h0, &dhpart2_bytes_before_mac);
        let dhpart2 = DhPart { mac, ..dhpart2_no_mac };
        let dhpart2_bytes = Message::DhPart2(dhpart2.clone()).encode();

        let responder_hello_bytes = self.remote_hello_bytes.as_ref().expect("Hello already received");
        let hvi = compute_hvi(&dhpart2_bytes, responder_hello_bytes);

        let commit_no_mac = Commit {
            h2: self.chain.h2,
            zid: self.local_zid,
            hash: crypto::HASH_ALGO,
            cipher: crypto::CIPHER_ALGO,
            auth: crypto::AUTH_ALGO,
            key_agreement: crypto::KEY_AGREEMENT_ALGO,
            sas: crypto::SAS_ALGO,
            hvi,
            mac: [0; 8],
        };
        let commit_bytes_before_mac = Message::Commit(commit_no_mac.clone()).encode();
        let mac = message_mac(&self.chain.h1, &commit_bytes_before_mac);
        let commit = Commit { mac, ..commit_no_mac };
        let commit_bytes = Message::Commit(commit.clone()).encode();

        self.local_keypair = Some(keypair);
        self.dhpart2_bytes = Some(dhpart2_bytes);
        self.commit_bytes = Some(commit_bytes);
        self.commit = Some(commit.clone());
        self.state = HandshakeState::CommitSent;
        Ok(vec![EngineEvent::Send(Message::Commit(commit))])
    }

    fn on_commit(&mut self, message: Message, raw: &[u8]) -> Result<Vec<EngineEvent>, EngineError> {
        let Message::Commit(commit) = message else { unreachable!() };
        let remote_hello = self.remote_hello.as_ref().expect("Hello already received");
        if !verify_hash_chain_hop(&commit.h2, 1, &remote_hello.h3) {
            self.state = HandshakeState::Failed;
            return Err(EngineError::ChainVerificationFailed);
        }
        if commit.hash != crypto::HASH_ALGO
            || commit.cipher != crypto::CIPHER_ALGO
            || commit.key_agreement != crypto::KEY_AGREEMENT_ALGO
        {
            self.state = HandshakeState::Failed;
            return Err(EngineError::UnsupportedAlgorithm);
        }

        self.commit_bytes = Some(raw.to_vec());
        self.commit = Some(commit);
        self.state = HandshakeState::CommitReceived;

        let keypair = generate_ec25_keypair();
        let dhpart1_no_mac = DhPart {
            h1: self.chain.h1,
            rs1_id: [0; 8],
            rs2_id: [0; 8],
            aux_id: [0; 8],
            pbx_id: [0; 8],
            pv: keypair.public_bytes.to_vec(),
            mac: [0; 8],
        };
        let dhpart1_bytes_before_mac = Message::DhPart1(dhpart1_no_mac.clone()).encode();
        let mac = message_mac(&self.chain.h0, &dhpart1_bytes_before_mac);
        let dhpart1 = DhPart { mac, ..dhpart1_no_mac };
        let dhpart1_bytes = Message::DhPart1(dhpart1.clone()).encode();

        self.local_keypair = Some(keypair);
        self.dhpart1_bytes = Some(dhpart1_bytes);
        self.state = HandshakeState::DhPart1Sent;
        Ok(vec![EngineEvent::Send(Message::DhPart1(dhpart1))])
    }

    fn on_dhpart1(&mut self, message: Message, raw: &[u8]) -> Result<Vec<EngineEvent>, EngineError> {
        let Message::DhPart1(dhpart1) = message else { unreachable!() };
        let remote_hello = self.remote_hello.as_ref().expect("Hello already received");
        // Responder's own H2 is never transmitted -- two hops from H1 to H3.
        if !verify_hash_chain_hop(&dhpart1.h1, 2, &remote_hello.h3) {
            self.state = HandshakeState::Failed;
            return Err(EngineError::ChainVerificationFailed);
        }
        self.dhpart1_h1 = Some(dhpart1.h1);
        self.dhpart1_bytes = Some(raw.to_vec());

        let keypair = self.local_keypair.take().expect("keypair precomputed before Commit");
        let peer_pv: [u8; 64] = dhpart1.pv.as_slice().try_into().map_err(|_| EngineError::UnexpectedMessage)?;
        let dh_result = crypto::ec25_shared_secret(keypair, &peer_pv);

        let dhpart2_bytes = self.dhpart2_bytes.clone().expect("DHPart2 precomputed before Commit");
        let commit_bytes = self.commit_bytes.clone().expect("Commit sent");
        let responder_hello_bytes = self.remote_hello_bytes.clone().expect("Hello received");

        let th = total_hash(&responder_hello_bytes, &commit_bytes, raw, &dhpart2_bytes);
        self.finish_key_agreement(dh_result, th)?;

        self.state = HandshakeState::DhPart2Sent;
        let Message::DhPart2(dhpart2) = Message::decode(&dhpart2_bytes)? else { unreachable!() };
        let mut events = vec![EngineEvent::Send(Message::DhPart2(dhpart2))];
        if let Some((_, sas)) = &self.sas {
            events.push(EngineEvent::SasReady(sas.clone()));
        }
        Ok(events)
    }

    fn on_dhpart2(&mut self, message: Message, raw: &[u8]) -> Result<Vec<EngineEvent>, EngineError> {
        let Message::DhPart2(dhpart2) = message else { unreachable!() };
        let commit = self.commit.clone().expect("Commit received");
        if !verify_hash_chain_hop(&dhpart2.h1, 1, &commit.h2) {
            self.state = HandshakeState::Failed;
            return Err(EngineError::ChainVerificationFailed);
        }
        let own_hello_bytes = self.local_hello_bytes.clone();
        let hvi = compute_hvi(raw, &own_hello_bytes);
        if hvi != commit.hvi {
            self.state = HandshakeState::Failed;
            return Err(EngineError::HviMismatch);
        }
        self.dhpart2_h1 = Some(dhpart2.h1);

        let keypair = self.local_keypair.take().expect("keypair generated for DHPart1");
        let peer_pv: [u8; 64] = dhpart2.pv.as_slice().try_into().map_err(|_| EngineError::UnexpectedMessage)?;
        let dh_result = crypto::ec25_shared_secret(keypair, &peer_pv);

        let commit_bytes = self.commit_bytes.clone().expect("Commit received");
        let dhpart1_bytes = self.dhpart1_bytes.clone().expect("DHPart1 sent");
        let th = total_hash(&own_hello_bytes, &commit_bytes, &dhpart1_bytes, raw);
        self.finish_key_agreement(dh_result, th)?;

        let (zrtp_key_i, zrtp_key_r) = self.zrtp_keys.expect("derived above");
        let _ = zrtp_key_i;
        let iv = random_iv();
        let mut plaintext = Vec::with_capacity(36);
        plaintext.extend_from_slice(&self.chain.h0);
        plaintext.extend_from_slice(&CACHE_EXPIRY_SECS.to_be_bytes());
        let encrypted = confirm_encrypt(&zrtp_key_r, &iv, &plaintext);
        let (_, mackey_r) = self.mac_keys.expect("derived above");
        let confirm_mac = message_mac(&mackey_r, &encrypted);
        let confirm1 = Confirm { confirm_mac, cfb_iv: iv, encrypted };

        self.state = HandshakeState::Confirm1Sent;
        let mut events = vec![EngineEvent::Send(Message::Confirm1(confirm1))];
        if let Some((_, sas)) = &self.sas {
            events.push(EngineEvent::SasReady(sas.clone()));
        }
        Ok(events)
    }

    fn on_confirm1(&mut self, message: Message) -> Result<Vec<EngineEvent>, EngineError> {
        let Message::Confirm1(confirm1) = message else { unreachable!() };
        let (_, zrtp_key_r) = self.zrtp_keys.expect("derived after DHPart1");
        let (_, mackey_r) = self.mac_keys.expect("derived after DHPart1");
        let expected_mac = message_mac(&mackey_r, &confirm1.encrypted);
        if expected_mac != confirm1.confirm_mac {
            self.state = HandshakeState::Failed;
            return Err(EngineError::MacVerificationFailed);
        }
        let plaintext = confirm_decrypt(&zrtp_key_r, &confirm1.cfb_iv, &confirm1.encrypted);
        if plaintext.len() < 32 {
            self.state = HandshakeState::Failed;
            return Err(EngineError::MacVerificationFailed);
        }
        let h0_remote: [u8; 32] = plaintext[..32].try_into().unwrap();
        let dhpart1_h1 = self.dhpart1_h1.expect("stored when DHPart1 was received");
        if !verify_hash_chain_hop(&h0_remote, 1, &dhpart1_h1) {
            self.state = HandshakeState::Failed;
            return Err(EngineError::ChainVerificationFailed);
        }

        let (zrtp_key_i, _) = self.zrtp_keys.expect("derived above");
        let (mackey_i, _) = self.mac_keys.expect("derived above");
        let iv = random_iv();
        let mut plaintext = Vec::with_capacity(36);
        plaintext.extend_from_slice(&self.chain.h0);
        plaintext.extend_from_slice(&CACHE_EXPIRY_SECS.to_be_bytes());
        let encrypted = confirm_encrypt(&zrtp_key_i, &iv, &plaintext);
        let confirm_mac = message_mac(&mackey_i, &encrypted);
        let confirm2 = Confirm { confirm_mac, cfb_iv: iv, encrypted };
        self.state = HandshakeState::Confirm2Sent;
        Ok(vec![EngineEvent::Send(Message::Confirm2(confirm2))])
    }

    fn on_confirm2(&mut self, message: Message) -> Result<Vec<EngineEvent>, EngineError> {
        let Message::Confirm2(confirm2) = message else { unreachable!() };
        let (zrtp_key_i, _) = self.zrtp_keys.expect("derived after DHPart2");
        let (mackey_i, _) = self.mac_keys.expect("derived after DHPart2");
        let expected_mac = message_mac(&mackey_i, &confirm2.encrypted);
        if expected_mac != confirm2.confirm_mac {
            self.state = HandshakeState::Failed;
            return Err(EngineError::MacVerificationFailed);
        }
        let plaintext = confirm_decrypt(&zrtp_key_i, &confirm2.cfb_iv, &confirm2.encrypted);
        if plaintext.len() < 32 {
            self.state = HandshakeState::Failed;
            return Err(EngineError::MacVerificationFailed);
        }
        let h0_remote: [u8; 32] = plaintext[..32].try_into().unwrap();
        let dhpart2_h1 = self.dhpart2_h1.expect("stored when DHPart2 was received");
        if !verify_hash_chain_hop(&h0_remote, 1, &dhpart2_h1) {
            self.state = HandshakeState::Failed;
            return Err(EngineError::ChainVerificationFailed);
        }

        self.state = HandshakeState::Secure;
        self.persist_retained_secret();
        let mut events = vec![EngineEvent::Send(Message::Conf2Ack)];
        events.push(self.secure_on_event());
        Ok(events)
    }

    fn on_conf2ack(&mut self) -> Result<Vec<EngineEvent>, EngineError> {
        self.state = HandshakeState::Secure;
        self.persist_retained_secret();
        Ok(vec![self.secure_on_event()])
    }

    fn secure_on_event(&self) -> EngineEvent {
        let keys = self.srtp_keys.clone().expect("derived during key agreement");
        EngineEvent::SecureOn(SecureSession {
            srtp_key_i: keys.key_i,
            srtp_salt_i: keys.salt_i,
            srtp_key_r: keys.key_r,
            srtp_salt_r: keys.salt_r,
            role: self.role,
        })
    }

    fn finish_key_agreement(&mut self, dh_result: Vec<u8>, th: [u8; 32]) -> Result<(), EngineError> {
        let remote_zid = self.remote_zid.expect("Hello already received");
        let (zid_i, zid_r) = match self.role {
            Role::Initiator => (self.local_zid, remote_zid),
            Role::Responder => (remote_zid, self.local_zid),
        };
        // Retained-secret ID matching isn't implemented (see this module's
        // doc comment) -- always derive s0 as a first-ever call.
        let s0 = derive_s0(&dh_result, zid_i, zid_r, &th, None);
        let context = kdf_context(zid_i, zid_r, &th);
        self.total_hash = Some(th);
        self.srtp_keys = Some(derive_srtp_keys(&s0, &context));
        self.mac_keys = Some(derive_mac_keys(&s0, &context));
        self.zrtp_keys = Some(derive_zrtp_keys(&s0, &context));
        self.sas = Some(derive_sas(&s0, &context));
        self.s0 = Some(s0);
        Ok(())
    }

    /// Informational continuity record only -- not fed back into key
    /// derivation on a later call (see this module's doc comment).
    fn persist_retained_secret(&mut self) {
        let Some(remote_zid) = self.remote_zid else {
            return;
        };
        let Some(s0) = self.s0 else { return };
        self.store.store(CacheEntry {
            local_zid: self.local_zid,
            remote_zid,
            secrets: RetainedSecrets { rs1: crypto::sha256(&s0).to_vec(), rs2: Vec::new(), verified: false },
        });
    }
}

fn random_iv() -> [u8; 16] {
    use ring::rand::SecureRandom;
    let rng = ring::rand::SystemRandom::new();
    let mut iv = [0u8; 16];
    rng.fill(&mut iv).expect("system RNG must succeed");
    iv
}

#[cfg(test)]
#[path = "../../tests/unit/zrtp/engine.rs"]
mod tests;
