//! Hold/resume re-INVITEs, RFC 4028 session-timer refresh re-INVITEs, and
//! SIP INFO DTMF relay -- every "send a fresh mid-dialog request" path.

use tracing::debug;

use super::{DialogRequestContext, StackIdentity};
use crate::{
    call::dialog::DialogState,
    client::SipStack,
    wire::sdp::{build_hold_offer, build_resume_offer, build_video_media_section},
    wire::util::new_branch,
};

impl SipStack {
    // ── Hold / Resume (re-INVITE) ─────────────────────────────────────────────

    pub(crate) async fn send_reinvite(&mut self, call_id: &str, hold: bool) {
        let identity = self.stack_identity();
        let advertised_ip = identity.adv_ip.clone();
        let dialog = match self.dialogs.get_mut(call_id) {
            Some(d) if d.state == DialogState::Confirmed => d,
            _ => return,
        };
        let Some(media) = &dialog.media else { return };
        let (rtp_ip, rtp_port) = match &media.relay {
            Some(r) => (r.relayed_addr.ip().to_string(), r.relayed_addr.port()),
            None => (advertised_ip.clone(), media.local_rtp),
        };
        let dtls_fp = media.local_dtls.as_ref().map(|d| &d.local_fingerprint);
        let dtls_setup = media.local_dtls.as_ref().and_then(|d| d.role);
        let mut local_sdp = if hold {
            build_hold_offer(&rtp_ip, rtp_port, media.codec, media.local_srtp.as_ref(), dtls_fp, dtls_setup)
        } else {
            build_resume_offer(&rtp_ip, rtp_port, media.codec, media.local_srtp.as_ref(), dtls_fp, dtls_setup)
        };
        // Hold/resume re-INVITEs are audio-only-shaped above; if this call
        // also negotiated video, append its own `m=video` section (no ICE
        // renegotiation, same convention the audio branch above already
        // follows) -- otherwise a hold/resume would silently drop video
        // from the SDP entirely.
        if let Some(video) = &media.video {
            let (video_ip, video_port) = match &video.relay {
                Some(r) => (r.relayed_addr.ip().to_string(), r.relayed_addr.port()),
                None => (advertised_ip.clone(), video.local_rtp),
            };
            local_sdp.push_str(&build_video_media_section(
                &video_ip,
                video_port,
                video.codec,
                video.local_srtp.as_ref(),
                None,
                dtls_fp,
                dtls_setup,
            ));
        }
        let local_sdp = local_sdp.as_str();

        let cseq = dialog.next_local_cseq();
        let branch = new_branch();

        let ctx = DialogRequestContext::new(&identity, dialog);
        let server = &ctx.server;
        let username = &ctx.username;
        let display = &ctx.display;
        let adv_ip = &ctx.adv_ip;
        let local_ip = &ctx.local_ip;
        let local_port = ctx.local_port;
        let call_id_s = &ctx.call_id;
        let from_tag = &ctx.local_tag;
        let to_uri = &ctx.remote_uri;
        let to_tag = &ctx.remote_tag_param;
        let contact = ctx.contact;
        let body_len = local_sdp.len();

        dialog.hold_pending = Some(hold);
        dialog.local_sdp = Some(local_sdp.to_string());
        let via_proto = ctx.via_proto;
        let contact_transport = ctx.contact_transport;
        let via_line = crate::client::build_via(via_proto, local_ip, local_port, &branch);
        let contact_line = crate::client::build_contact(username, adv_ip, local_port, contact_transport);
        let user_agent = crate::USER_AGENT;

        let reinvite = format!(
            "INVITE {to_uri} SIP/2.0\r\n\
             {via_line}\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id_s}\r\n\
             CSeq: {cseq} INVITE\r\n\
             {contact_line}\
             Content-Type: application/sdp\r\n\
             User-Agent: {user_agent}\r\n\
             Content-Length: {body_len}\r\n\r\n\
             {local_sdp}"
        );
        debug!("→ re-INVITE ({})", if hold { "hold" } else { "resume" });
        let _ = self.transport.send(reinvite.as_bytes(), contact).await;
    }

    // ── Session Timers (RFC 4028) ─────────────────────────────────────────────

