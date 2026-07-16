# sip-core (`crates/sip-core`)

`deelip-sip` is DeeLip's SIP user-agent: registration, call signaling (INVITE/BYE/
CANCEL/re-INVITE), SDP offer/answer construction and parsing (audio and video),
STUN/TURN/ICE endpoint resolution for each call leg, SUBSCRIBE/NOTIFY event packages
(presence, voicemail MWI), outgoing presence PUBLISH, SIP MESSAGE instant messaging,
and ZRTP end-to-end key negotiation. It owns every protocol-level decision a call
needs (codec/SRTP/ICE negotiation happens here, not in `ui`) and hands the `ui` crate
only the resolved result (`CallMediaReady`) it needs to start real media via
`deelip-media`. `deelip-config` (account settings) and `deelip-nat` (STUN/TURN/ICE
primitives) are this crate's own dependencies; nothing in `sip-core` depends on `ui`
or `media-engine`.

## Architecture

**Entry point**: `client::SipStack` is one registered SIP identity's whole runtime --
`SipStack::spawn` starts it on its own background task and hands back a `SipHandle`
(a cheap command/event façade: send a `SipCommand`, receive `SipEvent`s). `run()`'s
`tokio::select!` loop is the heart of the whole crate: incoming datagrams
(`transport.recv()`), commands from `ui` (`cmd_rx`), completed background call-setup
results (`internal_rx`, see below), and three periodic ticks (re-registration,
presence/MWI/session-timer refresh, NAT keepalive).

**Why call setup is split into a background task + a completion event**: STUN/TURN/
ICE resolution is real network I/O bounded by multi-second timeouts. Doing it inline
inside `initiate_call`/`accept_call`/`on_response` would block this account's *entire*
event loop -- every other call's hold/resume, incoming messages, re-registration --
for that whole time, since `run()`'s `select!` fully awaits one branch before looping
back. Each of the three call-setup paths instead spawns its own resolution as a
`tokio::spawn`ed task and reports back via `StackEvent` (`OutgoingOfferReady`/
`IncomingAnswerReady`/`OutgoingConnected`), which `run()` picks up as just another
`select!` branch alongside everything else.

**`client/{connect,run_loop,builders}.rs`**: split from a single `client.rs`
purely for file size (same precedent as `views/settings/`, `views/dialer/`,
`call/lifecycle/`), not a behavior/API change -- `SipStack` keeps every method
it had, just spread across `connect.rs` (transport setup + reconnect loop),
`run_loop.rs` (the main event loop + dispatchers), and `builders.rs`
(wire-format response/ACK builders). Every name re-exported from
`client/mod.rs` was already `pub`/`pub(crate)` at this same `client::` path in
the original file.

**Call lifecycle** (`call/lifecycle/{mod,outgoing,incoming,reinvite,teardown}.rs`
plus the `response/` submodule, all just `impl SipStack` blocks split by concern --
see
`call/transfer.rs` for the same multi-file-single-impl pattern applied elsewhere):
- `outgoing.rs`: `initiate_call` resolves media on a background task;
  `on_outgoing_offer_ready` sends the actual INVITE once that resolves.
- `incoming.rs`: `on_invite` (fresh INVITE, or a re-INVITE on an already-confirmed
  dialog); `accept_call`/`on_incoming_answer_ready` mirrors the outgoing side's
  background-resolve-then-finish shape; `reject_call`.
- `response/{mod,connected,session_timers,challenge}.rs`: `mod.rs`'s `on_response`
  classifies every response into a local `Act` enum first (so no `.await` runs while
  `self.dialogs` is mutably borrowed), then executes it via one `handle_*` method per
  `Act` variant, split out purely for file size (`connected.rs::handle_connected`,
  `session_timers.rs::handle_session_refresh_ack`/`handle_session_interval_too_small`,
  `challenge.rs::handle_invite_challenged`) -- not a behavior change.
- `reinvite.rs`: hold/resume re-INVITEs (`send_reinvite`), RFC 4028 session-timer
  refresh re-INVITEs (`send_session_refresh`/`refresh_session_timers`), and SIP INFO
  DTMF relay (`send_dtmf_info`).
