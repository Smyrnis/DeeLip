//! Outgoing call setup: `initiate_call` resolves media (RTP port, ICE, SRTP,
//! optional video) on a background task, `on_outgoing_offer_ready` sends the
//! actual INVITE once that resolution completes.

use std::net::SocketAddr;

use tracing::{debug, error};

use deelip_config::TransportProtocol;

use crate::{
    call::dialog::{Dialog, PendingOfferMedia, PendingVideoOffer},
    call::media_setup::{self, NetworkConfig},
    client::{SipStack, StackEvent},
    events::SipEvent,
    wire::sdp::{IceAttrs, SrtpParams, VideoCodec, build_offer, build_video_media_section},
    wire::util::{new_branch, new_call_id, new_tag, uri_host_port},
};

/// Resolve a `SipAccount::local_account` call's actual destination straight
/// from the dialed target's own URI (an IP or hostname, optional `:port`)
/// instead of an outbound proxy -- there isn't one. Always resolves as if
/// for UDP (the only transport a local account supports).
async fn resolve_local_call_target(to: &str, network: &NetworkConfig) -> Option<SocketAddr> {
    let (host, port) = uri_host_port(to)?;
    crate::wire::dns::resolve_target(
        &host,
        port,
        TransportProtocol::Udp,
        network.custom_nameserver.as_deref(),
        network.dns_srv_enabled,
    )
    .await
    .ok()
}

impl SipStack {
    // ── Outgoing call ─────────────────────────────────────────────────────────

    /// `attempt_ice` lets the caller opt this specific call out of ICE even
    /// when it's enabled globally (see `SipCommand::MakeCall`'s doc comment).
    ///
    /// Only allocates the local RTP port and kicks off STUN/TURN/ICE
    /// resolution on a background task -- see `StackEvent`'s doc comment for
    /// why that can't just be `.await`ed inline here. The INVITE itself isn't
    /// built or sent until that task reports back via `on_outgoing_offer_ready`.
    pub(crate) async fn initiate_call(&mut self, to: &str, attempt_ice: bool) {
        let call_id = new_call_id(&self.local_ip);
        let from_tag = new_tag();
        let branch = new_branch();

        let local_rtp = match deelip_nat::alloc_rtp_port(self.network.rtp_port_range) {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to allocate local RTP port: {e}");
                let _ = self.event_tx.send(SipEvent::CallFailed {
                    call_id,
                    code: 0,
                    reason: "Local RTP port allocation failed".into(),
                });
                return;
            }
        };

