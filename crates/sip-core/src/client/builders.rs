//! Wire-format builders shared across `SipStack`'s response/request paths:
//! transport-flavored Via/Contact strings, and the actual response/ACK
//! string builders.

use std::net::SocketAddr;

use deelip_config::TransportProtocol;

use super::SipStack;
use crate::wire::message::SipMessage;

impl SipStack {
    pub(crate) fn via_proto(&self) -> &'static str {
        via_proto_str(self.resolved_transport)
    }

    /// `;transport=...` URI parameter for our own `Contact:` header — empty
    /// for UDP (the default the far end assumes with no parameter at all),
    /// explicit otherwise so a peer sending a fresh request back to us
    /// (e.g. an Asterisk-originated INVITE) knows to reuse/re-establish
    /// TCP/TLS rather than defaulting to UDP on our registered port, which
    /// silently goes nowhere since we never bind a UDP listener there.
    pub(crate) fn contact_transport_param(&self) -> &'static str {
        contact_transport_param_str(self.resolved_transport)
    }

    // ── Shared response helpers ────────────────────────────────────────────────

    pub(crate) async fn send_ok(&self, req: &SipMessage, from: SocketAddr) {
        let ok = self.build_response(req, 200, "OK", "", "");
        let _ = self.transport.send(ok.as_bytes(), from).await;
    }

    pub(crate) fn build_response(&self, req: &SipMessage, code: u16, phrase: &str, to_tag: &str, body: &str) -> String {
        self.build_response_with_body(req, code, phrase, to_tag, body)
    }

    pub(crate) fn build_response_with_body(
        &self, req: &SipMessage, code: u16, phrase: &str, to_tag: &str, body: &str,
    ) -> String {
        let via = req.header("Via").unwrap_or("");
        let from = req.header("From").unwrap_or("");
        let to = req.header("To").unwrap_or("");
        let call_id = req.header("Call-ID").unwrap_or("");
        let cseq = req.header("CSeq").unwrap_or("");
        let body_len = body.len();

        let to_line =
            if !to_tag.is_empty() && !to.contains(";tag=") { format!("{to};tag={to_tag}") } else { to.to_string() };

        let ct_header = if !body.is_empty() { "Content-Type: application/sdp\r\n" } else { "" };
        let user_agent = crate::USER_AGENT;

        let mut resp = format!(
            "SIP/2.0 {code} {phrase}\r\n\
             Via: {via}\r\n\
             To: {to_line}\r\n\
             From: {from}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq}\r\n\
             User-Agent: {user_agent}\r\n\
             {ct_header}\
             Content-Length: {body_len}\r\n\r\n"
        );
        if !body.is_empty() {
            resp.push_str(body);
        }
        resp
    }

    pub(crate) fn build_ack(
        &self, call_id: &str, from_tag: &str, to_uri: &str, to_tag: Option<&str>, cseq: u32, branch: &str,
    ) -> String {
        let server = &self.identity_host;
        let username = &self.account.username;
        let adv_ip = &self.advertised_ip;
        let local_ip = &self.local_ip;
        let local_port = self.local_port;
        let to_tag_part = to_tag.map(|t| format!(";tag={t}")).unwrap_or_default();
        let display = self.account.display_name.as_deref().unwrap_or(username);
        let via_proto = self.via_proto();
        let contact_transport = self.contact_transport_param();

        format!(
            "ACK {to_uri} SIP/2.0\r\n\
             Via: SIP/2.0/{via_proto} {local_ip}:{local_port};branch={branch}\r\n\
             Max-Forwards: 70\r\n\
             To: <{to_uri}>{to_tag_part}\r\n\
             From: \"{display}\" <sip:{username}@{server}>;tag={from_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: {cseq} ACK\r\n\
             Contact: <sip:{username}@{adv_ip}:{local_port}{contact_transport}>\r\n\
             Content-Length: 0\r\n\r\n"
        )
    }
}

pub(super) fn via_proto_str(proto: TransportProtocol) -> &'static str {
    match proto {
        TransportProtocol::Udp => "UDP",
        TransportProtocol::Tcp => "TCP",
        TransportProtocol::Tls => "TLS",
        TransportProtocol::Auto => unreachable!("resolved_transport is never Auto"),
    }
}

pub(super) fn contact_transport_param_str(proto: TransportProtocol) -> &'static str {
    match proto {
        TransportProtocol::Udp => "",
        TransportProtocol::Tcp => ";transport=tcp",
        TransportProtocol::Tls => ";transport=tls",
        TransportProtocol::Auto => unreachable!("resolved_transport is never Auto"),
    }
}
