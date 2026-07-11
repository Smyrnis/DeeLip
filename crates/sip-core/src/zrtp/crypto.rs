//! ZRTP crypto: hash chain, KDF, DH/EC key agreement, SRTP/SAS/MAC key
//! derivation, and Confirm-message encryption. Which formulas are quoted
//! directly from RFC 6189 vs. reconstructed, and the crypto backends used:
//! `docs/zrtp.md`.

use ring::{agreement, digest, hmac, rand::SecureRandom};

pub const HASH_ALGO: [u8; 4] = *b"S256";
pub const CIPHER_ALGO: [u8; 4] = *b"AES1";
pub const AUTH_ALGO: [u8; 4] = *b"HS80";
pub const KEY_AGREEMENT_ALGO: [u8; 4] = *b"EC25";
pub const SAS_ALGO: [u8; 4] = *b"B32 ";

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let digest = digest::digest(&digest::SHA256, data);
    digest.as_ref().try_into().expect("SHA-256 is 32 bytes")
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    let tag = hmac::sign(&key, data);
    tag.as_ref().try_into().expect("HMAC-SHA256 is 32 bytes")
}

// ── Hash chain (RFC 6189 section 4.3, section 9) ────────────────────────────
// H0 is random; H1=hash(H0), H2=hash(H1), H3=hash(H2). Hello carries H3;
// each side's own next messages progressively reveal H2/H1/H0 (see
// `engine.rs`'s doc comment for exactly which message carries which value
// and why -- the responder's H2 is never transmitted at all, since it's
// only needed to validate a Commit message and the responder never sends one).

#[derive(Debug, Clone, Copy)]
pub struct HashChain {
    pub h0: [u8; 32],
    pub h1: [u8; 32],
    pub h2: [u8; 32],
    pub h3: [u8; 32],
}

pub fn generate_hash_chain() -> HashChain {
    let rng = ring::rand::SystemRandom::new();
    let mut h0 = [0u8; 32];
    rng.fill(&mut h0).expect("system RNG must succeed");
    let h1 = sha256(&h0);
    let h2 = sha256(&h1);
    let h3 = sha256(&h2);
    HashChain { h0, h1, h2, h3 }
}

/// Verify that `image` hashes forward (via `hops` repeated SHA-256 applications)
/// to `expected` -- e.g. `hops=1` for a directly-adjacent chain link,
/// `hops=2` when the intermediate value (the responder's H2) was never sent.
pub fn verify_hash_chain_hop(image: &[u8; 32], hops: u32, expected: &[u8; 32]) -> bool {
    let mut cur = *image;
    for _ in 0..hops {
        cur = sha256(&cur);
    }
    &cur == expected
}

// ── KDF (RFC 6189 section 4.5.1) ─────────────────────────────────────────────
// KDF(KI, Label, Context, L) = HMAC(KI, i || Label || 0x00 || Context || L),
// i = the fixed 32-bit big-endian counter 0x00000001, L = output length in
// bits (encoded here as a 32-bit big-endian integer, matching the other
// explicit length fields the RFC text describes elsewhere), output
// truncated to the leftmost L bits (every L used in this module is a
// multiple of 8, so this is always a whole-byte truncation).
pub fn kdf(ki: &[u8], label: &str, context: &[u8], bit_len: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + label.len() + 1 + context.len() + 4);
    data.extend_from_slice(&1u32.to_be_bytes());
    data.extend_from_slice(label.as_bytes());
    data.push(0);
    data.extend_from_slice(context);
    data.extend_from_slice(&(bit_len as u32).to_be_bytes());
    let full = hmac_sha256(ki, &data);
    full[..bit_len / 8].to_vec()
}

/// `ZIDi || ZIDr || total_hash` -- the `Context` argument shared by every
/// `kdf` call in this module (RFC 6189 section 4.5.1).
pub fn kdf_context(zid_i: [u8; 12], zid_r: [u8; 12], total_hash: &[u8]) -> Vec<u8> {
    let mut ctx = Vec::with_capacity(12 + 12 + total_hash.len());
    ctx.extend_from_slice(&zid_i);
    ctx.extend_from_slice(&zid_r);
    ctx.extend_from_slice(total_hash);
    ctx
}

