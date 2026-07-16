# nat (`crates/nat`)

NAT traversal for RTP media: discovering/obtaining a usable address+port pair for
each call leg when the phone sits behind a NAT device, via three independent
mechanisms of increasing capability (STUN reflexive discovery, TURN relay, full ICE).
`sip-core` and `ui` consume this crate's output when building SDP offers/answers and
setting up the actual media socket; `nat` itself has no SIP/RTP knowledge of its own —
it only produces addresses and, for ICE/TURN, live `Conn` objects.

## Architecture

- **`stun.rs`** — STUN Binding Request/Response (RFC 5389), IPv4 only.
  `discover_external_addr(stun_server)` sends one Binding Request over a fresh UDP
  socket and parses the response for XOR-MAPPED-ADDRESS (preferred) or the older
  plain MAPPED-ADDRESS, returning the external `SocketAddr` a NAT device maps this
  socket's outbound traffic to. One-shot, synchronous-feeling despite the `tokio`
  socket — used to learn "what does the outside world see this socket as," not to
  keep a mapping alive.
- **`turn_relay.rs`** — TURN client (RFC 5766) via the `turn` crate.
  `allocate_relay(turn_server, username, password)` allocates one relayed transport
  address and returns a `TurnRelay` whose `conn` is a `webrtc_util::Conn` trait object
  behaving like a normal socket (`send_to`/`recv_from`), with TURN framing and peer
  permissions handled internally. No ICE involved — this is a plain, unconditional
  relay path.
- **`ice.rs`** — full ICE (RFC 8445) via the `webrtc-ice` crate (same `webrtc-rs`
  family/version as `turn`/`webrtc-util`). `gather()` collects host + server-reflexive
  (via STUN) + relay (via TURN, if configured) candidates into an `IceGathered`;
  `connect()` feeds the remote's parsed-from-SDP ICE parameters into the same agent
  and runs connectivity checks, producing an `IceConnection` whose `conn` is — like
  `TurnRelay::conn` — a drop-in `webrtc_util::Conn` for `MediaEngine::start`'s `relay`
  parameter.
- **`lib.rs`** — re-exports the three modules' public surface, plus
  `alloc_rtp_port`/`alloc_rtp_port_ephemeral`/`alloc_rtp_port_in_range`: local RTP port
  allocation (even port, per SIP convention), either from the OS's ephemeral range or
  a configured `min..=max` range (so a fixed firewall/NAT port-forward can cover every
  call).

## How the three compose

This crate does not itself decide which mechanism to use for a given call — that
policy (try ICE first if configured, fall back to plain STUN-reflexive or TURN
otherwise) lives in the call-setup code that calls into this crate (`ice.rs`'s own
module doc points at it as the "additive, not a replacement" fallback relationship).
From this crate's own vantage point: `ice.rs` is additive to, not a replacement for,
the plain STUN-reflexive/TURN-unconditional path — a call falls back to that older
path if ICE gathering fails or times out. TURN is used unconditionally (no ICE
involved at all) whenever a TURN server is configured without ICE; STUN alone just
answers "what's our external address," used to build a plain `c=`/`m=` SDP line for
peers that don't speak ICE.

## Design decisions & invariants

- **`ConnectedConn` (`ice.rs`)**: the `Conn` returned by ICE connectivity checks
  (`AgentConn`, private to `webrtc-ice`) only implements the "connected socket" half of
  `Conn` — `send`/`recv`, always talking to whichever candidate pair won a connectivity
  check. Its `send_to`/`recv_from` unconditionally return "not applicable," since
  there's no per-call destination once a pair is selected. But `MediaEngine`'s
  `RtpSocket` abstraction is built around `send_to`/`recv_from` (shared with the TURN
  relay `Conn`, which *does* implement them properly) — `ConnectedConn` bridges the
  gap by delegating to `send`/`recv` + `remote_addr()`, so an ICE-selected `Conn` can
  be handed to `MediaEngine::start`'s `relay` parameter completely unchanged, same
  shape as `TurnRelay::conn`.
- **`IceConnection` keeps its `Agent` alive** alongside the winning `Conn` deliberately
  — `AgentConn`'s own docs don't guarantee it keeps working independent of its parent
  `Agent`'s lifetime, so rather than assume independence, both are held together for
  as long as the call's media is active. Mirrors `TurnRelay` keeping its
  `turn::client::Client` alive alongside its `conn` for the identical reason.
- **`alloc_rtp_port`'s TOCTOU tradeoff**: both the ranged and ephemeral allocators bind
  a probe socket only to confirm a port is free, then drop it immediately — there's a
  small window where another process could grab the same port before the real RTP
  socket binds it. Accepted, not fixed: closing that window would need atomically
  reserving-and-holding the port across the probe and the real bind, which the
  standard socket API doesn't offer cleanly, and the race is narrow enough in practice
  not to have caused a real failure.
- **`turn_relay.rs`'s `RELAY_ALLOC_TIMEOUT` (5s, matching `stun.rs`'s existing
  timeout) bounds both the TURN listener startup and the Allocate request.**
  Without it, an unreachable or silently-dropping TURN server left a call
  stuck in "Calling…"/"Ringing" indefinitely with no way to cancel.
- **ICE's connectivity-check cancel channel is never actually triggered** (`connect()`
  in `ice.rs`) — `_cancel_tx` is kept alive for the whole call so `cancel_rx.recv()`
  doesn't resolve immediately (a closed channel's `recv` returns `None` right away,
  which `dial`/`accept` would treat as a cancel request). This is a "never cancel"
  channel by construction, not an unfinished cancellation feature.

## Known limitations / open items

- STUN support is IPv4 only (`stun.rs`'s own module doc).
- No protocol-provenance caveat was found in this crate the way ZRTP's implementation
  carries one (see `docs/crates/sip-core.md`) — STUN/TURN/ICE here are all thin wrappers
  around well-established external crates (`turn`, `webrtc-ice`, `webrtc-util`)
  implementing the wire protocols themselves, rather than a from-scratch
  reimplementation whose correctness this project would need to disclose the
  provenance of.
