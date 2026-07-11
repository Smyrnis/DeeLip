# ZRTP (RFC 6189)

Sources: `crates/sip-core/src/zrtp/{engine,wire,crypto}.rs`,
`crates/media-engine/src/zrtp_session.rs`.

DeeLip's ZRTP support is a from-scratch implementation split across two
crates: `sip-core::zrtp` is the protocol itself (wire format, crypto,
handshake state machine), `media-engine::zrtp_session` drives it against one
call's actual RTP socket.

## Provenance and verification status

RFC 6189's own packet-format figures (the exact header byte layout, the
message preamble value, the CRC algorithm) were not obtainable through this
project's tooling (the fetcher used truncated the document before reaching
those figures, and no secondary source had byte-exact detail either). The
framing constants in `wire.rs` (`ZRTP_MAGIC_COOKIE`, `MSG_PREAMBLE`, the
CRC-32 variant) are implemented from general knowledge of the protocol
rather than a freshly-verified spec quote.

The KDF/s0/SRTP-key derivation formulas in `crypto.rs`, by contrast, **are**
quoted directly from RFC 6189 sections 4.5.1-4.5.3, fetched and verified.
The one exception is the "ZRTP key" (Confirm-payload encryption key) label
string, which was not independently confirmed -- see `derive_zrtp_keys`'s
own doc comment.

The hash-chain reveal sequence in `engine.rs` (which message carries which
side's chain value, the exact `hops` argument to
`crypto::verify_hash_chain_hop` at each transition) was not found verbatim
in RFC 6189's text either -- it's this implementation's own reconstruction
of how the mechanism must fit together.

**Net effect**: only *self*-consistency (two instances of this exact code
interoperating with each other, exercised by `engine.rs`'s own two-instance
handshake test) has actually been verified. Real-world interop with another
ZRTP implementation (Zfone/Linphone/PJSIP/etc.) is unverified and should be
checked against a real peer before this is trusted for that.

## Crypto

Uses `ring` (already an existing transitive dependency of this workspace via
`rustls`'s own crypto backend -- the same crate this app's TLS transport
already trusts) for SHA-256/HMAC-SHA256/P-256 ECDH, and the RustCrypto
`aes`/`cfb-mode` crates for the Confirm payload's AES-128-CFB encryption.

Only one algorithm per category is implemented: SHA-256 / AES-128 / EC25
(P-256 ECDH) / a base32 SAS rendering of our own devising (not RFC 6189
Appendix A's actual word list, which wasn't obtainable either). These don't
need to be independently negotiable, since the auth-tag/cipher types only
describe the existing SRTP suite this app already uses for SDES.

## Hash-chain reveal sequence (why `HandshakeState` looks the way it does)

Both sides generate a hash chain H0 (random) -> H1=hash(H0) -> H2=hash(H1)
-> H3=hash(H2) and reveal it progressively across their own messages so each
message is transitively bound to the one before it (RFC 6189 section 9)
without exposing a pre-image before it's needed:

- **Initiator** (sent the original INVITE, maps to the SIP caller):
  Hello(H3) -> Commit(H2) -> DHPart2(H1) -> Confirm2(H0). Every step is a
  direct one-hop chain link (`hash(H2) == H3`, etc.).
- **Responder** (SIP callee): Hello(H3) -> DHPart1(H1) -> Confirm1(H0). The
  responder never sends a Commit (only the initiator does), so its own H2 is
  never transmitted at all -- the verifier just applies SHA-256 *twice* when
  checking DHPart1's H1 against Hello's H3 (`hash(hash(H1)) == H3`) instead
  of validating an intermediate H2.

## Scope cuts

Deliberately scoped down from the full RFC: only the messages needed for a
plain two-party DH/EC key exchange are implemented (Hello, Commit,
DHPart1/2, Confirm1/2, Conf2ACK). GoClear/ClearACK, Ping/PingACK, SASrelay,
Error/ErrorACK, the signature extension, PBX/multistream/preshared modes are
all out of scope.

- No retained-secret ID matching (`rs1IDi`/`rs1IDr`/etc. wire fields are
  always zeroed and never checked) -- `s0` is always derived as if this were
  a first-ever call with this peer. The retained-secrets cache (`cache.rs`)
  is still populated after each successful call and can be surfaced as an
  informational "seen this peer securely before" hint, but it does not feed
  back into key derivation or auto-verification.
- Commit contention (both sides sending Commit simultaneously) isn't
  handled -- fine for a normal two-party call, since only the SIP caller
  ever sends Commit here.

## Media-engine integration (`zrtp_session.rs`)

`SqliteSecretStore` drives `ZrtpEngine` for one call's RTP socket:
retransmitting our own last-sent handshake message on packet loss,
persisting retained secrets in the same SQLite database as the rest of
DeeLip's config, and translating engine events into what `engine.rs`'s RTP
loop needs to act on (send bytes, swap in SRTP keys, surface the SAS).

Retransmission is a flat retry (`RESEND_INTERVAL` apart, up to
`MAX_ATTEMPTS`) rather than RFC 6189's own exponential-backoff schedule --
simpler, and (per the verification status above) this implementation's own
tests are the only thing that have ever exercised it.
