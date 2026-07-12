# Working behind your router (NAT)

Almost everyone's phone or computer sits behind a router doing NAT (Network Address
Translation) — your device has a private address that the outside internet can't
reach directly. That's normally a problem for real-time calls, which need a direct
path for audio/video to flow. DeeLip has three techniques available to work around
it:

- **STUN** — asks a public server "what does the outside world see my address as?"
  so DeeLip can advertise a reachable address to the other party. **On by default**,
  using a public Google STUN server, and works for most home routers with no setup.
- **ICE** — tries every possible path (direct, STUN-discovered, relayed) and picks
  whichever one actually connects. This is the most reliable option on tricky
  networks, but it's **off by default** — turn it on in Settings > Network (globally,
  or per account if you only need it for one provider).
- **TURN relay** — for the strictest networks, where no direct path is possible at
  all: media is relayed through a third-party server instead. Also configured in
  Settings > Network; once a TURN server is set, DeeLip relays every call through it
  rather than only falling back to it case by case.

So for a typical home or office network, STUN alone is usually enough and needs no
configuration. If calls still won't connect, or your SIP provider gave you a specific
STUN/TURN server (or ICE credentials) to use, set those in Settings > Network:

- **STUN server** — defaults to a public Google server; override it if your provider
  wants you to use their own.
- **TURN server, username, password** — set these if your provider (or your own
  network) requires a relay.
- **ICE** — the on/off toggle described above.
- **RTP port range** — restricts the UDP ports DeeLip uses for call audio/video, so
  you can open just that range in a firewall instead of a wide port range.
- **Custom nameserver / SIP SRV lookups** — point DNS queries at a specific server,
  and optionally enable SRV record lookups if your provider publishes them.

One limitation worth knowing: STUN support is IPv4-only today.

## If calls still won't connect

See [Troubleshooting](/troubleshooting/) — most connectivity problems are either a
registration issue (check your account settings) or a very restrictive
firewall/network that blocks even a relayed connection.

---
Curious how any of this actually works under the hood? The engineering notes live
in [`docs/crates/nat.md`](https://github.com/Smyrnis/DeeLip/blob/main/docs/crates/nat.md)
on GitHub.
