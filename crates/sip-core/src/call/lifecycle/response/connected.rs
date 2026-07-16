//! Initial-INVITE-connected handling for `SipStack::on_response` -- split
//! out of `response.rs` purely for file size (same precedent as
//! `views/settings/`, `views/dialer/`, `sip-core/src/call/lifecycle/`), not
//! a behavior change. One logical flow: parse the answer, resolve the
//! DTLS-SRTP role, spawn the background ICE-connect resolution, then fold
//! its result into `CallMedia`/`CallMediaReady` once it completes.

use tracing::debug;

use crate::call::dialog::PendingOfferMedia;
use crate::call::lifecycle::VIDEO_CODECS;
use crate::call::media_setup;
use crate::client::{OutgoingVideoConnected, SipStack, StackEvent};
use crate::events::SipEvent;
use crate::wire::sdp::{Setup, parse_sdp, parse_video_section, split_media_sections};
use crate::wire::util::new_branch;

/// RFC 4145 §4.1: as the offerer, our final role is simply the complement
/// of whatever the answer committed to. `None` if the answer didn't
/// actually negotiate DTLS-SRTP at all (no fingerprint/setup, or an
/// answer nonsensically echoing `actpass` instead of resolving it) --
/// callers treat that the same as the remote not supporting DTLS-SRTP.
fn resolve_offerer_dtls_role(answered_setup: Option<Setup>) -> Option<Setup> {
    match answered_setup {
        Some(Setup::Active) => Some(Setup::Passive),
        Some(Setup::Passive) => Some(Setup::Active),
        Some(Setup::ActPass) | None => None,
    }
}

impl SipStack {
    #[allow(clippy::too_many_arguments)] // mirrors the `Act::Connected` variant's own
    // field set one-for-one; a struct wrapper would
    // just rename this same list, not shrink it.
    pub(super) async fn handle_connected(
        &mut self, call_id: String, remote_sdp: String, pending_offer: Option<PendingOfferMedia>,
        ice_gathered: Option<deelip_nat::IceGathered>, ack_cid: String, ack_from_tag: String, ack_to_uri: String,
        ack_to_tag: Option<String>, ack_cseq: u32, session_expires_hdr: Option<(u32, Option<String>)>,
    ) {
        // A 2xx ACK is a new transaction in its own right (RFC 3261
        // §13.2.2.4) -- unlike a non-2xx ACK, it gets a fresh branch.
        let ack = self.build_ack(&ack_cid, &ack_from_tag, &ack_to_uri, ack_to_tag.as_deref(), ack_cseq, &new_branch());
        let _ = self.transport.send(ack.as_bytes(), self.server_addr).await;

        // RFC 4028 Session Timers: fold the response's negotiated
        // `Session-Expires`/`refresher=` into the dialog -- absent
        // entirely if the far end doesn't support them (no
        // `Session-Expires` in the 200 OK), regardless of whether we
        // proposed one ourselves.
        if let Some((interval, refresher)) = session_expires_hdr
            && let Some(dialog) = self.dialogs.get_mut(&call_id)
        {
            dialog.session_expires = Some(interval);
            dialog.we_are_refresher = refresher.as_deref() != Some("uas");
            if dialog.we_are_refresher {
                dialog.session_refresh_at =
                    Some(tokio::time::Instant::now() + tokio::time::Duration::from_secs((interval / 2).max(1) as u64));
            }
        }

        let codecs = media_setup::account_codecs(&self.account);
        let Some(parsed) = parse_sdp(&remote_sdp, &codecs) else {
            // Must send a real BYE here, not just drop the dialog -- the 2xx
            // is already ACKed. See docs/crates/sip-core.md's
            // "handle_connected's post-ACK codec-mismatch teardown" note.
            self.hang_up(&call_id).await;
            self.dialogs.remove(&call_id);
            let _ = self.event_tx.send(SipEvent::CallFailed {
                call_id,
                code: 0,
                reason: "No compatible codec in answer".into(),
            });
            return;
        };
        let Some(PendingOfferMedia { local_rtp, local_srtp, relay, local_dtls }) = pending_offer else {
            debug!(call_id, "Connected with no pending offer media -- dropping");
            return;
        };

        // Resolve our final DTLS-SRTP role now that the answer's
        // `a=setup` is known -- `None` (dropping `local_dtls` entirely)
        // if the answer didn't actually negotiate it, same fallback
        // shape `resolve_srtp_and_relay` already uses for SDES.
        let local_dtls = local_dtls.and_then(|mut params| match resolve_offerer_dtls_role(parsed.setup) {
            Some(role) if parsed.fingerprint.is_some() => {
                params.role = Some(role);
                params.remote_fingerprint = parsed.fingerprint.clone();
                Some(params)
            }
            _ => {
                tracing::warn!("DTLS-SRTP requested but answer didn't negotiate it -- falling back to plaintext RTP");
                None
            }
        });

        // Video (negotiation only): if we offered a video leg,
        // parse the answer's own video section (a separate,
        // independent parse of `remote_sdp` -- never folded into
        // `parsed`/`ParsedSdp`, same as `accept_call`'s treatment).
        let pending_offer_video = self.dialogs.get_mut(&call_id).and_then(|d| d.pending_offer_video.take());
        let parsed_video = pending_offer_video.as_ref().and_then(|_| {
            split_media_sections(&remote_sdp)
                .into_iter()
                .find(|(m_line, _)| m_line.starts_with("m=video "))
                .and_then(|(m_line, attrs)| parse_video_section(m_line, &attrs, &VIDEO_CODECS))
        });

        // ICE connectivity checks (RFC 8445) are real network I/O,
        // same reasoning as `initiate_call`/`accept_call` -- defer to
        // a background task rather than blocking this whole account's
        // event loop on `finish_ice_connect`'s multi-second timeout.
        let internal_tx = self.internal_tx.clone();
        let remote_rtp = parsed.rtp_addr;
        let remote_srtp = parsed.srtp.clone();
        let codec = parsed.codec;
        let dtmf_type = parsed.dtmf_type;
        let cn_type = parsed.cn_type;
        tokio::spawn(async move {
            let ice = media_setup::finish_ice_connect(ice_gathered, true, &parsed).await;

            let video = match (pending_offer_video, parsed_video) {
                (Some(pending_video), Some(parsed_video)) => {
                    let video_ice = media_setup::finish_ice_connect_raw(
                        pending_video.ice_gathered,
                        true,
                        parsed_video.ice_ufrag.as_deref(),
                        parsed_video.ice_pwd.as_deref(),
                        &parsed_video.ice_candidates,
                    )
                    .await;
                    Some(OutgoingVideoConnected {
                        local_rtp: pending_video.local_rtp,
                        local_srtp: pending_video.local_srtp,
                        relay: pending_video.relay,
                        ice: video_ice,
                        codec: parsed_video.codec,
                        remote_rtp: parsed_video.rtp_addr,
                        remote_srtp: parsed_video.srtp,
                    })
                }
                _ => None,
            };

            let _ = internal_tx.send(StackEvent::OutgoingConnected {
                call_id,
                local_rtp,
                local_srtp,
                relay,
                ice,
                codec,
                dtmf_type,
                cn_type,
                remote_rtp,
                remote_srtp,
                local_dtls,
                video,
            });
        });
    }