- `teardown.rs`: outgoing BYE (`hang_up`) and the incoming BYE/ACK/CANCEL handlers.
- `mod.rs`: shared `StackIdentity` (fields derived from `&self` alone) and
  `DialogRequestContext` (everything needed to build a mid-dialog request/response for
  one `Dialog`) -- see "Design decisions" below for why these exist as separate types.

**`call::dialog`**: `Dialog` is the state machine for one call (`DialogState`:
Calling → Ringing → Confirmed → Terminating → Terminated), holding everything needed
to rebuild a hold/resume re-INVITE, RFC 4028 session-timer state, and (once confirmed)
`CallMedia` -- the negotiated codec/SRTP/relay/ICE state, plus an optional `VideoMedia`
leg.

**`call::media_setup`**: SDP construction combined with STUN/TURN/ICE endpoint
resolution for one call leg -- this is the actual call-setup "business logic," moved
here from `ui/src/media.rs` specifically so it runs inside `SipStack`'s own async task
rather than being `rt.block_on`'d from the egui UI thread (ICE gathering's multi-second
timeout would otherwise freeze the whole window on every call). `account_codecs`/
`codec_from_str` resolve an account's codec preference list; `try_gather_ice`/
`try_answer_with_ice`/`finish_ice_connect` wrap `deelip_nat::ice` with graceful
fallback (`None` on any failure, never a hard error, so a call always falls back to
plain UDP); `resolve_call_media`/`resolve_video_media` combine the raw negotiated
pieces into the `CallMedia`/`CallMediaReady` (or `VideoMedia`/`VideoMediaReady`) pair
every call-setup path ends with.

**Wire layer** (`wire/`, zero dependency on call dialogs or subscriptions --
everything else in this crate builds on top of it):
- `message.rs`: `SipMessage`/`SipMethod`/`SipStartLine` -- parses raw bytes into a
  message, with RFC 3261 §7.3.1 header folding.
- `sdp.rs`: audio codec enum-dispatch (`AudioCodec`, one `payload_type`/`rtpmap`/
  `fmtp` per variant), SDES-SRTP (`SrtpParams`/`SrtpSession`), ICE attribute lines,
  offer/answer/hold/resume builders, and the SDP parser (`parse_sdp_forcing`). Also
  the video counterparts (`VideoCodec`, `build_video_media_section`,
  `parse_video_section`, `split_media_sections`) -- see "Video negotiation" below.
- `auth.rs`: RFC 2617 digest auth -- `build_challenge_response` is the shared
  parse-compute-build sequence REGISTER/INVITE/SUBSCRIBE/MESSAGE/PUBLISH's 401/407
  retry handling all call into.
- `dns.rs`: a minimal hand-rolled DNS client for the optional custom-nameserver
  override and SRV-record (RFC 3263) service discovery -- hand-rolled rather than
  pulling in a full resolver crate, matching this crate's existing style for simple/
  fixed wire formats. Only does what SIP needs: one A/AAAA or SRV question per query,
  sent to a single server expected to do its own recursive resolution. Falls back to
  `tokio::net::lookup_host` (the pre-existing behavior from before this module existed)
  whenever neither a custom nameserver nor a usable `/etc/resolv.conf` entry is
  available; that fallback is itself bounded by `DNS_TIMEOUT` since this resolution
  sits on `main()`'s startup path before the app window exists -- an unreachable or
  misbehaving OS resolver must not be able to hang the caller forever.
- `framing.rs`: Content-Length-based message boundary detection for stream transports
  (TCP/TLS) -- UDP doesn't need this, one datagram is always exactly one message.
- `util.rs`: call-id/tag/branch generation, `Via`/`Session-Expires`/`Call-Info`/
  `Replaces` header parsing helpers.

