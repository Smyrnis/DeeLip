use std::net::SocketAddr;
use std::time::Duration;

use deelip_media::{ConferenceLeg, MediaEngine};
use deelip_sip::{parse_sdp, sdp, IceAttrs, ParsedSdp, SrtpSession};
use tokio::runtime::Handle;

use deelip_nat::{IceConnection, IceGathered, TurnRelay};

use crate::app::DeelipApp;
use crate::helpers::account_codecs;

/// Bounded wait for ICE candidate gathering (host candidates are instant;
/// server-reflexive/relay each cost one STUN/TURN round trip) -- generous
/// enough for a live network's worst case without stalling call setup for
/// long if a configured STUN/TURN server is simply unreachable.
const ICE_GATHER_TIMEOUT: Duration = Duration::from_secs(3);

impl DeelipApp {
    /// (server, username, password) if a TURN relay is configured, derived
    /// from the current settings draft.
    pub(crate) fn turn_config(&self) -> Option<(String, String, String)> {
        self.config.turn_server.clone().map(|server| (
            server,
            self.config.turn_username.clone().unwrap_or_default(),
            self.config.turn_password.clone().unwrap_or_default(),
        ))
    }

    /// Attempt to gather local ICE candidates for a new call, bounded by
    /// `ICE_GATHER_TIMEOUT` -- returns `None` (never an error the caller must
    /// handle) if ICE is disabled, no STUN/TURN server is configured, or
    /// gathering fails/times out, so every call site can fall back to the
    /// existing `resolve_rtp_endpoint` path exactly as if ICE didn't exist.
    pub(crate) fn try_gather_ice(&self, is_controlling: bool) -> Option<IceGathered> {
        if !self.config.ice_enabled { return None; }
        let stun = self.config.stun_server.clone();
        let turn = self.turn_config();
        if stun.is_none() && turn.is_none() { return None; }
        let turn_ref = turn.as_ref().map(|(s, u, p)| (s.as_str(), u.as_str(), p.as_str()));
        match self.rt.block_on(deelip_nat::ice::gather(stun.as_deref(), turn_ref, is_controlling, ICE_GATHER_TIMEOUT)) {
            Ok(gathered) => Some(gathered),
            Err(e) => { tracing::warn!("ICE candidate gathering failed/timed out, falling back: {e}"); None }
        }
    }

    /// Finish an in-progress ICE negotiation once the remote's SDP (offer or
    /// answer, whichever direction) is known, running connectivity checks
    /// and returning the winning connection. Only used for the *first* SDP
    /// exchange of a new call -- `None` if `gathered` is `None` (ICE wasn't
    /// attempted) or the remote's SDP didn't itself signal ICE support.
    ///
    /// Note this is a *different* failure mode than `try_gather_ice`
    /// returning `None`: by the time this runs, our own offer/answer has
    /// already been sent committing the far end to our gathered default
    /// candidate's address (that's the whole point of RFC 8445's mandated
    /// default-candidate-in-c=/m= backwards-compatibility rule). If
    /// connectivity checks then fail here, there's no clean way back to the
    /// plain `resolve_rtp_endpoint` path post-commitment (it would bind a
    /// different socket than the address already promised in the sent SDP)
    /// -- this is an inherent structural limit of ICE's gather-then-commit
    /// shape, not a bug, and the call is simply left without working media
    /// in that case, same as any other NAT-traversal failure this codebase
    /// never protected against pre-ICE either.
    pub(crate) fn finish_ice_connect(&self, gathered: Option<IceGathered>, is_controlling: bool, remote_sdp: &str) -> Option<IceConnection> {
        let gathered = gathered?;
        let parsed = parse_sdp(remote_sdp, &sdp::ALL_CODECS)?;
        let ufrag = parsed.ice_ufrag?;
        let pwd = parsed.ice_pwd?;
        if parsed.ice_candidates.is_empty() { return None; }
        match self.rt.block_on(deelip_nat::ice::connect(gathered, is_controlling, &ufrag, &pwd, &parsed.ice_candidates)) {
            Ok(conn) => Some(conn),
            Err(e) => { tracing::warn!("ICE connectivity checks failed, falling back: {e}"); None }
        }
    }

