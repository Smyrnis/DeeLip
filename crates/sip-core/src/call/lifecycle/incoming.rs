//! Incoming call handling: `on_invite` (fresh INVITE or a re-INVITE on an
//! already-confirmed dialog), `accept_call`/`on_incoming_answer_ready` (the
//! same background-resolve-then-finish shape `initiate_call`/
//! `on_outgoing_offer_ready` use for the outgoing side), and `reject_call`.

use std::net::SocketAddr;

use tracing::{debug, error};

use super::{DialogRequestContext, VIDEO_CODECS};
use crate::{
    call::dialog::{Dialog, DialogState},
    call::media_setup::{self, DtlsCallParams, NetworkConfig},
    client::{IncomingVideoAnswer, SipStack, StackEvent},
    events::SipEvent,
    wire::message::{SipMessage, SipMethod},
    wire::sdp::{
        Setup, SrtpParams, build_answer, build_video_media_section, parse_sdp_forcing, parse_video_section,
        split_media_sections,
    },
    wire::util::{new_tag, parse_tag, parse_uri},
};

/// RFC 4145 §4.1 / RFC 5763 §5: when the offer proposes `actpass`, the
/// answerer picks which side initiates the DTLS handshake -- either is
/// valid, but this default (`Active`, i.e. we send the ClientHello) is
/// UNVERIFIED against a real interop peer. If the offer instead pins a
/// specific role, we must take the complementary one (RFC 4145: the two
/// sides can't both be active or both passive).
fn resolve_answerer_dtls_role(offered_setup: Option<Setup>) -> Setup {
    match offered_setup {
        Some(Setup::Active) => Setup::Passive,
        Some(Setup::Passive) => Setup::Active,
        Some(Setup::ActPass) | None => Setup::Active,
    }
}

impl SipStack {
    // ── Incoming INVITE ───────────────────────────────────────────────────────

