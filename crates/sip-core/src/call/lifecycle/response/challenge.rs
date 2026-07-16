//! 401/407 auth-retry handling for `SipStack::on_response` -- split out of
//! `response.rs` purely for file size (same precedent as `views/settings/`,
//! `views/dialer/`, `sip-core/src/call/lifecycle/`), not a behavior change.

use tracing::debug;

use crate::client::SipStack;
use crate::events::SipEvent;
use crate::wire::auth::build_challenge_response;
use crate::wire::util::new_branch;

impl SipStack {
    #[allow(clippy::too_many_arguments)] // mirrors the `Act::InviteChallenged` variant's field set
    pub(super) async fn handle_invite_challenged(
        &mut self, call_id: String, to_uri: String, local_sdp: String, challenge_raw: String, ack_cid: String,
        ack_from_tag: String, ack_to_uri: String, ack_to_tag: Option<String>, ack_cseq: u32,
    ) {
        // ACK to a non-2xx response must reuse the *original*
        // INVITE's branch (RFC 3261 §17.1.1.3), unlike a 2xx ACK
        // which is a new transaction with its own fresh branch.
        let Some(invite_branch) = self.dialogs.get(&call_id).map(|d| d.invite_branch.clone()) else {
            return;
        };
        let ack = self.build_ack(&ack_cid, &ack_from_tag, &ack_to_uri, ack_to_tag.as_deref(), ack_cseq, &invite_branch);
        let _ = self.transport.send(ack.as_bytes(), self.server_addr).await;

        let Some(auth) = build_challenge_response(
            self.account.auth_username(),
            &self.account.password,
            "INVITE",
            &to_uri,
            &challenge_raw,
        ) else {
            self.dialogs.remove(&call_id);
            let _ =
                self.event_tx.send(SipEvent::CallFailed { call_id, code: 401, reason: "Bad auth challenge".into() });
            return;
        };

        let Some(dialog) = self.dialogs.get_mut(&call_id) else {
            return;
        };
        let cseq = dialog.next_local_cseq();
        let branch = new_branch();
        dialog.invite_branch = branch.clone();
        let dialog_call_id = dialog.call_id.clone();
        let dialog_from_tag = dialog.local_tag.clone();
        let msg = self.build_invite(
            &dialog_call_id,
            &dialog_from_tag,
            cseq,
            &to_uri,
            &local_sdp,
            Some(&auth),
            &branch,
            crate::client::SESSION_EXPIRES_DEFAULT,
        );
        debug!("→ INVITE {to_uri} (authenticated)");
        let _ = self.transport.send(msg.as_bytes(), self.server_addr).await;
    }
}