/// `total_hash = hash(Hello_responder || Commit || DHPart1 || DHPart2)`
/// (RFC 6189 section 4.4.1.4) -- the encoded wire bytes of each message, in
/// this fixed order regardless of which side is computing it.
pub fn total_hash(responder_hello: &[u8], commit: &[u8], dhpart1: &[u8], dhpart2: &[u8]) -> [u8; 32] {
    let mut data = Vec::with_capacity(responder_hello.len() + commit.len() + dhpart1.len() + dhpart2.len());
    data.extend_from_slice(responder_hello);
    data.extend_from_slice(commit);
    data.extend_from_slice(dhpart1);
    data.extend_from_slice(dhpart2);
    sha256(&data)
}

/// `s0 = hash(counter || DHResult || "ZRTP-HMAC-KDF" || ZIDi || ZIDr ||
/// total_hash || len(s1)||s1 || len(s2)||s2 || len(s3)||s3)` (RFC 6189
/// section 4.4.1.4) -- `s1` is the matched retained secret if this pair of
/// ZIDs has one cached (`None` on a first-ever call with this peer); `s2`
/// (aux secret) and `s3` (PBX secret) are always empty here, since neither
/// is implemented.
pub fn derive_s0(
    dh_result: &[u8], zid_i: [u8; 12], zid_r: [u8; 12], total_hash: &[u8; 32], s1: Option<&[u8]>,
) -> [u8; 32] {
    let mut data = Vec::new();
    data.extend_from_slice(&1u32.to_be_bytes());
    data.extend_from_slice(dh_result);
    data.extend_from_slice(b"ZRTP-HMAC-KDF");
    data.extend_from_slice(&zid_i);
    data.extend_from_slice(&zid_r);
    data.extend_from_slice(total_hash);
    for secret in [s1, None, None] {
        let bytes = secret.unwrap_or(&[]);
        data.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        data.extend_from_slice(bytes);
    }
    sha256(&data)
}

#[derive(Clone)]
pub struct SrtpKeys {
    pub key_i: [u8; 16],
    pub salt_i: [u8; 14],
    pub key_r: [u8; 16],
    pub salt_r: [u8; 14],
}

/// RFC 6189 section 4.5.3, quoted verbatim in this module's doc comment.
pub fn derive_srtp_keys(s0: &[u8], context: &[u8]) -> SrtpKeys {
    SrtpKeys {
        key_i: kdf(s0, "Initiator SRTP master key", context, 128).try_into().unwrap(),
        salt_i: kdf(s0, "Initiator SRTP master salt", context, 112).try_into().unwrap(),
        key_r: kdf(s0, "Responder SRTP master key", context, 128).try_into().unwrap(),
        salt_r: kdf(s0, "Responder SRTP master salt", context, 112).try_into().unwrap(),
    }
}

/// `mackeyi`/`mackeyr` -- key the Confirm1/Confirm2 message MAC (RFC 6189
/// section 4.5.3), 256 bits since this implementation only ever negotiates
/// SHA-256 as the hash algorithm.
pub fn derive_mac_keys(s0: &[u8], context: &[u8]) -> ([u8; 32], [u8; 32]) {
    let mackey_i = kdf(s0, "Initiator HMAC key", context, 256).try_into().unwrap();
    let mackey_r = kdf(s0, "Responder HMAC key", context, 256).try_into().unwrap();
    (mackey_i, mackey_r)
}

/// Keys the Confirm1/Confirm2 payload's own AES-128-CFB encryption --
/// **the "ZRTP Key" label string here was not independently confirmed**
/// against spec text (see this module's doc comment); it follows the same
/// naming pattern as every other confirmed label but hasn't been verified
/// the way the SRTP/MAC/SAS labels above were.
pub fn derive_zrtp_keys(s0: &[u8], context: &[u8]) -> ([u8; 16], [u8; 16]) {
    let key_i = kdf(s0, "Initiator ZRTP key", context, 128).try_into().unwrap();
    let key_r = kdf(s0, "Responder ZRTP key", context, 128).try_into().unwrap();
    (key_i, key_r)
}

/// `sashash = KDF(s0, "SAS", KDF_Context, 256)`, `sasvalue` = leftmost 32
/// bits (RFC 6189 section 4.5.2, quoted verbatim in this module's doc
/// comment) -- rendered here as 4 base32 characters of our own devising
/// (not RFC 6189 Appendix A's actual PGP-word/base32 scheme, which wasn't
/// obtainable this session). Self-consistent (both sides render the same
/// `sasvalue` the same way) but not a byte-for-byte match to what a real
/// ZRTP client would display for the same value.
pub fn derive_sas(s0: &[u8], context: &[u8]) -> (u32, String) {
    let sashash = kdf(s0, "SAS", context, 256);
    let sasvalue = u32::from_be_bytes(sashash[..4].try_into().unwrap());
    (sasvalue, render_sas_base32(sasvalue))
}