**Subscriptions** (`subscription/`): presence (RFC 3856, `presence.rs`) and voicemail
MWI (RFC 3842, `mwi.rs`) share the same SUBSCRIBE/refresh/auth-retry mechanics
(`handlers.rs`'s `build_subscribe`, parameterized by event package/accept header) but
are kept as separate types/maps rather than generalized, since the NOTIFY body shape
and the state each carries are genuinely different. `publish.rs` is the mirror image
of presence: outgoing PUBLISH (RFC 3903) of this account's *own* status, refreshed on
its own timer, using an `etag` (`SIP-ETag`/`SIP-If-Match`) instead of a remote dialog
tag to identify which published state a request refers to.

**`registration.rs`**: `register_once` (initial unauthenticated REGISTER, then a
digest-authenticated retry on 401/407), plus `SipAccount::allow_ip_rewrite`'s
NAT self-discovery (adopting the registrar's `received=` Via param as the advertised
IP, re-checked on every re-registration).

**`transport.rs`**: `SipTransport` unifies UDP (datagram), TCP, and TLS behind one
send/recv API. TCP and TLS share a `StreamHalves<S>` (split read/write halves plus a
`MessageFramer`) since both are a persistent byte stream needing the same
split-read/write-plus-framing plumbing; only how the stream itself gets established
differs.

**`message_method.rs`**: standalone SIP MESSAGE (RFC 3428) -- neither a call dialog
nor a subscription, just a single request/response transaction, so it gets its own
small home.

**Answering a peer's own INFO** (`run_loop.rs::dispatch`, `SipMethod::Info`): a peer's
INFO (e.g. Asterisk echoing DTMF back once `dtmf_mode=info` is set) doesn't carry
anything DeeLip needs to act on today, but RFC 6086 still expects a response --
leaving it unanswered just makes the sender retransmit it several times before giving
up, confirmed live (three "unhandled request" log lines for what was really 1-2
messages). Answered with a plain 200 OK, same as OPTIONS.

## Video negotiation

Video is negotiated additively alongside the audio leg, driven by
`SipAccount::video_enabled` and a fixed single-codec list (`VIDEO_CODECS =
[VideoCodec::H264]`, `call/lifecycle/mod.rs`). Every stage mirrors its audio
counterpart but stays a separate, parallel path rather than folding into the existing
audio types:
- `outgoing.rs::prepare_video_offer`/`incoming.rs::prepare_video_answer` allocate
  their own RTP port, gather their own ICE candidates independently of audio's, and
  generate their own SRTP key, appending a `build_video_media_section` onto the
  audio offer/answer's SDP text.
- The answer/response side parses the video `m=` section via
  `split_media_sections` + `parse_video_section` -- a **separate, independent**
  parse of the same raw SDP text, deliberately never folded into `parse_sdp_forcing`/
  `ParsedSdp` itself. `split_media_sections` exists specifically so a second `m=`
  line's attribute lines (its own `a=rtpmap`/`a=candidate`/`a=crypto`) can't leak into
  `parse_sdp_forcing`'s single flat accumulator and corrupt audio parsing.
- Failure anywhere in the video path (port allocation, ICE gather) never fails the
  call -- it just leaves the video leg absent and the call proceeds audio-only,
  exactly as if video had never been attempted.
- `call::dialog::VideoMedia`/`PendingVideoOffer`, `client::IncomingVideoAnswer`/
  `OutgoingVideoConnected`, and `events::VideoMediaReady` all mirror their audio
  counterparts one-for-one, minus the two fields that don't apply to video
  (`dtmf_type`/`cn_type`).

This negotiation path is now fully consumed on the other end too: `media-engine`'s
`ZrtpRuntime`/`VideoEngine` (see `docs/crates/media-engine.md`) and `ui/src/media.rs::
start_video` read `CallMediaReady::video`/drive real capture/encode/decode against it
-- this is a live, working call feature, not a negotiation-only placeholder.

## Design decisions & invariants

**`StackEvent`/background call-setup tasks**: see "why call setup is split" above.
The completion events (`OutgoingOfferReady`/`IncomingAnswerReady`/`OutgoingConnected`)
carry a video-specific sub-payload (`PendingVideoOffer`/`IncomingVideoAnswer`/
`OutgoingVideoConnected`) alongside the audio fields, always `None` when video wasn't
offered/negotiated for that call.

**`EventSender`** (`client.rs`) wraps the raw `mpsc::UnboundedSender<SipEvent>` so
every one of the ~30 `event_tx.send(...)` call sites across this crate also wakes up
whichever UI is consuming events, without touching each call site individually --
`.send()`'s signature deliberately mirrors `UnboundedSender::send` exactly (hence the
`#[allow(clippy::result_large_err)]`) so no caller needed to change when this wrapper
was introduced. `waker` is caller-supplied precisely so this crate doesn't need to
depend on `egui`/`eframe` to know how to request a repaint. The `SendError<SipEvent>`
payload was already this size when callers held the raw `UnboundedSender` directly, so
wrapping it here doesn't make anything larger -- it just gives clippy a named function
to measure, hence the `#[allow(clippy::result_large_err)]` rather than boxing the error.