        let network = self.network.clone();
        let account = self.account.clone();
        let resolved_transport = self.resolved_transport;
        let advertised_ip = self.advertised_ip.clone();
        let internal_tx = self.internal_tx.clone();
        let to = to.to_string();
        let attempt_ice = attempt_ice && account.wants_ice(network.ice_enabled);
        tokio::spawn(async move {
            let mut relay = None;
            let ice_gathered = if attempt_ice { media_setup::try_gather_ice(&network, true, true).await } else { None };
            let ice_attrs = ice_gathered.as_ref().map(|g| IceAttrs {
                ufrag: g.local_ufrag.clone(),
                pwd: g.local_pwd.clone(),
                candidates: g.candidates.clone(),
            });
            // Same reasoning as the pre-move code this replaced: the plain
            // c=/m= fallback address is deliberately never the ICE agent's
            // own gathered candidate socket -- that only becomes usable once
            // the answer confirms the far end also speaks ICE
            // (`on_outgoing_connected`), and if it doesn't, the ICE agent
            // (and that socket) is simply dropped. Advertising it here and
            // binding an unrelated `local_rtp` on connect would leave the far
            // end sending RTP to a socket nothing is listening on.
            let (rtp_ip, rtp_port) =
                media_setup::resolve_rtp_endpoint(&network, &advertised_ip, local_rtp, &mut relay).await;

            let wants_srtp = account.wants_srtp(resolved_transport);
            let srtp = if wants_srtp { Some(SrtpParams::generate()) } else { None };
            let codecs = media_setup::account_codecs(&account);
            let mut local_sdp =
                build_offer(&rtp_ip, rtp_port, srtp.as_ref(), &codecs, ice_attrs.as_ref(), account.vad_enabled);

            // Video (negotiation only -- see this account field's own doc
            // comment): appended onto the audio offer's own SDP text, never
            // changing `build_offer` itself. Any failure along this path
            // (port allocation, ICE gather) just leaves `video` as `None`
            // and the call proceeds audio-only, exactly as it always has.
            let video = if account.video_enabled {
                Self::prepare_video_offer(&network, &advertised_ip, attempt_ice, wants_srtp, &mut local_sdp).await
            } else {
                None
            };

            let _ = internal_tx.send(StackEvent::OutgoingOfferReady {
                call_id,
                from_tag,
                branch,
                to,
                local_sdp,
                pending_offer: PendingOfferMedia { local_rtp, local_srtp: srtp, relay },
                ice_gathered,
                video,
            });
        });
    }

    /// Resolve and append a video `m=` section to an in-progress offer's
    /// `local_sdp` -- allocates its own RTP port, gathers its own ICE
    /// candidates (independent of audio's), and generates its own SRTP key
    /// if `wants_srtp`. Returns the state to carry forward until the answer
    /// arrives (mirrors the audio leg's own `PendingOfferMedia`/
    /// `ice_gathered` pair, bundled into one struct -- see
    /// `PendingVideoOffer`). Returns `None` (leaving `local_sdp`
    /// unmodified) if the video RTP port can't be allocated -- video is
    /// always additive, never a reason to fail the whole call.
    async fn prepare_video_offer(
        network: &NetworkConfig, advertised_ip: &str, attempt_ice: bool, wants_srtp: bool, local_sdp: &mut String,
    ) -> Option<PendingVideoOffer> {
        let local_rtp = match deelip_nat::alloc_rtp_port(network.rtp_port_range) {
            Ok(p) => p,
            Err(e) => {
                error!("Video: failed to allocate local RTP port ({e}), continuing audio-only");
                return None;
            }
        };
        let mut relay = None;
        let ice_gathered = if attempt_ice { media_setup::try_gather_ice(network, true, true).await } else { None };
        let ice_attrs = ice_gathered.as_ref().map(|g| IceAttrs {
            ufrag: g.local_ufrag.clone(),
            pwd: g.local_pwd.clone(),
            candidates: g.candidates.clone(),
        });
        let (rtp_ip, rtp_port) = media_setup::resolve_rtp_endpoint(network, advertised_ip, local_rtp, &mut relay).await;
        let local_srtp = if wants_srtp { Some(SrtpParams::generate()) } else { None };

        local_sdp.push_str(&build_video_media_section(
            &rtp_ip,
            rtp_port,
            VideoCodec::H264,
            local_srtp.as_ref(),
            ice_attrs.as_ref(),
        ));

        Some(PendingVideoOffer { local_rtp, local_srtp, relay, ice_gathered })
    }

    /// Finish placing an outgoing call once `initiate_call`'s background
    /// offer resolution completes: build and send the actual INVITE, then
    /// insert the dialog. Runs back on `run()`'s own task (via `StackEvent`),
    /// so it can freely touch `self.dialogs`/`self.transport` again.
    pub(crate) async fn on_outgoing_offer_ready(&mut self, ev: StackEvent) {
        let StackEvent::OutgoingOfferReady {
            call_id,
            from_tag,
            branch,
            to,
            local_sdp,
            pending_offer,
            ice_gathered,
            video,
        } = ev
        else {
            unreachable!("caller already matched this variant")
        };

        let mut dialog = Dialog::new_outgoing(call_id.clone(), from_tag.clone(), to.clone());
        dialog.local_sdp = Some(local_sdp.clone());
        dialog.invite_branch = branch.clone();
        dialog.ice_gathered = ice_gathered;
        dialog.pending_offer = Some(pending_offer);
        dialog.pending_offer_video = video;

        // `SipAccount::local_account` has no outbound proxy to route through
        // -- resolve the dialed target's own host straight from its URI
        // instead of sending to `self.server_addr` (a never-valid
        // placeholder for a serverless account, see `connect_local`).
        let dest = if self.account.local_account {
            match resolve_local_call_target(&to, &self.network).await {
                Some(addr) => addr,
                None => {
                    error!("Local Account: couldn't resolve call target {to}");
                    let _ = self.event_tx.send(SipEvent::CallFailed {
                        call_id,
                        code: 0,
                        reason: "Couldn't resolve call target".into(),
                    });
                    return;
                }
            }
        } else {
            self.server_addr
        };

        let msg = self.build_invite(
            &dialog.call_id,
            &dialog.local_tag,
            dialog.local_cseq,
            &to,
            &local_sdp,
            None,
            &branch,
            crate::client::SESSION_EXPIRES_DEFAULT,
        );
        debug!("→ INVITE {to}");
        if let Err(e) = self.transport.send(msg.as_bytes(), dest).await {
            error!("Failed to send INVITE: {e}");
            let _ = self.event_tx.send(SipEvent::CallFailed {
                call_id,
                code: 0,
                reason: format!("Failed to send INVITE: {e}"),
            });
            return;
        }
        self.dialogs.insert(call_id, dialog);
    }

    #[allow(clippy::too_many_arguments)] // each param is a distinct, meaningfully-named
    // piece of an INVITE's identity; bundling them
    // into a struct wouldn't reduce real complexity here.
    pub(super) fn build_invite(
        &self, call_id: &str, from_tag: &str, cseq: u32, to: &str, sdp: &str, auth: Option<&str>, branch: &str,
        session_expires: u32,
    ) -> String {
        let identity = self.stack_identity();
        let server = &identity.server;
        let username = &identity.username;
        let adv_ip = &identity.adv_ip;
        let local_ip = &identity.local_ip;
        let local_port = identity.local_port;
        let display = &identity.display;
        let body_len = sdp.len();
        let via_proto = identity.via_proto;
        let contact_transport = identity.contact_transport;
        let via_line = crate::client::build_via(via_proto, local_ip, local_port, branch);
        let contact_line = crate::client::build_contact(username, adv_ip, local_port, contact_transport);
        let user_agent = crate::USER_AGENT;

        let mut msg = format!(
            "INVITE {to} SIP/2.0\r\n\
             {via_line}\
             Max-Forwards: 70\r\n\
             To: <{to}>\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} INVITE\r\n\
             {contact_line}\
             Content-Type: application/sdp\r\n\
             User-Agent: {user_agent}\r\n"
        );
        if self.account.hide_caller_id {
            msg.push_str("Privacy: id\r\n");
        }
        // RFC 4028 Session Timers -- proposed as `refresher=uac` since we're
        // the one placing this INVITE; see `client::SESSION_EXPIRES_DEFAULT`/
        // `SESSION_MIN_SE` and `on_response`'s `Act::Connected`/422 handling.
        if self.account.session_timers_enabled {
            msg.push_str(&format!(
                "Supported: timer\r\nSession-Expires: {session_expires};refresher=uac\r\n\
                 Min-SE: {}\r\n",
                crate::client::SESSION_MIN_SE
            ));
        }
        if let Some(a) = auth {
            msg.push_str(a);
            msg.push_str("\r\n");
        }
        msg.push_str(&format!("Content-Length: {body_len}\r\n\r\n{sdp}"));
        msg
    }
}