    /// For the answerer side: if the incoming `offer` signaled ICE support
    /// (has ufrag/pwd/candidates) and ICE is enabled locally, gather our own
    /// candidates and run connectivity checks immediately -- unlike the
    /// offerer side (`finish_ice_connect`), the answerer already knows the
    /// remote's ICE parameters up front, straight from the offer, so there's
    /// no need to wait for a later event. Returns our own `IceAttrs` for the
    /// answer SDP, our default candidate's address for the plain c=/m= line,
    /// and the winning connection -- all three or nothing.
    pub(crate) fn try_answer_with_ice(&self, offer: &ParsedSdp) -> Option<(IceAttrs, SocketAddr, IceConnection)> {
        if !self.config.ice_enabled { return None; }
        let ufrag = offer.ice_ufrag.as_deref()?;
        let pwd = offer.ice_pwd.as_deref()?;
        if offer.ice_candidates.is_empty() { return None; }
        let gathered = self.try_gather_ice(false)?;
        let attrs = IceAttrs { ufrag: gathered.local_ufrag.clone(), pwd: gathered.local_pwd.clone(), candidates: gathered.candidates.clone() };
        let default_addr = gathered.default_addr;
        match self.rt.block_on(deelip_nat::ice::connect(gathered, false, ufrag, pwd, &offer.ice_candidates)) {
            Ok(conn) => Some((attrs, default_addr, conn)),
            Err(e) => { tracing::warn!("ICE connectivity checks failed, falling back: {e}"); None }
        }
    }

    /// Start (or restart, on resume) media for `calls[idx]`, using its own
    /// stored codec/srtp/relay/local_rtp — marks it `focused_call` on success.
    pub(crate) fn start_media(&mut self, idx: usize, remote_sdp: &str) {
        let acc_codecs = account_codecs(&self.accounts[self.calls[idx].account].account);
        let Some(parsed) = parse_sdp(remote_sdp, &acc_codecs) else {
            tracing::error!("Cannot parse remote SDP");
            return;
        };
        self.calls[idx].codec     = parsed.codec;
        self.calls[idx].dtmf_type = parsed.dtmf_type;

        let secure = self.accounts.get(self.calls[idx].account).is_some_and(|a| a.handle.secure);
        let srtp_session = match (&self.calls[idx].local_srtp, &parsed.srtp) {
            (Some(local), Some(remote)) => Some(SrtpSession { local: local.clone(), remote: remote.clone() }),
            _ => {
                if secure {
                    tracing::warn!("TLS signaling active but remote SDP has no a=crypto — falling back to plaintext RTP");
                }
                None
            }
        };

        let port    = self.calls[idx].local_rtp;
        let relay   = self.calls[idx].ice.as_ref().map(|i| i.conn.clone())
            .or_else(|| self.calls[idx].relay.as_ref().map(|r| r.conn.clone()));
        let rt      = self.rt.clone();
        let input_device  = self.config.audio.input_device.clone();
        let output_device = self.config.audio.output_device.clone();
        let engine  = rt.block_on(MediaEngine::start(
            port, parsed.rtp_addr, parsed.codec, parsed.dtmf_type, srtp_session, relay,
            self.config.audio.echo_cancellation,
            input_device.as_deref(), output_device.as_deref(),
            self.config.recording_enabled, &self.calls[idx].call_id,
            None,
        ));
        match engine {
            Ok(e)  => { self.media = Some(e); self.focused_call = Some(idx); }
            Err(e) => { tracing::error!("MediaEngine failed: {e}"); }
        }
    }

