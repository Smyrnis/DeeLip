---
layout: home

hero:
  name: DeeLip
  text: A lightweight SIP softphone for Linux, Windows, and macOS
  tagline: Encrypted calls, video, and a clean native desktop UI — no Electron, no bloat.
  image:
    src: /icon.png
    alt: DeeLip
  actions:
    - theme: brand
      text: Download
      link: /downloads/
    - theme: alt
      text: Read the Docs
      link: /docs/README
    - theme: alt
      text: GitHub
      link: https://github.com/Smyrnis/DeeLip

features:
  - title: Full SIP calling
    details: Registration, hold/resume, attended transfer, and 3-way local conferencing.
  - title: ZRTP end-to-end encryption
    details: A from-scratch ZRTP (RFC 6189) implementation with SAS verification, alongside SDES-SRTP.
  - title: Video calling
    details: H.264 video negotiated additively over the same encrypted RTP session as audio.
  - title: Wide codec support
    details: G.711, G.722, G.729, GSM, iLBC, and Opus, with acoustic echo cancellation and AGC.
  - title: Real NAT traversal
    details: STUN, TURN relay fallback, and full ICE — works behind real-world NATs, not just LANs.
  - title: Presence, voicemail, and messaging
    details: SIP presence subscriptions, voicemail (MWI) notifications, and threaded SIP MESSAGE chat.
  - title: Call rules and LDAP directory
    details: Do-not-disturb, call forwarding, a configurable dial plan, and LDAP directory search.
  - title: Self-updating
    details: Checks GitHub Releases and verifies a SHA-256 checksum before swapping in a new build.
---

## See it in action

<div class="screenshot-row">
  <img src="/screenshots/dialer.png" alt="DeeLip dialer and in-call screen" />
  <img src="/screenshots/contacts.png" alt="DeeLip contacts list" />
  <img src="/screenshots/history.png" alt="DeeLip call history" />
</div>

<div class="info-columns">
  <div class="info-col">
    <h4>Platforms</h4>
    <ul>
      <li><a href="/downloads/">Linux — .deb / .rpm / .tar.gz</a></li>
      <li><a href="/downloads/">Windows — .msi</a></li>
      <li><a href="/downloads/">macOS — .dmg</a></li>
      <li><a href="/faq/">No mobile app</a></li>
    </ul>
  </div>
  <div class="info-col">
    <h4>Get involved</h4>
    <ul>
      <li><a href="https://github.com/Smyrnis/DeeLip/issues/new" target="_blank" rel="noopener">Report a bug</a></li>
      <li><a href="https://github.com/Smyrnis/DeeLip/discussions" target="_blank" rel="noopener">Ask a question</a></li>
      <li><a href="https://github.com/Smyrnis/DeeLip" target="_blank" rel="noopener">Contribute on GitHub</a></li>
    </ul>
  </div>
  <div class="info-col">
    <h4>Project</h4>
    <ul>
      <li><a href="https://github.com/Smyrnis/DeeLip" target="_blank" rel="noopener">Source code</a></li>
      <li><a href="https://github.com/Smyrnis/DeeLip/blob/main/LICENSE" target="_blank" rel="noopener">MIT License</a></li>
      <li><a href="/docs/changelogs/CHANGELOG">Changelog</a></li>
    </ul>
  </div>
  <div class="info-col">
    <h4>Privacy</h4>
    <p>Everything lives in one local SQLite file. No account, no cloud sync, no telemetry.</p>
  </div>
</div>

<style>
.screenshot-row {
  display: flex;
  gap: 16px;
  flex-wrap: wrap;
  justify-content: center;
  margin: 32px 0;
}
.screenshot-row img {
  max-width: 30%;
  border-radius: 8px;
  border: 1px solid var(--vp-c-divider);
}
@media (max-width: 719px) {
  .screenshot-row img {
    max-width: 100%;
  }
}
</style>
