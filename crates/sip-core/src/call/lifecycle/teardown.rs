//! Call teardown: outgoing BYE (`hang_up`), and the incoming BYE/ACK/CANCEL
//! handlers.

use std::net::SocketAddr;

use tracing::debug;

use super::DialogRequestContext;
use crate::{call::dialog::DialogState, client::SipStack, events::SipEvent, wire::message::SipMessage};

impl SipStack {
    pub(crate) async fn hang_up(&mut self, call_id: &str) {
        let identity = self.stack_identity();
        let dialog = match self.dialogs.get_mut(call_id) {
            Some(d) => d,
            None => return,
        };

        dialog.state = DialogState::Terminating;
        let cseq = dialog.next_local_cseq();
        let branch = crate::wire::util::new_branch();

        let ctx = DialogRequestContext::new(&identity, dialog);
        let server = &ctx.server;
        let username = &ctx.username;
        let display = &ctx.display;
        let local_ip = &ctx.local_ip;
        let adv_ip = &ctx.adv_ip;
        let local_port = ctx.local_port;
        let call_id_s = &ctx.call_id;
        let from_tag = &ctx.local_tag;
        let to_uri = &ctx.remote_uri;
        let to_tag = &ctx.remote_tag_param;
        let contact = ctx.contact;
        let via_proto = ctx.via_proto;
        let contact_transport = ctx.contact_transport;

        let bye = format!(
            "BYE {to_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch};rport\r\n\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id_s}\r\n\
             CSeq: {cseq} BYE\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
             User-Agent: DeeLip/0.1.0\r\n\
             Content-Length: 0\r\n\r\n"
        );
        let _ = self.transport.send(bye.as_bytes(), contact).await;
    }

    // ── Incoming BYE / ACK / CANCEL ──────────────────────────────────────────

    pub(crate) async fn on_bye(&mut self, msg: SipMessage, from: SocketAddr) {
        let call_id = match msg.call_id() {
            Some(id) => id.to_string(),
            None => return,
        };
        debug!("← BYE {call_id}");
        let ok = self.build_response(&msg, 200, "OK", "", "");
        let _ = self.transport.send(ok.as_bytes(), from).await;
        if let Some(mut dialog) = self.dialogs.remove(&call_id) {
            dialog.state = DialogState::Terminated;
        }
        let _ = self.event_tx.send(SipEvent::CallEnded { call_id });
    }

    pub(crate) fn on_ack(&mut self, msg: SipMessage) {
        if let Some(id) = msg.call_id().map(str::to_string) {
            if let Some(d) = self.dialogs.get_mut(&id) {
                if d.state == DialogState::Calling {
                    d.state = DialogState::Confirmed;
                }
            }
        }
    }

    pub(crate) async fn on_cancel(&mut self, msg: SipMessage, from: SocketAddr) {
        let ok = self.build_response(&msg, 200, "OK", "", "");
        let _ = self.transport.send(ok.as_bytes(), from).await;
        if let Some(call_id) = msg.call_id() {
            let call_id = call_id.to_string();
            debug!("← CANCEL {call_id}");
            self.dialogs.remove(&call_id);
            let _ = self.event_tx.send(SipEvent::CallEnded { call_id });
        }
    }
}
