//! `SipStack`'s main event loop (`run()`), its message/command dispatchers,
//! and the background-call-setup event handler.

use tokio::time::{Duration, Instant, interval, sleep_until};
use tracing::{debug, error};

use super::events::StackEvent;
use super::{CmdRx, MAX_RETRY, PRESENCE_TICK, REG_MARGIN, SipStack};
use crate::events::{SipCommand, SipEvent};
use crate::wire::message::{SipMessage, SipMethod, SipStartLine};

impl SipStack {
    pub async fn run(mut self) -> Result<(), (anyhow::Error, CmdRx)> {
        // `SipAccount::local_account` never registers (see the `sleep_until`
        // branch's `if !self.account.local_account` guard below) -- without
        // this, `ui` would wait forever for a `SipEvent::Registered` that's
        // never coming, leaving `reg_ok` permanently false (blocking
        // History/Contacts quick-dial and Redial, both gated on it).
        if self.account.local_account {
            let _ = self.event_tx.send(SipEvent::Registered { expires: 0 });
        }
        let mut reregister_at = Instant::now();
        let mut retry_delay = Duration::from_secs(5);
        // Set once a REGISTER is rejected for a reason retrying can never
        // fix (see `PermanentRegError`) -- stops the `sleep_until` branch
        // below from ever re-arming for this account again, instead of
        // backing off forever on an error that will never succeed.
        // Everything else this loop does (calls, presence, keepalive)
        // keeps working normally; only re-registration itself stops.
        let mut permanently_failed = false;
        let mut presence_tick = interval(PRESENCE_TICK);
        // NAT/firewall keepalive -- only ticks when the account has one
        // configured; `if keepalive_tick.is_some()` below guards the whole
        // branch, so an unset value just never sends anything (today's
        // behavior, unchanged).
        let mut keepalive_tick =
            self.account.keepalive_secs.filter(|&s| s > 0).map(|s| interval(Duration::from_secs(s as u64)));

        loop {
            tokio::select! {
                _ = presence_tick.tick() => {
                    self.refresh_presence_subscriptions().await;
                    self.refresh_mwi_subscriptions().await;
                    self.refresh_presence_publish().await;
                    self.refresh_session_timers().await;
                }
                _ = async { keepalive_tick.as_mut().unwrap().tick().await }, if keepalive_tick.is_some() => {
                    self.send_keepalive().await;
                }
                // `SipAccount::local_account` never registers -- guarding
                // the branch off entirely (rather than just skipping the
                // send inside it) means `reregister_at`'s initial
                // `Instant::now()` never fires and no retry/backoff
                // machinery for it ever engages either.
                _ = sleep_until(reregister_at), if !self.account.local_account && !permanently_failed => {
                    match self.register_once().await {
                        Ok(expires) => {
                            retry_delay   = Duration::from_secs(5);
                            reregister_at = Instant::now()
                                + Duration::from_secs(expires.saturating_sub(REG_MARGIN) as u64);
                            let _ = self.event_tx.send(SipEvent::Registered { expires });
                            // Initial publish only -- once `presence_publish`
                            // exists, its own refresh timer (ticked above)
                            // keeps it alive independently of registration.
                            if self.account.publish_presence && self.presence_publish.is_none() {
                                self.publish_own_presence(!self.account.dnd).await;
                            }
                        }
                        Err(e) => {
                            let permanent = e.downcast_ref::<crate::registration::PermanentRegError>().is_some();
                            error!("Registration failed: {e}");
                            let _ = self.event_tx.send(SipEvent::RegistrationFailed {
                                reason: e.to_string(),
                                permanent,
                            });
                            if permanent {
                                permanently_failed = true;
                            } else {
                                reregister_at = Instant::now() + retry_delay;
                                retry_delay   = (retry_delay * 2).min(MAX_RETRY);
                            }
                        }
                    }
                }
                result = self.transport.recv() => {
                    match result {
                        Ok((data, from)) => {
                            if let Some(msg) = SipMessage::parse(&data) {
                                self.dispatch(msg, from).await;
                            }
                        }
                        Err(e) => {
                            error!("Transport error: {e:#}");
                            // Any call whose dialog was live at the moment the
                            // transport died is otherwise left to hang from the
                            // UI's perspective indefinitely (or until Asterisk's
                            // own retransmit timers eventually give up and send
                            // a BYE/CANCEL we happen to still be around to
                            // receive, which can take 20+ seconds) -- the
                            // in-memory dialog itself is gone the moment
                            // `spawn`'s reconnect loop rebuilds a fresh
                            // `SipStack`, so there's no way to recover it either
                            // way. Fail them immediately instead so the UI
                            // reflects reality right away.
                            for call_id in self.dialogs.keys().cloned().collect::<Vec<_>>() {
                                let _ = self.event_tx.send(SipEvent::CallFailed {
                                    call_id, code: 0, reason: "Connection lost".into(),
                                });
                            }
                            return Err((e, self.cmd_rx));
                        }
                    }
                }
                Some(cmd) = self.cmd_rx.recv() => {
                    self.handle_command(cmd).await;
                }
                Some(ev) = self.internal_rx.recv() => {
                    self.handle_stack_event(ev).await;
                }
            }
        }
    }

