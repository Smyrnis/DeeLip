//! RFC 4028 Session Timers responses for `SipStack::on_response` -- split
//! out of `response.rs` purely for file size (same precedent as
//! `views/settings/`, `views/dialer/`, `sip-core/src/call/lifecycle/`), not
//! a behavior change.

use tracing::debug;

use crate::client::SipStack;
use crate::wire::util::new_branch;

impl SipStack {
    #[allow(clippy::too_many_arguments)] // mirrors the `Act::SessionRefreshAck` variant's field set
    pub(super) async fn handle_session_refresh_ack(
        &mut self, call_id: String, ack_cid: String, ack_from_tag: String, ack_to_uri: String,
        ack_to_tag: Option<String>, ack_cseq: u32, session_expires_hdr: Option<(u32, Option<String>)>,
    ) {
        let ack = self.build_ack(&ack_cid, &ack_from_tag, &ack_to_uri, ack_to_tag.as_deref(), ack_cseq, &new_branch());
        let _ = self.transport.send(ack.as_bytes(), self.server_addr).await;
        // Reschedule from this response's (possibly renegotiated)
        // Session-Expires -- same parsing as the initial INVITE's
        // 200 OK in `handle_connected` above.
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
    }

    #[allow(clippy::too_many_arguments)] // mirrors the `Act::SessionIntervalTooSmall` variant's field set
    pub(super) async fn handle_session_interval_too_small(
        &mut self, call_id: String, to_uri: String, local_sdp: String, min_se: u32, ack_cid: String,
        ack_from_tag: String, ack_to_uri: String, ack_to_tag: Option<String>, ack_cseq: u32,
    ) {
        let Some(invite_branch) = self.dialogs.get(&call_id).map(|d| d.invite_branch.clone()) else {
            return;
        };
        let ack = self.build_ack(&ack_cid, &ack_from_tag, &ack_to_uri, ack_to_tag.as_deref(), ack_cseq, &invite_branch);
        let _ = self.transport.send(ack.as_bytes(), self.server_addr).await;

        let Some(dialog) = self.dialogs.get_mut(&call_id) else {
            return;
        };
        let cseq = dialog.next_local_cseq();
        let branch = new_branch();
        dialog.invite_branch = branch.clone();
        dialog.session_expires_retried = true;
        let dialog_call_id = dialog.call_id.clone();
        let dialog_from_tag = dialog.local_tag.clone();
        let session_expires = min_se.max(crate::client::SESSION_EXPIRES_DEFAULT);
        let msg = self.build_invite(
            &dialog_call_id,
            &dialog_from_tag,
            cseq,
            &to_uri,
            &local_sdp,
            None,
            &branch,
            session_expires,
        );
        debug!("→ INVITE {to_uri} (Session-Expires: {session_expires}, retried after 422)");
        let _ = self.transport.send(msg.as_bytes(), self.server_addr).await;
    }
}