    /// Finish connecting an outgoing call once `on_response`'s background
    /// ICE-connect resolution completes: fold the result into `CallMedia`/
    /// `CallMediaReady` and emit `CallConnected`. A no-op if the dialog is
    /// already gone (hung up, CANCELed, or failed some other way while this
    /// was resolving in the background) -- there's nothing left to connect.
    pub(crate) async fn on_outgoing_connected(&mut self, ev: StackEvent) {
        let StackEvent::OutgoingConnected {
            call_id,
            local_rtp,
            local_srtp,
            relay,
            ice,
            codec,
            dtmf_type,
            cn_type,
            remote_rtp,
            remote_srtp,
            local_dtls,
            video,
        } = ev
        else {
            unreachable!("caller already matched this variant")
        };

        if !self.dialogs.contains_key(&call_id) {
            return;
        }
        let wants_srtp = self.account.wants_srtp(self.resolved_transport);
        let (mut media, mut ready) = media_setup::resolve_call_media(
            local_rtp,
            local_srtp,
            relay,
            ice,
            codec,
            dtmf_type,
            cn_type,
            remote_rtp,
            remote_srtp,
            wants_srtp,
            local_dtls,
        );
        debug!(call_id, video_negotiated = video.is_some(), "Outgoing call connected");
        if let Some(v) = video {
            let (video_media, video_ready) = media_setup::resolve_video_media(
                v.local_rtp,
                v.local_srtp,
                v.relay,
                v.ice,
                v.codec,
                v.remote_rtp,
                v.remote_srtp,
                wants_srtp,
            );
            media.video = Some(video_media);
            ready.video = Some(video_ready);
        }
        self.dialogs.get_mut(&call_id).expect("checked above").media = Some(media);
        let _ = self.event_tx.send(SipEvent::CallConnected { call_id, media: ready });
    }
}