    pub(crate) async fn on_invite(&mut self, msg: SipMessage, from: SocketAddr) {
        let call_id = match msg.call_id() {
            Some(id) => id.to_string(),
            None => return,
        };

        // re-INVITE on existing confirmed dialog -- either a real hold/resume
        // or (RFC 4028) a session-refresh carrying the *same* direction as
        // before, which must not be mistaken for a resume (see `was_held` below).
        let reinvite_action = if let Some(dialog) = self.dialogs.get_mut(&call_id) {
            if dialog.state == DialogState::Confirmed {
                let body = String::from_utf8_lossy(&msg.body).into_owned();
                let is_sendonly = body.lines().any(|l| l.trim() == "a=sendonly");
                let local_sdp = dialog.local_sdp.clone().unwrap_or_default();
                let local_tag = dialog.local_tag.clone();
                let was_held = dialog.is_held;
                dialog.is_held = is_sendonly;
                // Any re-INVITE (hold, resume, or a plain session refresh)
                // proves the dialog is still alive -- reset our own refresh
                // deadline too, so a far-end-initiated refresh (they're
                // `refresher=uas` from our perspective) doesn't race a
                // redundant one of our own.
                if let Some(interval) = dialog.session_expires {
                    dialog.session_refresh_at = Some(
                        tokio::time::Instant::now() + tokio::time::Duration::from_secs((interval / 2).max(1) as u64),
                    );
                }
                Some((is_sendonly, was_held, local_sdp, local_tag))
            } else {
                None
            }
        } else {
            None
        };
        if let Some((is_sendonly, was_held, local_sdp, local_tag)) = reinvite_action {
            let ok = self.build_response_with_body(&msg, 200, "OK", &local_tag, &local_sdp);
            let _ = self.transport.send(ok.as_bytes(), from).await;
            let ev = if is_sendonly {
                Some(SipEvent::RemoteHeld { call_id: call_id.clone() })
            } else if was_held {
                Some(SipEvent::RemoteResumed { call_id: call_id.clone() })
            } else {
                // Same direction as before (typically a session-refresh
                // re-INVITE with unchanged SDP) -- nothing actually changed,
                // so there's nothing to notify the UI about.
                None
            };
            if let Some(ev) = ev {
                let _ = self.event_tx.send(ev);
            }
            return;
        }

        // Fresh INVITE
        let from_hdr = msg.header("From").unwrap_or("").to_string();
        let from_uri = parse_uri(&from_hdr).unwrap_or_else(|| from_hdr.clone());
        let from_tag = parse_tag(&from_hdr).unwrap_or_default();
        let (cseq_n, _) = msg.cseq().unwrap_or((1, SipMethod::Invite));
        let remote_sdp = String::from_utf8_lossy(&msg.body).into_owned();
        let remote_via = msg.header("Via").unwrap_or("").to_string();
        let local_tag = new_tag();

        debug!("← INVITE from {from_uri} ({from})");
        let trying = self.build_response(&msg, 100, "Trying", &local_tag, "");
        let ringing = self.build_response(&msg, 180, "Ringing", &local_tag, "");
        let _ = self.transport.send(trying.as_bytes(), from).await;
        let _ = self.transport.send(ringing.as_bytes(), from).await;

        let mut dialog = Dialog::new_incoming(
            call_id.clone(),
            local_tag,
            from_uri.clone(),
            from_tag,
            cseq_n,
            remote_sdp.clone(),
            remote_via,
        );
        dialog.remote_contact = Some(from.to_string());
        // RFC 4028 Session Timers: stash the caller's proposal (if any) for
        // `accept_call`/`on_incoming_answer_ready` to echo back in the 200
        // OK -- `self.account.session_timers_enabled` is checked there, not
        // here, since accepting is what actually commits to it.
        dialog.incoming_session_expires =
            msg.header("Session-Expires").and_then(crate::wire::util::parse_session_expires);
        self.dialogs.insert(call_id.clone(), dialog);

        let remote_answer_after = crate::wire::util::parse_call_info_answer_after(&msg);
        let _ = self.event_tx.send(SipEvent::IncomingCall { call_id, from: from_uri, remote_answer_after });
    }

    /// Check codec compatibility and allocate a local RTP port -- both fast,
    /// local operations -- then kick off STUN/TURN/ICE resolution for our
    /// answer on a background task (see `StackEvent`'s doc comment for why).
    /// Declines with 486 (via `reject_call`) and emits `SipEvent::CallFailed`
    /// immediately if no mutually-acceptable codec is found or a local RTP
    /// port can't be allocated; otherwise the 200 OK isn't sent and
    /// `SipEvent::CallConnected` isn't emitted until the background task
    /// reports back via `on_incoming_answer_ready`.
    pub(crate) async fn accept_call(&mut self, call_id: &str) {
        let Some(dialog) = self.dialogs.get(call_id) else {
            return;
        };
        let remote_sdp = dialog.remote_sdp.clone().unwrap_or_default();

        let codecs = media_setup::account_codecs(&self.account);
        let force_codec = self.account.force_incoming_codec.as_deref().and_then(media_setup::codec_from_str);
        let Some(parsed) = parse_sdp_forcing(&remote_sdp, &codecs, force_codec) else {
            self.reject_call(call_id).await;
            let _ = self.event_tx.send(SipEvent::CallFailed {
                call_id: call_id.to_string(),
                code: 488,
                reason: "No compatible codec".into(),
            });
            return;
        };

        // Video (negotiation only): a separate, independent parse of the
        // same `remote_sdp` via `wire/sdp/video.rs`'s section-aware helpers,
        // never folded into `parsed`/`ParsedSdp` itself -- see this file's
        // `VIDEO_CODECS`/`prepare_video_answer` doc comments. `None` if the
        // account doesn't have video enabled or the remote didn't offer it.
        let parsed_video = if self.account.video_enabled {
            split_media_sections(&remote_sdp)
                .into_iter()
                .find(|(m_line, _)| m_line.starts_with("m=video "))
                .and_then(|(m_line, attrs)| parse_video_section(m_line, &attrs, &VIDEO_CODECS))
        } else {
            None
        };

        let local_rtp = match deelip_nat::alloc_rtp_port(self.network.rtp_port_range) {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to allocate local RTP port: {e}");
                self.reject_call(call_id).await;
                let _ = self.event_tx.send(SipEvent::CallFailed {
                    call_id: call_id.to_string(),
                    code: 0,
                    reason: "Local RTP port allocation failed".into(),
                });
                return;
            }
        };