**`SipEvent`/`Act` left un-boxed despite `large_enum_variant`**: `SipEvent::CallConnected`'s
`media: CallMediaReady` and `on_response`'s local `Act::Connected` variant (`response/
mod.rs`) both carry `DtlsCallParams` (cert/key DER bytes), making each noticeably larger
than its enum's other variants. Both are deliberately left un-boxed
(`#[allow(clippy::large_enum_variant)]`) rather than adding indirection to every
construction/match site -- neither enum was cheap to match on before this field existed
either, so the marginal cost of one more field isn't worth the ergonomic hit.

**`StackIdentity`/`DialogRequestContext`** (`call/lifecycle/mod.rs`): replace what
used to be ~10 lines of hand-cloned fields duplicated across 6+ call sites. The
non-obvious ordering constraint: `StackIdentity` (derived from `&self` alone) must be
built *before* taking a `self.dialogs.get_mut(...)` borrow, since calling a `&self`
method afterward would conflict with that outstanding `&mut` borrow --
`DialogRequestContext::new(&identity, dialog)` only needs `&Dialog`, so it's safely
callable while `dialog: &mut Dialog` is still held for later mutation.

**`Dialog::remote_contact` must be populated on the caller side too** (`response/
mod.rs`'s `on_response`, `Act::Connected` construction, on the initial INVITE's 200
OK): the callee side already captured this from the incoming INVITE's own source
address (`incoming.rs::on_invite`), but the caller side never did, so every
mid-dialog request the *caller* sent (`hang_up`'s BYE, hold/resume re-INVITEs,
transfer) fell back to `identity.server_addr` instead of the far end's real
`Contact:` (see `DialogRequestContext::new`). Harmless with a real registrar+proxy
in the path (the proxy still routes it), but a dead end for `local_account`/
proxy-less calls -- confirmed live: a caller's BYE never reached the callee in
exactly that setup. Fixed by parsing the 200 OK's own `Contact:` header the same
way the callee side already did.

**`connect_transport_auto`** (`client.rs`): `TransportProtocol::Auto` tries UDP, then
TCP, then TLS, each bounded by a timeout, keeping the first candidate that both
connects *and* gets an actual response to a one-shot unauthenticated `REGISTER`
probe (`probe_register`, `Expires: 0` so a compliant registrar treats it as a no-op).
UDP alone always "connects" (it's just a local bind), so it can't be told apart from a
genuinely unreachable server without a real round trip -- hence the probe rather than
just trying to bind each transport in turn.

**A dead transport fails every in-flight call immediately** (`run_loop.rs`'s `run()`,
the `transport.recv()` error branch): without this, any call whose dialog was live at
the moment the transport died is otherwise left hanging from the UI's perspective
indefinitely (or until the far end's own retransmit timers eventually give up and send
a BYE/CANCEL, which can take 20+ seconds) -- the in-memory dialog is gone the moment
`spawn`'s reconnect loop rebuilds a fresh `SipStack` regardless, so there's no way to
recover it either way. `run()` sends `CallFailed` for every open dialog right away
instead, so the UI reflects reality immediately rather than waiting out a timeout that
couldn't have helped anyway.

**`SipStack::spawn`'s reconnect loop**: a transport failure (dropped TLS/TCP
connection, etc.) doesn't kill the account permanently. `run()` hands back the still-
good `cmd_rx` on failure, and `spawn`'s loop reconnects with exponential backoff,
reusing the *same* `cmd_tx`/`event_rx` pair `SipHandle` was constructed with so the
reconnect is transparent to callers. In-flight dialogs/subscriptions are necessarily
lost across a transport replacement (unavoidable -- there's no live dialog state to
carry across a torn-down connection), but the account itself now recovers instead of
staying dead until the whole process restarts.