    /// Send a no-op refresh re-INVITE for `call_id`: same SDP as currently
    /// negotiated (no media change, unlike `send_reinvite`'s hold/resume
    /// offers), just a fresh `Session-Expires` to keep the dialog from being
    /// treated as stale. Called from `client::run`'s periodic scan once
    /// `dialog.session_refresh_at` is due -- a no-op if the dialog vanished,
    /// isn't Confirmed, or isn't ours to refresh in the first place.
    pub(crate) async fn send_session_refresh(&mut self, call_id: &str) {
        let identity: StackIdentity = self.stack_identity();
        let dialog = match self.dialogs.get_mut(call_id) {
            Some(d) if d.state == DialogState::Confirmed && d.we_are_refresher => d,
            _ => return,
        };
        let Some(interval) = dialog.session_expires else {
            return;
        };
        let Some(local_sdp) = dialog.local_sdp.clone() else {
            return;
        };
        // The `refresher=` param always refers to the *original* INVITE's
        // UAC/UAS roles, not whoever happens to send this particular
        // re-INVITE -- see `Dialog::original_role_is_uac`'s doc comment.
        let refresher = if dialog.original_role_is_uac { "uac" } else { "uas" };

        let cseq = dialog.next_local_cseq();
        let branch = new_branch();

        let ctx = DialogRequestContext::new(&identity, dialog);
        let server = &ctx.server;
        let username = &ctx.username;
        let display = &ctx.display;
        let adv_ip = &ctx.adv_ip;
        let local_ip = &ctx.local_ip;
        let local_port = ctx.local_port;
        let call_id_s = &ctx.call_id;
        let from_tag = &ctx.local_tag;
        let to_uri = &ctx.remote_uri;
        let to_tag = &ctx.remote_tag_param;
        let contact = ctx.contact;
        let body_len = local_sdp.len();

        dialog.session_refresh_pending = true;
        let via_proto = ctx.via_proto;
        let contact_transport = ctx.contact_transport;
        let via_line = crate::client::build_via(via_proto, local_ip, local_port, &branch);
        let contact_line = crate::client::build_contact(username, adv_ip, local_port, contact_transport);
        let user_agent = crate::USER_AGENT;

        let refresh = format!(
            "INVITE {to_uri} SIP/2.0\r\n\
             {via_line}\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id_s}\r\n\
             CSeq: {cseq} INVITE\r\n\
             {contact_line}\
             Content-Type: application/sdp\r\n\
             Supported: timer\r\n\
             Session-Expires: {interval};refresher={refresher}\r\n\
             User-Agent: {user_agent}\r\n\
             Content-Length: {body_len}\r\n\r\n\
             {local_sdp}"
        );
        debug!(call_id, "→ session-refresh re-INVITE");
        let _ = self.transport.send(refresh.as_bytes(), contact).await;
    }

    /// Refresh any Confirmed dialog whose `session_refresh_at` has passed
    /// and where we're the refresher -- called from the same 30s tick as
    /// `refresh_presence_subscriptions`/`refresh_mwi_subscriptions`/
    /// `refresh_presence_publish`.
    pub(crate) async fn refresh_session_timers(&mut self) {
        let now = tokio::time::Instant::now();
        let due: Vec<String> = self
            .dialogs
            .iter()
            .filter(|(_, d)| {
                d.state == DialogState::Confirmed
                    && d.we_are_refresher
                    && d.session_refresh_at.is_some_and(|at| at <= now)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for call_id in due {
            self.send_session_refresh(&call_id).await;
        }
    }

    /// Send one DTMF digit via SIP INFO (`application/dtmf-relay`, the
    /// long-standing de facto format most PBXes/gateways that support this
    /// scheme at all expect) instead of an RFC 2833 RTP telephone-event
    /// burst. Mirrors `transfer::blind_transfer`'s header shape exactly.
    pub(crate) async fn send_dtmf_info(&mut self, call_id: &str, digit: char) {
        let identity = self.stack_identity();
        let dialog = match self.dialogs.get_mut(call_id) {
            Some(d) if d.state == DialogState::Confirmed => d,
            _ => return,
        };

        let cseq = dialog.next_local_cseq();
        let branch = new_branch();

        let ctx = DialogRequestContext::new(&identity, dialog);
        let server = &ctx.server;
        let username = &ctx.username;
        let display = &ctx.display;
        let adv_ip = &ctx.adv_ip;
        let local_ip = &ctx.local_ip;
        let local_port = ctx.local_port;
        let call_id_s = &ctx.call_id;
        let from_tag = &ctx.local_tag;
        let to_uri = &ctx.remote_uri;
        let to_tag = &ctx.remote_tag_param;
        let contact = ctx.contact;
        let via_proto = ctx.via_proto;
        let contact_transport = ctx.contact_transport;
        let via_line = crate::client::build_via(via_proto, local_ip, local_port, &branch);
        let contact_line = crate::client::build_contact(username, adv_ip, local_port, contact_transport);

        let user_agent = crate::USER_AGENT;
        let body = format!("Signal={digit}\r\nDuration=250\r\n");
        let info = format!(
            "INFO {to_uri} SIP/2.0\r\n\
             {via_line}\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id_s}\r\n\
             CSeq: {cseq} INFO\r\n\
             {contact_line}\
             Content-Type: application/dtmf-relay\r\n\
             User-Agent: {user_agent}\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        debug!("→ INFO {to_uri} (DTMF digit={digit})");
        let _ = self.transport.send(info.as_bytes(), contact).await;
    }
}