    /// Merge the two currently-connected calls into a local 3-way
    /// conference: stops the single-leg `MediaEngine` and starts a
    /// conference-mode one bridging both remote parties into the same
    /// mic/speaker pair. Needs no new SIP signaling -- both remote parties
    /// stay in an ordinary 2-party call with DeeLip; only local audio
    /// mixing changes.
    pub(crate) fn start_conference(&mut self) {
        if self.calls.len() != 2 { return; }

        let codecs0 = account_codecs(&self.accounts[self.calls[0].account].account);
        let Some(parsed0) = parse_sdp(&self.calls[0].remote_sdp, &codecs0) else {
            tracing::error!("Cannot parse call 0's remote SDP for conference");
            return;
        };
        let codecs1 = account_codecs(&self.accounts[self.calls[1].account].account);
        let Some(parsed1) = parse_sdp(&self.calls[1].remote_sdp, &codecs1) else {
            tracing::error!("Cannot parse call 1's remote SDP for conference");
            return;
        };
        self.calls[0].codec     = parsed0.codec;
        self.calls[0].dtmf_type = parsed0.dtmf_type;
        self.calls[1].codec     = parsed1.codec;
        self.calls[1].dtmf_type = parsed1.dtmf_type;

        let secure0 = self.accounts.get(self.calls[0].account).is_some_and(|a| a.handle.secure);
        let srtp0 = match (&self.calls[0].local_srtp, &parsed0.srtp) {
            (Some(l), Some(r)) => Some(SrtpSession { local: l.clone(), remote: r.clone() }),
            _ => {
                if secure0 { tracing::warn!("TLS signaling active but remote SDP has no a=crypto (leg0) — falling back to plaintext RTP"); }
                None
            }
        };
        let secure1 = self.accounts.get(self.calls[1].account).is_some_and(|a| a.handle.secure);
        let srtp1 = match (&self.calls[1].local_srtp, &parsed1.srtp) {
            (Some(l), Some(r)) => Some(SrtpSession { local: l.clone(), remote: r.clone() }),
            _ => {
                if secure1 { tracing::warn!("TLS signaling active but remote SDP has no a=crypto (leg1) — falling back to plaintext RTP"); }
                None
            }
        };

        // Any held leg was put on hold with a=sendonly, telling the far end
        // to stop sending us audio -- send a real resume re-INVITE
        // (a=sendrecv) so it actually resumes before we start mixing it
        // in, or that leg would come through silent even though we're now
        // "listening" locally (this is exactly the case for a call held
        // as part of the attended-transfer consultation flow, and equally
        // for an ordinary call-waiting pair where one side is on hold).
        let mut resumed = false;
        if self.calls[0].is_held { self.send_resume(0); resumed = true; }
        if self.calls[1].is_held { self.send_resume(1); resumed = true; }
        if resumed {
            // Fire-and-forget like hold/resume already is everywhere else in
            // this codebase, but this one case is more timing-sensitive than
            // usual: we're about to tear down and rebuild the whole engine
            // right after, so give the far end a brief moment to actually
            // process the re-INVITE and resume sending before we do (same
            // precedent as `hangup_before_exit`'s post-BYE grace sleep).
            // (See `hangup_before_exit` for why this must be an async block,
            // not a bare `tokio::time::sleep(...)` argument.)
            self.rt.block_on(async { tokio::time::sleep(Duration::from_millis(300)).await });
        }

        if let Some(engine) = self.media.take() { self.rt.block_on(engine.stop()); }

        let port0  = self.calls[0].local_rtp;
        let relay0 = self.calls[0].relay.as_ref().map(|r| r.conn.clone());
        let port1  = self.calls[1].local_rtp;
        let relay1 = self.calls[1].relay.as_ref().map(|r| r.conn.clone());
        let rt     = self.rt.clone();
        let input_device  = self.config.audio.input_device.clone();
        let output_device = self.config.audio.output_device.clone();

        let leg2 = ConferenceLeg {
            local_rtp_port: port1,
            remote_rtp: parsed1.rtp_addr,
            codec: parsed1.codec,
            dtmf_pt: parsed1.dtmf_type,
            srtp: srtp1,
            relay: relay1,
        };

        let engine = rt.block_on(MediaEngine::start(
            port0, parsed0.rtp_addr, parsed0.codec, parsed0.dtmf_type, srtp0, relay0,
            self.config.audio.echo_cancellation,
            input_device.as_deref(), output_device.as_deref(),
            self.config.recording_enabled, &self.calls[0].call_id,
            Some(leg2),
        ));
        match engine {
            Ok(e) => {
                self.media = Some(e);
                self.focused_call = Some(0);
                self.calls[0].is_held = false;
                self.calls[1].is_held = false;
                self.in_conference = true;
                self.attended_transfer_original = None;
                self.status_line = "In conference".into();
            }
            Err(e) => tracing::error!("Conference MediaEngine failed: {e}"),
        }
    }

    /// Resolve the (ip, port) to advertise in an SDP offer/answer, using
    /// `advertised_ip` as the direct-path fallback. Allocates a TURN relay on
    /// first use if one is configured, storing it into `relay` for reuse
    /// across hold/resume within that same call. Not a method (despite living
    /// in `impl DeelipApp`) so it can be called with `relay` borrowed from
    /// `self.calls[idx].relay` without aliasing `self`.
    pub(crate) fn resolve_rtp_endpoint(
        rt: &Handle,
        turn_config: Option<(String, String, String)>,
        advertised_ip: &str,
        local_rtp: u16,
        relay: &mut Option<TurnRelay>,
    ) -> (String, u16) {
        if relay.is_none() {
            if let Some((server, username, password)) = turn_config {
                match rt.block_on(deelip_nat::allocate_relay(&server, &username, &password)) {
                    Ok(r) => *relay = Some(r),
                    Err(e) => tracing::warn!("TURN allocation failed ({e}), falling back to direct"),
                }
            }
        }
        match relay {
            Some(r) => (r.relayed_addr.ip().to_string(), r.relayed_addr.port()),
            None => (advertised_ip.to_string(), local_rtp),
        }
    }
}