**`SipAccount::local_account`** (a serverless, direct-IP calling mode,
`client.rs::connect_local`, `outgoing.rs::resolve_local_call_target`): binds a plain
UDP listener with no registrar to resolve or connect to. `server_addr` is a
never-sent-to placeholder; outgoing calls resolve their real destination straight from
the dialed target's own URI instead. Always UDP regardless of `account.transport` --
TCP/TLS need a real persistent connection to a live peer at socket-creation time,
which doesn't exist without a fixed server.

**RFC 4028 Session Timers**: negotiated per-dialog (`Dialog::session_expires`/
`we_are_refresher`/`original_role_is_uac`), refreshed via a no-op re-INVITE
(`send_session_refresh`) at half the negotiated interval. `refresher=` always refers
to the *original* INVITE's UAC/UAS roles (`original_role_is_uac`), not whoever happens
to send a particular refresh re-INVITE. A 422 (Session Interval Too Small) response is
retried once with a `Session-Expires` at least as large as the response's own
`Min-SE`; a second 422 is a final failure, same shape as `auth_retried`'s one-retry
rule for 401/407. On the callee side (`incoming.rs`, building the 2xx response to an
INVITE), our own response's `refresher=` takes highest precedence per the RFC's own
resolution rules, so it's decided unilaterally there rather than just echoing the
caller's request: an explicit `uas` ask (they want us to refresh) is honored,
otherwise it defaults to `uac` (them) -- the same default-favors-caller rule the
outgoing side applies in `on_response`.

**Session-refresh vs. hold/resume disambiguation** (`on_invite`/`on_response`): a
session-refresh re-INVITE (same SDP, same direction, `session_refresh_pending: true`)
must not be mistaken for a real hold/resume ack -- `on_response`'s `Act` classification
checks `session_refresh_pending` before falling into the ordinary re-INVITE-ack path,
which would otherwise default `hold_pending` to `true` and wrongly report the call as
held.

