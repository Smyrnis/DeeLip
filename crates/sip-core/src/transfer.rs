use std::net::SocketAddr;

use tracing::debug;

use crate::{
    client::SipStack,
    dialog::DialogState,
    util::{encode_replaces_param, new_branch},
};

impl SipStack {
    /// Blind-transfer an active call via REFER. `target` must already be a
    /// fully-qualified SIP URI. Fire-and-forget beyond the REFER response
    /// itself (see `SipEvent::TransferAccepted`/`TransferFailed`) — no NOTIFY
    /// sipfrag progress tracking; the far end normally sends BYE on this
    /// dialog once the transferred call succeeds.
    pub(crate) async fn blind_transfer(&mut self, call_id: &str, target: &str) {
        let dialog = match self.dialogs.get_mut(call_id) {
            Some(d) if d.state == DialogState::Confirmed => d,
            _ => return,
        };

        let cseq   = dialog.next_local_cseq();
        let branch = new_branch();

        let server     = self.account.server.clone();
        let username   = self.account.username.clone();
        let display    = self.account.display_name.clone().unwrap_or_else(|| username.clone());
        let adv_ip     = self.advertised_ip.clone();
        let local_ip   = self.local_ip.clone();
        let local_port = self.local_port;
        let call_id_s  = dialog.call_id.clone();
        let from_tag   = dialog.local_tag.clone();
        let to_uri     = dialog.remote_uri.clone();
        let to_tag     = dialog.remote_tag.as_deref()
            .map(|t| format!(";tag={t}")).unwrap_or_default();
        let contact: SocketAddr = dialog.remote_contact
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(self.server_addr);
        let via_proto = self.via_proto();
        let contact_transport = self.contact_transport_param();

        let refer = format!(
            "REFER {to_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id_s}\r\n\
             CSeq: {cseq} REFER\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
             Refer-To: <{target}>\r\n\
             User-Agent: DeeLip/0.1.0\r\n\
             Content-Length: 0\r\n\r\n"
        );
        debug!("→ REFER {to_uri} (Refer-To: {target})");
        let _ = self.transport.send(refer.as_bytes(), contact).await;
    }

    /// Attended transfer: sends REFER on the ORIGINAL call's dialog with a
    /// `Replaces` parameter (RFC 3891) referencing the CONSULTATION call's
    /// dialog identity, so the transferee re-INVITEs the consultation
    /// target directly instead of dialing fresh. Mirrors `blind_transfer`'s
    /// header shape exactly, differing only in the `Refer-To` value.
    pub(crate) async fn attended_transfer(&mut self, call_id: &str, consultation_call_id: &str) {
        let (target, replaces) = {
            let Some(consult) = self.dialogs.get(consultation_call_id) else { return };
            let replaces = format!(
                "{};to-tag={};from-tag={}",
                consult.call_id,
                consult.remote_tag.as_deref().unwrap_or(""),
                consult.local_tag,
            );
            (consult.remote_uri.clone(), replaces)
        };
        let refer_to = format!("{target}?Replaces={}", encode_replaces_param(&replaces));

        let dialog = match self.dialogs.get_mut(call_id) {
            Some(d) if d.state == DialogState::Confirmed => d,
            _ => return,
        };

        let cseq   = dialog.next_local_cseq();
        let branch = new_branch();

        let server     = self.account.server.clone();
        let username   = self.account.username.clone();
        let display    = self.account.display_name.clone().unwrap_or_else(|| username.clone());
        let adv_ip     = self.advertised_ip.clone();
        let local_ip   = self.local_ip.clone();
        let local_port = self.local_port;
        let call_id_s  = dialog.call_id.clone();
        let from_tag   = dialog.local_tag.clone();
        let to_uri     = dialog.remote_uri.clone();
        let to_tag     = dialog.remote_tag.as_deref()
            .map(|t| format!(";tag={t}")).unwrap_or_default();
        let contact: SocketAddr = dialog.remote_contact
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(self.server_addr);
        let via_proto = self.via_proto();
        let contact_transport = self.contact_transport_param();

        let refer = format!(
            "REFER {to_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id_s}\r\n\
             CSeq: {cseq} REFER\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
             Refer-To: <{refer_to}>\r\n\
             User-Agent: DeeLip/0.1.0\r\n\
             Content-Length: 0\r\n\r\n"
        );
        debug!("→ REFER {to_uri} (attended transfer, Replaces: {replaces})");
        let _ = self.transport.send(refer.as_bytes(), contact).await;
    }

    /// Redirect a not-yet-answered incoming call via 302 Moved Temporarily —
    /// `target` must already be a fully-qualified SIP URI. Used for the
    /// no-answer-forward timeout; removes the dialog like `reject_call` does.
    pub(crate) async fn redirect_call(&mut self, call_id: &str, target: &str) {
        if let Some(dialog) = self.dialogs.remove(call_id) {
            let contact: SocketAddr = dialog.remote_contact
                .as_deref()
                .and_then(|s| s.parse().ok())
                .unwrap_or(self.server_addr);
            let branch     = new_branch();
            let local_ip   = &self.local_ip;
            let local_port = self.local_port;
            let username   = &self.account.username;
            let server     = &self.account.server;
            let display    = self.account.display_name.as_deref().unwrap_or(username);
            let local_tag  = &dialog.local_tag;
            let remote_uri = &dialog.remote_uri;
            let from_tag   = dialog.remote_tag.as_deref()
                .map(|t| format!(";tag={t}")).unwrap_or_default();
            let cseq_n = dialog.remote_cseq.unwrap_or(1);
            let via_proto = self.via_proto();

            let redirect = format!(
                "SIP/2.0 302 Moved Temporarily\r\n\
                 Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch}\r\n\
                 To: \"{display}\" <sip:{username}@{server}>;tag={local_tag}\r\n\
                 From: <{remote_uri}>{from_tag}\r\n\
                 Call-ID: {call_id}\r\n\
                 CSeq: {cseq_n} INVITE\r\n\
                 Contact: <{target}>\r\n\
                 Content-Length: 0\r\n\r\n"
            );
            debug!("→ 302 Moved Temporarily {call_id} (Contact: {target})");
            let _ = self.transport.send(redirect.as_bytes(), contact).await;
        }
    }
}
