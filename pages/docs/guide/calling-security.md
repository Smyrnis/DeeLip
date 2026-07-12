# Calling & security

This is the part of DeeLip that makes and receives calls — everything from
registering with your SIP provider to keeping a conversation private.

## What it does

- **Registration** — connects to your SIP provider (or works completely serverless,
  see "Local Account" below) and keeps that connection alive, automatically
  reconnecting if your network drops.
- **Full call control** — place and receive calls, put a call on hold and resume it,
  do an attended or blind transfer, or bridge two calls into a local 3-way
  conference.
- **Presence, voicemail, and messaging** — see when a contact is available, get
  notified when a new voicemail is waiting, and send/receive SIP instant messages in
  a real threaded conversation.

## End-to-end encryption

Encryption is a per-account choice, set in Settings > Account:

- **Match Transport** (the default) — encrypts media only when the account's
  signaling transport is already encrypted (TLS).
- **Disabled** — never encrypt media, for providers/peers that can't negotiate it.
- **Enabled** — always use SDES-SRTP, a simpler encryption mode that doesn't need
  ZRTP support on the other end.
- **ZRTP** (RFC 6189) — negotiated in-band with a short verification code (SAS) both
  sides can read aloud to confirm nobody is intercepting the key exchange.

Whichever mode you pick, DeeLip never needs a certificate authority or a cloud
account to encrypt a call — the key exchange happens directly between the two
phones.

## Call handling

- **DTMF** — how touch-tone digits are sent during a call (for phone menus/IVRs):
  RFC 2833 (the default, sent as RTP events), SIP INFO, in-band audio tones, or
  Auto. Set per account in Settings > Account.
- **Forwarding and Do Not Disturb** — forward calls always, only when unanswered
  (with a configurable ring timeout), or only when busy. Do Not Disturb rejects
  everything without ringing.
- **Auto-answer** — automatically pick up after a configurable delay. There's also
  an intercom-style mode that answers or declines based on a `Call-Info` header the
  caller sends, for paging/intercom setups.
- **Hide caller ID** — withhold your identity on outgoing calls, if your provider
  supports it.
- **Presence and voicemail** — publish your own availability to contacts watching
  it, and get notified (with a mailbox indicator) when your provider signals a new
  voicemail is waiting.
- **Keeping the connection healthy** — registration expiry, keepalive interval, and
  SIP session timers are all tunable per account if your provider needs
  non-default values; the defaults work for most providers out of the box.

## Blocking callers

Settings > Advanced has a global blocklist — numbers or SIP URIs added there are
rejected automatically, regardless of which account they call.

## Local Account (serverless calling)

DeeLip can place and receive calls directly to a bare IP address with no SIP
provider or registrar involved at all — useful for testing, a local network, or
anywhere you don't need a phone number, just a direct connection to another
DeeLip/SIP client.

## Video

Video rides alongside the same encrypted call as audio, negotiated automatically
when both sides support it — no separate setup.

---
Curious how any of this actually works under the hood? The engineering notes live
in [`docs/crates/sip-core.md`](https://github.com/Smyrnis/DeeLip/blob/main/docs/crates/sip-core.md)
on GitHub.