**Outgoing call's c=/m= fallback address is never the ICE agent's own candidate
socket** (`outgoing.rs::initiate_call`'s background task, `resolve_rtp_endpoint`):
that socket only becomes usable once the answer confirms the far end also speaks ICE
(`on_outgoing_connected`); if it doesn't, the ICE agent (and that socket) is simply
dropped. Advertising it as the plain fallback address and then binding an unrelated
`local_rtp` port on connect would leave the far end sending RTP to a socket nothing is
listening on.

**`handle_connected`'s post-ACK codec-mismatch teardown** (`response/connected.rs`): if
the answer's SDP has no compatible codec, a real BYE is sent (`hang_up`) rather than
just dropping the dialog entry -- by this point the 2xx has already been ACKed, so both
sides already consider the dialog Confirmed, and silently dropping our own map entry
would leave the far end dangling forever with no teardown signal. This differs from the
pre-ACK failure paths elsewhere in the same handler (e.g. a 401/407 with an unparseable
challenge), which only ever reach a `Calling` dialog the far end doesn't yet consider
established, so no BYE is needed there.

**`parse_uri`'s bare-form fallback** (`wire/util.rs`): some UAs send a malformed
bare-form header with a display-name-like token glued directly onto the URI with no
quotes/brackets (e.g. `600:sip:600@host`) -- RFC 3261's bare `addr-spec` form has no
display name at all. Skipping forward to the scheme rather than storing the leading
token protects `remote_uri`/`peer_uri` from corruption if one ever arrives that way;
a no-op when the scheme already starts the string, the normal well-formed case.

**Auth retry is one-shot per request type**: `Dialog::auth_retried` (INVITE),
`PresenceSubscription`/`MwiSubscription::auth_retried` (SUBSCRIBE),
`PendingMessage::auth_retried` (MESSAGE), and `PresencePublish::auth_retried`
(PUBLISH) all follow the same shape -- a second 401/407 after already retrying once is
treated as a final failure, not retried forever (bad credentials shouldn't loop).

**Why MWI is a separate module/map from presence** (`subscription/mwi.rs`): the
SUBSCRIBE/refresh/auth-retry mechanics are shared (`build_subscribe`'s
`event_package`/`accept` params), but the NOTIFY body shape and the state each carries
are different enough that a shared generic struct would just be a blob fighting two
different call sites -- duplicating ~10 plain bookkeeping fields is cheaper than that
generalization.

**Hand-rolled DNS/SDP/message/auth parsing** rather than pulling in existing crates:
consistent style choice across `wire/` for simple, fixed wire formats this crate fully
controls both ends of (mostly) -- avoids a resolver/SDP-parsing dependency's surface
area for formats that are each only a few dozen lines to parse directly.

## DTLS-SRTP (RFC 5763/5764) — SDP half

DeeLip's third media-encryption option (alongside SDES-SRTP and ZRTP, selected via
the same `SipAccount`/`MediaEncryption::DtlsSrtp` account setting) splits the same
way ZRTP does: this crate owns the SDP negotiation, `media-engine` owns the actual
DTLS handshake and SRTP key export (see `docs/crates/media-engine.md`).

- **`wire/sdp/dtls.rs`**: `Setup` (RFC 4145 §4 -- `actpass`/`active`/`passive`,
  carried as `a=setup`), `DtlsFingerprint` (RFC 8122 -- SHA-256 over the DER cert,
  carried as `a=fingerprint`), and `generate_dtls_cert()`, which mints one
  self-signed X.509 cert/key pair (via `rcgen`) **per call**, shared across the
  audio and video legs -- never per media section. `media-engine` reconstructs a
  `webrtc_dtls::crypto::Certificate` from the same DER bytes at
  `MediaEngine::start` (`rcgen::KeyPair: TryFrom<&[u8]>` for the key,
  `rustls::pki_types::CertificateDer::from` for the cert), keeping the
  `webrtc-dtls` dependency itself isolated to that one crate.
- **Role resolution** (`response/connected.rs::resolve_offerer_dtls_role`): RFC 4145
  §4.1 -- as the offerer, our final role is simply the complement of whatever the
  answer committed to (`Active` -> we're `Passive` and vice versa). `None` (treated
  identically to "remote doesn't support DTLS-SRTP") if the answer never resolved
  `actpass` to a concrete role, or omitted the fingerprint/setup lines entirely.
- Same additive-failure philosophy as everything else in this crate: if DTLS-SRTP
  isn't offered/negotiated, the call just proceeds on whatever encryption (or none)
  was otherwise negotiated -- never a hard call failure.

**Security note**: unlike ZRTP, DTLS-SRTP has no independent human-verifiable SAS --
its only MITM protection is the fingerprint's integrity in transit over SIP
signaling. Selecting `MediaEncryption::DtlsSrtp` on an account whose resolved
transport isn't TLS means the fingerprint travels in the clear, giving no real MITM
protection (the same exposure as SDES without secure signaling). `wants_srtp` still
returns `false` for this variant either way, since the actual key material is never
SDES-carried -- only the fingerprint/setup bootstrap rides in the SDP.

## ZRTP (RFC 6189) — protocol half

DeeLip's ZRTP support is a from-scratch implementation split across two crates:
`sip-core::zrtp` (this section) is the protocol itself (wire format, crypto, handshake
state machine); `media-engine::zrtp_session` (see `docs/crates/media-engine.md`) drives it
against one call's actual RTP socket.

### Provenance and verification status

RFC 6189's own packet-format figures (the exact header byte layout, the message
preamble value, the CRC algorithm) were not obtainable through this project's tooling
(the fetcher used truncated the document before reaching those figures, and no
secondary source had byte-exact detail either). The framing constants in `wire.rs`
(`ZRTP_MAGIC_COOKIE`, `MSG_PREAMBLE`, the CRC-32 variant) are implemented from general
knowledge of the protocol rather than a freshly-verified spec quote. One explicit
instance: `Message::encode`'s length field is a plain byte count (covering the type
block + body + CRC), not a word count -- this sidesteps a word-alignment requirement
this implementation has no need for, since nothing outside this module inspects the
field directly. A self-consistent choice, not a claim of matching RFC 6189's own
on-the-wire convention exactly.

The KDF/s0/SRTP-key derivation formulas in `crypto.rs`, by contrast, **are** quoted
directly from RFC 6189 sections 4.5.1-4.5.3, fetched and verified. The one exception is
the "ZRTP key" (Confirm-payload encryption key) label string, which was not
independently confirmed -- see `derive_zrtp_keys`'s own doc comment.