        let network = self.network.clone();
        let advertised_ip = self.advertised_ip.clone();
        let wants_srtp = self.account.wants_srtp(self.resolved_transport);
        let wants_dtls_srtp = self.account.wants_dtls_srtp();
        let wants_ice = self.account.wants_ice(self.network.ice_enabled);
        let vad_enabled = self.account.vad_enabled;
        let internal_tx = self.internal_tx.clone();
        let call_id = call_id.to_string();
        tokio::spawn(async move {
            let mut relay = None;
            let ice_result = media_setup::try_answer_with_ice(&network, wants_ice, &parsed).await;
            let (rtp_ip, rtp_port, ice_attrs, ice) = match ice_result {
                Some((attrs, addr, conn)) => (addr.ip().to_string(), addr.port(), Some(attrs), Some(conn)),
                None => {
                    let (ip, port) =
                        media_setup::resolve_rtp_endpoint(&network, &advertised_ip, local_rtp, &mut relay).await;
                    (ip, port, None, None)
                }
            };

            let local_srtp = if wants_srtp { Some(SrtpParams::generate()) } else { None };
            // Only attempted if the offer actually carried a fingerprint --
            // otherwise the remote doesn't support DTLS-SRTP and we fall
            // back to whatever SDES/plaintext `wants_srtp` already decided.
            let local_dtls = if wants_dtls_srtp && let Some(remote_fingerprint) = parsed.fingerprint.clone() {
                match DtlsCallParams::generate() {
                    Ok(mut params) => {
                        params.role = Some(resolve_answerer_dtls_role(parsed.setup));
                        params.remote_fingerprint = Some(remote_fingerprint);
                        Some(params)
                    }
                    Err(e) => {
                        error!("Failed to generate DTLS-SRTP certificate: {e:#}");
                        None
                    }
                }
            } else {
                None
            };
            let mut local_sdp = build_answer(
                &rtp_ip,
                rtp_port,
                parsed.codec,
                local_srtp.as_ref(),
                ice_attrs.as_ref(),
                vad_enabled,
                local_dtls.as_ref().map(|d| &d.local_fingerprint),
                local_dtls.as_ref().and_then(|d| d.role),
            );

            let video = Self::prepare_video_answer(
                &network,
                &advertised_ip,
                wants_ice,
                wants_srtp,
                local_dtls.as_ref(),
                parsed_video,
                &mut local_sdp,
            )
            .await;

            let _ = internal_tx.send(StackEvent::IncomingAnswerReady {
                call_id,
                parsed,
                local_sdp,
                local_rtp,
                local_srtp,
                relay,
                ice,
                local_dtls,
                video,
            });
        });
    }

    /// Answerer-side counterpart of `prepare_video_offer`: given the
    /// remote's already-parsed video offer (if any) and this account's
    /// ICE/SRTP policy, resolve our own video RTP endpoint/ICE/SRTP and
    /// append the answer's own video `m=` section to `local_sdp`. Returns
    /// `None` (leaving `local_sdp` unmodified) if the remote didn't offer
    /// video or our own port allocation fails -- video is always
    /// additive, never a reason to decline the whole call.
    async fn prepare_video_answer(
        network: &NetworkConfig, advertised_ip: &str, wants_ice: bool, wants_srtp: bool,
        local_dtls: Option<&DtlsCallParams>, parsed_video: Option<crate::wire::sdp::ParsedVideoMedia>,
        local_sdp: &mut String,
    ) -> Option<IncomingVideoAnswer> {
        let parsed = parsed_video?;
        let local_rtp = match deelip_nat::alloc_rtp_port(network.rtp_port_range) {
            Ok(p) => p,
            Err(e) => {
                error!("Video: failed to allocate local RTP port ({e}), continuing audio-only");
                return None;
            }
        };
        let mut relay = None;
        let ice_result = media_setup::try_answer_with_ice_raw(
            network,
            wants_ice,
            parsed.ice_ufrag.as_deref(),
            parsed.ice_pwd.as_deref(),
            &parsed.ice_candidates,
        )
        .await;
        let (rtp_ip, rtp_port, ice_attrs, ice) = match ice_result {
            Some((attrs, addr, conn)) => (addr.ip().to_string(), addr.port(), Some(attrs), Some(conn)),
            None => {
                let (ip, port) = media_setup::resolve_rtp_endpoint(network, advertised_ip, local_rtp, &mut relay).await;
                (ip, port, None, None)
            }
        };
        let local_srtp = if wants_srtp { Some(crate::wire::sdp::SrtpParams::generate()) } else { None };
        local_sdp.push_str(&build_video_media_section(
            &rtp_ip,
            rtp_port,
            parsed.codec,
            local_srtp.as_ref(),
            ice_attrs.as_ref(),
            local_dtls.map(|d| &d.local_fingerprint),
            local_dtls.and_then(|d| d.role),
        ));

        Some(IncomingVideoAnswer { parsed, local_rtp, local_srtp, relay, ice })
    }

    /// Finish accepting an incoming call once `accept_call`'s background
    /// answer resolution completes: send the 200 OK, fold the result into
    /// `CallMedia`/`CallMediaReady`, and emit `CallConnected`. A no-op if the
    /// dialog is already gone (CANCELed/hung-up while this was resolving) --
    /// there's nothing left to answer.
    pub(crate) async fn on_incoming_answer_ready(&mut self, ev: StackEvent) {
        let StackEvent::IncomingAnswerReady {
            call_id,
            parsed,
            local_sdp,
            local_rtp,
            local_srtp,
            relay,
            ice,
            local_dtls,
            video,
        } = ev
        else {
            unreachable!("caller already matched this variant")
        };

        let identity = self.stack_identity();
        let Some(dialog) = self.dialogs.get_mut(&call_id) else {
            return;
        };

        let cseq_n = dialog.remote_cseq.unwrap_or(1);
        let ctx = DialogRequestContext::new(&identity, dialog);
        let contact = ctx.contact;
        let call_id_str = &ctx.call_id;
        let local_tag = &ctx.local_tag;
        let from_tag_part = &ctx.remote_tag_param;
        let remote_uri = &ctx.remote_uri;
        let remote_via = &ctx.remote_via;
        let adv_ip = &ctx.adv_ip;
        let local_port = ctx.local_port;
        let username = &ctx.username;
        let server = &ctx.server;
        let display = &ctx.display;
        let contact_transport = ctx.contact_transport;
        let body_len = local_sdp.len();

        // RFC 4028 Session Timers: our own 2xx response's `refresher=` takes
        // highest precedence per the RFC's own resolution rules, so we
        // decide unilaterally here rather than just echoing the caller's
        // request -- honor an explicit "uas" ask (they want us to refresh),
        // otherwise default to "uac" (them), same default-favors-caller rule
        // the outgoing-call side uses in `on_response`.
        let session_timer_info = if self.account.session_timers_enabled {
            dialog.incoming_session_expires.take().map(|(interval, their_refresher)| {
                let we_are_refresher = their_refresher.as_deref() == Some("uas");
                (interval, we_are_refresher)
            })
        } else {
            None
        };
        let session_timer_hdr = session_timer_info
            .map(|(interval, we_are_refresher)| {
                let echoed = if we_are_refresher { "uas" } else { "uac" };
                format!("Supported: timer\r\nSession-Expires: {interval};refresher={echoed}\r\n")
            })
            .unwrap_or_default();
        let user_agent = crate::USER_AGENT;

        let ok_msg = format!(
            "SIP/2.0 200 OK\r\n\
             Via: {remote_via}\r\n\
             To: \"{display}\" <sip:{username}@{server}>;tag={local_tag}\r\n\
             From: <{remote_uri}>{from_tag_part}\r\n\
             Call-ID: {call_id_str}\r\n\
             CSeq: {cseq_n} INVITE\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
             Content-Type: application/sdp\r\n\
             User-Agent: {user_agent}\r\n\
             {session_timer_hdr}\
             Content-Length: {body_len}\r\n\r\n\
             {local_sdp}"
        );

        let _ = self.transport.send(ok_msg.as_bytes(), contact).await;

        let wants_srtp = self.account.wants_srtp(self.resolved_transport);
        let (mut media, mut ready) = media_setup::resolve_call_media(
            local_rtp,
            local_srtp,
            relay,
            ice,
            parsed.codec,
            parsed.dtmf_type,
            parsed.cn_type,
            parsed.rtp_addr,
            parsed.srtp,
            wants_srtp,
            local_dtls,
        );
        debug!(call_id = %call_id_str, video_negotiated = video.is_some(), "Incoming call answered");
        if let Some(v) = video {
            let (video_media, video_ready) = media_setup::resolve_video_media(
                v.local_rtp,
                v.local_srtp,
                v.relay,
                v.ice,
                v.parsed.codec,
                v.parsed.rtp_addr,
                v.parsed.srtp,
                wants_srtp,
            );
            media.video = Some(video_media);
            ready.video = Some(video_ready);
        }

        let dialog =
            self.dialogs.get_mut(&call_id).expect("dialog present -- checked above, nothing removes it in between");
        dialog.state = DialogState::Confirmed;
        dialog.local_sdp = Some(local_sdp);
        dialog.media = Some(media);
        if let Some((interval, we_are_refresher)) = session_timer_info {
            dialog.session_expires = Some(interval);
            dialog.we_are_refresher = we_are_refresher;
            if we_are_refresher {
                dialog.session_refresh_at =
                    Some(tokio::time::Instant::now() + tokio::time::Duration::from_secs((interval / 2).max(1) as u64));
            }
        }

        let _ = self.event_tx.send(SipEvent::CallConnected { call_id, media: ready });
    }

    pub(crate) async fn reject_call(&mut self, call_id: &str) {
        if let Some(dialog) = self.dialogs.remove(call_id) {
            let identity = self.stack_identity();
            let cseq_n = dialog.remote_cseq.unwrap_or(1);
            let ctx = DialogRequestContext::new(&identity, &dialog);
            let contact = ctx.contact;
            let username = &ctx.username;
            let server = &ctx.server;
            let display = &ctx.display;
            let local_tag = &ctx.local_tag;
            let remote_uri = &ctx.remote_uri;
            let remote_via = &ctx.remote_via;
            let from_tag = &ctx.remote_tag_param;

            let decline = format!(
                "SIP/2.0 486 Busy Here\r\n\
                 Via: {remote_via}\r\n\
                 To: \"{display}\" <sip:{username}@{server}>;tag={local_tag}\r\n\
                 From: <{remote_uri}>{from_tag}\r\n\
                 Call-ID: {call_id}\r\n\
                 CSeq: {cseq_n} INVITE\r\n\
                 Content-Length: 0\r\n\r\n"
            );
            debug!("→ 486 Busy Here {call_id} to {contact}");
            if let Err(e) = self.transport.send(decline.as_bytes(), contact).await {
                error!("Failed to send 486 for {call_id}: {e}");
            }
        }
    }
}