    /// Send a lone CRLF-CRLF datagram to the registrar to hold a NAT/
    /// firewall's outbound UDP binding (or TCP/TLS connection) open between
    /// registrations -- RFC 2617-style auth and dialog state don't apply to
    /// this, it's purely traffic to keep the path alive, so failures are
    /// logged and otherwise ignored (the next tick tries again regardless).
    async fn send_keepalive(&self) {
        if let Err(e) = self.transport.send(b"\r\n\r\n", self.server_addr).await {
            debug!("Keepalive send failed: {e:#}");
        }
    }

    /// Dispatch a completed background call-setup result -- see
    /// `StackEvent`'s doc comment. Handlers live in `call::lifecycle`
    /// alongside the rest of the call-establishment logic they finish.
    async fn handle_stack_event(&mut self, ev: StackEvent) {
        match ev {
            ev @ StackEvent::OutgoingOfferReady { .. } => self.on_outgoing_offer_ready(ev).await,
            ev @ StackEvent::IncomingAnswerReady { .. } => self.on_incoming_answer_ready(ev).await,
            ev @ StackEvent::OutgoingConnected { .. } => self.on_outgoing_connected(ev).await,
        }
    }

    // ── Message dispatcher ────────────────────────────────────────────────────

    async fn dispatch(&mut self, msg: SipMessage, from: std::net::SocketAddr) {
        match msg.start_line.clone() {
            SipStartLine::Request { method, .. } => {
                match method {
                    SipMethod::Invite => self.on_invite(msg, from).await,
                    SipMethod::Bye => self.on_bye(msg, from).await,
                    SipMethod::Ack => self.on_ack(msg),
                    SipMethod::Cancel => self.on_cancel(msg, from).await,
                    SipMethod::Notify => self.on_notify(msg, from).await,
                    SipMethod::Options => self.send_ok(&msg, from).await,
                    // A peer's own INFO (e.g. Asterisk echoing DTMF back once
                    // `dtmf_mode=info` is set) doesn't carry anything DeeLip
                    // needs to act on today, but RFC 6086 still expects a
                    // response -- leaving it unanswered just makes the sender
                    // retransmit it several times before giving up, which is
                    // exactly what was observed live (three "unhandled
                    // request" log lines for what was really 1-2 messages).
                    SipMethod::Info => self.send_ok(&msg, from).await,
                    SipMethod::Message => self.on_message(msg, from).await,
                    _ => debug!(?method, "Ignoring unhandled request"),
                }
            }
            SipStartLine::Response { status, .. } => {
                self.on_response(msg, status, from).await;
            }
        }
    }

    // ── Command handler ───────────────────────────────────────────────────────

    async fn handle_command(&mut self, cmd: SipCommand) {
        match cmd {
            SipCommand::MakeCall { to, attempt_ice } => self.initiate_call(&to, attempt_ice).await,
            SipCommand::AcceptCall { call_id } => self.accept_call(&call_id).await,
            SipCommand::RejectCall { call_id } => self.reject_call(&call_id).await,
            SipCommand::HangUp { call_id } => self.hang_up(&call_id).await,
            SipCommand::HoldCall { call_id } => self.send_reinvite(&call_id, true).await,
            SipCommand::ResumeCall { call_id } => self.send_reinvite(&call_id, false).await,
            SipCommand::BlindTransfer { call_id, target } => self.blind_transfer(&call_id, &target).await,
            SipCommand::RedirectCall { call_id, target } => self.redirect_call(&call_id, &target).await,
            SipCommand::SubscribePresence { target_uri } => self.subscribe_presence(&target_uri).await,
            SipCommand::UnsubscribePresence { target_uri } => self.unsubscribe_presence(&target_uri).await,
            SipCommand::AttendedTransfer { call_id, consultation_call_id } => {
                self.attended_transfer(&call_id, &consultation_call_id).await
            }
            SipCommand::SendDtmfInfo { call_id, digit } => self.send_dtmf_info(&call_id, digit).await,
            SipCommand::SubscribeMwi { target_uri } => self.subscribe_mwi(&target_uri).await,
            SipCommand::SendMessage { to, body } => self.send_message(&to, &body).await,
            SipCommand::PublishPresence { available } => {
                if self.account.publish_presence {
                    self.publish_own_presence(available).await;
                }
            }
        }
    }
}