The hash-chain reveal sequence in `engine.rs` (which message carries which side's
chain value, the exact `hops` argument to `crypto::verify_hash_chain_hop` at each
transition) was not found verbatim in RFC 6189's text either -- it's this
implementation's own reconstruction of how the mechanism must fit together.

**Net effect**: only *self*-consistency (two instances of this exact code
interoperating with each other, exercised by `engine.rs`'s own two-instance handshake
test) has actually been verified. Real-world interop with another ZRTP implementation
(Zfone/Linphone/PJSIP/etc.) is unverified and should be checked against a real peer
before this is trusted for that.

### Crypto

Uses `ring` (already an existing transitive dependency of this workspace via
`rustls`'s own crypto backend) for SHA-256/HMAC-SHA256/P-256 ECDH, and the RustCrypto
`aes`/`cfb-mode` crates for the Confirm payload's AES-128-CFB encryption.

Only one algorithm per category is implemented: SHA-256 / AES-128 / EC25 (P-256 ECDH)
/ a base32 SAS rendering of our own devising (not RFC 6189 Appendix A's actual word
list, which wasn't obtainable either). These don't need to be independently
negotiable, since the auth-tag/cipher types only describe the existing SRTP suite this
app already uses for SDES.

### Hash-chain reveal sequence (why `HandshakeState` looks the way it does)

Both sides generate a hash chain H0 (random) → H1=hash(H0) → H2=hash(H1) →
H3=hash(H2) and reveal it progressively across their own messages so each message is
transitively bound to the one before it (RFC 6189 section 9) without exposing a
pre-image before it's needed:

- **Initiator** (sent the original INVITE, maps to the SIP caller): Hello(H3) →
  Commit(H2) → DHPart2(H1) → Confirm2(H0). Every step is a direct one-hop chain link
  (`hash(H2) == H3`, etc.).
- **Responder** (SIP callee): Hello(H3) → DHPart1(H1) → Confirm1(H0). The responder
  never sends a Commit (only the initiator does), so its own H2 is never transmitted
  at all -- the verifier just applies SHA-256 *twice* when checking DHPart1's H1
  against Hello's H3 (`hash(hash(H1)) == H3`) instead of validating an intermediate H2.

### Scope cuts

Deliberately scoped down from the full RFC: only the messages needed for a plain
two-party DH/EC key exchange are implemented (Hello, Commit, DHPart1/2, Confirm1/2,
Conf2ACK). GoClear/ClearACK, Ping/PingACK, SASrelay, Error/ErrorACK, the signature
extension, PBX/multistream/preshared modes are all out of scope.

- No retained-secret ID matching (`rs1IDi`/`rs1IDr`/etc. wire fields are always
  zeroed and never checked) -- `s0` is always derived as if this were a first-ever
  call with this peer. The retained-secrets cache (`cache.rs`) is still populated
  after each successful call and can be surfaced as an informational "seen this peer
  securely before" hint, but it does not feed back into key derivation or
  auto-verification.
- Commit contention (both sides sending Commit simultaneously) isn't handled -- fine
  for a normal two-party call, since only the SIP caller ever sends Commit here.
- Every message's `mac` field (Hello's, Commit's, DHPart's) is likewise never
  independently verified -- the hash-chain-hop checks in `on_commit`/`on_dhpart1`/
  `on_dhpart2` already provide equivalent tamper-evidence for what actually matters
  (binding each message to the one before it). Each `mac` is computed as a
  self-consistent value (HMAC over the message's own wire encoding with the `mac`
  field zeroed) rather than a claim that it matches any real ZRTP implementation's
  exact convention.

## Known limitations / open items

- ZRTP interop with a real second implementation is unverified (see above).
- Video conferencing is out of scope of this crate's own negotiation -- see
  `docs/crates/media-engine.md`'s video section for the conference-video status (a
  `media-engine`/`ui` concern, not a `sip-core` one; the SDP/ICE negotiation this
  crate does per-leg is identical whether or not the far end is later bridged into a
  local conference).
- Commit contention and full RFC 6189 message coverage (GoClear, SASrelay, PBX mode,
  etc.) remain deliberately out of scope, per "Scope cuts" above.