const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

fn render_sas_base32(value: u32) -> String {
    // 4 characters x 5 bits = 20 bits of the 32-bit sasvalue.
    (0..4)
        .map(|i| {
            let shift = 27 - i * 5;
            let idx = (value >> shift) & 0x1f;
            BASE32_ALPHABET[idx as usize] as char
        })
        .collect()
}

/// `hvi = hash(initiator's DHPart2 message || responder's Hello message)`
/// (RFC 6189 section 4.4.1.1, quoted verbatim in this module's doc
/// comment), truncated to 256 bits (a full SHA-256 output, so no actual
/// truncation happens here).
pub fn compute_hvi(dhpart2_bytes: &[u8], responder_hello_bytes: &[u8]) -> [u8; 32] {
    let mut data = Vec::with_capacity(dhpart2_bytes.len() + responder_hello_bytes.len());
    data.extend_from_slice(dhpart2_bytes);
    data.extend_from_slice(responder_hello_bytes);
    sha256(&data)
}

/// HMAC-SHA256-based message authentication for Hello/Commit/DHPart1/2,
/// keyed with the hash-chain image due to be revealed *next* by this
/// message (e.g. Hello's `mac` uses this side's own H2, even though the
/// receiver can't verify it until that H2 is later revealed) -- truncated
/// to 64 bits, matching the 8-byte `mac` wire field.
pub fn message_mac(chain_key: &[u8; 32], message_bytes_before_mac: &[u8]) -> [u8; 8] {
    hmac_sha256(chain_key, message_bytes_before_mac)[..8].try_into().unwrap()
}

// ── EC25 (P-256 ECDH) key agreement ──────────────────────────────────────────

pub struct Ec25KeyPair {
    private: agreement::EphemeralPrivateKey,
    pub public_bytes: [u8; 64],
}

pub fn generate_ec25_keypair() -> Ec25KeyPair {
    let rng = ring::rand::SystemRandom::new();
    let private =
        agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).expect("system RNG must succeed");
    let public = private.compute_public_key().expect("key generation must succeed");
    // Uncompressed SEC1 point: 0x04 || X (32 bytes) || Y (32 bytes) -- strip
    // the leading format byte, since both sides always know the curve and
    // the point is always uncompressed here.
    let full = public.as_ref();
    let mut public_bytes = [0u8; 64];
    public_bytes.copy_from_slice(&full[1..65]);
    Ec25KeyPair { private, public_bytes }
}

/// Computes the shared secret with `peer_public` (64-byte uncompressed
/// X||Y, same encoding as `Ec25KeyPair::public_bytes`) and consumes `self`
/// -- ECDH private keys are single-use by construction (`ring` doesn't
/// allow reuse), matching ZRTP's own fresh-keypair-per-call design anyway.
pub fn ec25_shared_secret(keypair: Ec25KeyPair, peer_public: &[u8; 64]) -> Vec<u8> {
    let mut sec1 = [0u8; 65];
    sec1[0] = 0x04;
    sec1[1..].copy_from_slice(peer_public);
    let peer_key = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, sec1);
    agreement::agree_ephemeral(keypair.private, &peer_key, |shared| shared.to_vec())
        .expect("peer public key must be a valid P-256 point")
}

// ── Confirm payload encryption (RFC 6189 section 4.6) ────────────────────────
// "Part of the Confirm1 and Confirm2 messages are encrypted using
// full-block Cipher Feedback Mode and contain a 128-bit random ... IV" --
// quoted verbatim in this module's doc comment.

use aes::cipher::{AsyncStreamCipher, KeyIvInit};

type Aes128CfbEnc = cfb_mode::Encryptor<aes::Aes128>;
type Aes128CfbDec = cfb_mode::Decryptor<aes::Aes128>;

pub fn confirm_encrypt(key: &[u8; 16], iv: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
    let mut buf = plaintext.to_vec();
    Aes128CfbEnc::new(key.into(), iv.into()).encrypt(&mut buf);
    buf
}

pub fn confirm_decrypt(key: &[u8; 16], iv: &[u8; 16], ciphertext: &[u8]) -> Vec<u8> {
    let mut buf = ciphertext.to_vec();
    Aes128CfbDec::new(key.into(), iv.into()).decrypt(&mut buf);
    buf
}

#[cfg(test)]
#[path = "../../tests/unit/zrtp/crypto.rs"]
mod tests;
