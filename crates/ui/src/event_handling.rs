use deelip_config::{CallRecord, CallStatus, Contact, Direction, Message};
use deelip_sip::{PresenceState, SipEvent};

use crate::app::{AccountSpawnMsg, AccountState, CallSlot, DeelipApp, PendingCall};
use crate::helpers::{extract_user_part, normalize_target, short_uri, unix_now};
use crate::strings::{t, tf};

impl DeelipApp {
    // ── Background account-spawn processing ──────────────────────────────────

    /// Drains the background account-spawn channel (see `main()`'s pre-window
    /// `rt.spawn` task and `AccountSpawnMsg`'s doc comment), called once per
    /// frame alongside `process_sip_events`. Newly-spawned accounts are
    /// appended to `self.accounts` in whatever order they finish
    /// connecting/timing-out in, not `config.accounts`' original order --
    /// every existing `self.accounts[i]` access already derives `i` from
    /// `self.accounts`' own current position (picker selection, tray
    /// lookups, etc.), never from config order, so this is safe.
    pub(crate) fn process_account_spawn_events(&mut self) {
        let Some(rx) = &self.account_spawn_rx else { return };
        let messages: Vec<AccountSpawnMsg> = rx.try_iter().collect();
        for msg in messages {
            match msg {
                AccountSpawnMsg::Spawned(account, handle) => {
                    self.accounts.push(AccountState {
                        label: crate::helpers::account_label(&account),
                        account: *account,
                        handle,
                        reg_ok: false,
                        status: t("status.registering"),
                        mwi: None,
                    });
                    self.refresh_idle_status();
                }
                AccountSpawnMsg::Done => {
                    self.account_spawn_rx = None;
                    self.refresh_idle_status();
                }
            }
        }
    }

    // ── SIP event processing ─────────────────────────────────────────────────

    /// Drain every account's event queue first, tagging each event with the
    /// account index it came from, then process them — a single loop can't
    /// borrow `self.accounts[i].handle.event_rx` mutably while also calling
    /// `&mut self` methods to react to what it received.
    pub(crate) fn process_sip_events(&mut self) {
        let mut events: Vec<(usize, SipEvent)> = Vec::new();
        for (i, acc) in self.accounts.iter_mut().enumerate() {
            while let Ok(event) = acc.handle.event_rx.try_recv() {
                events.push((i, event));
            }
        }
        for (account, event) in events {
            self.handle_sip_event(account, event);
        }
    }

    pub(crate) fn handle_sip_event(&mut self, account: usize, event: SipEvent) {
        match event {
            SipEvent::Registered { expires } => {
                self.accounts[account].reg_ok = true;
                self.accounts[account].status = if self.accounts[account].account.local_account {
                    t("status.ready_local")
                } else {
                    tf("status.registered", &[("expires", &expires.to_string())])
                };
                self.refresh_idle_status();
                self.subscribe_account_contacts(account);
                self.subscribe_account_mwi(account);
            }
            SipEvent::RegistrationFailed { reason, permanent } => {
                self.accounts[account].reg_ok = false;
                self.accounts[account].status = if permanent {
                    tf("status.registration_failed_permanent", &[("reason", &reason)])
                } else {
                    tf("status.registration_failed", &[("reason", &reason)])
                };
                self.refresh_idle_status();
            }
            SipEvent::CallRinging { .. } => {
                self.status_line = t("status.ringing");
            }
            SipEvent::CallConnected { call_id, media } => {
                // `pending_accept` (an inbound call we told `SipStack` to
                // accept) is checked first since it carries `call_id` to
                // match on; `pending_outbound` doesn't (there's at most one
                // in flight, and its `call_id` isn't known until the far end
                // answers) so it's the fallback once accept is ruled out.
                if self.pending_accept.as_ref().is_some_and(|p| p.call_id == call_id) {
                    let pending = self.pending_accept.take().unwrap();
                    // Free the audio device for the new call if another one
                    // is focused -- deferred until here (accept actually
                    // succeeded) rather than done eagerly in `do_accept`, so
                    // a decline never needlessly disturbs an already-active
                    // call's media.
                    if let Some(cur) = self.focused_call {
                        self.send_hold(cur);
                        self.stop_focused_media();
                        self.focused_call = None;
                    }
                    let slot = CallSlot {
                        account,
                        call_id,
                        remote_uri: pending.remote_uri.clone(),
                        direction: Direction::Inbound,
                        start_time: pending.start_time,
                        is_held: false,
                        recording_enabled: self.config.recording_enabled,
                        media,
                    };
                    self.calls.push(slot);
                    let idx = self.calls.len() - 1;
                    self.status_line = tf("status.in_call", &[("uri", &short_uri(&pending.remote_uri))]);
                    self.start_media(idx);
                } else if let Some(out) = self.pending_outbound.take() {
                    let slot = CallSlot {
                        account,
                        call_id,
                        remote_uri: out.remote_uri.clone(),
                        direction: Direction::Outbound,
                        start_time: out.start_time,
                        is_held: false,
                        recording_enabled: self.config.recording_enabled,
                        media,
                    };
                    self.calls.push(slot);
                    let idx = self.calls.len() - 1;
                    self.status_line = tf("status.in_call", &[("uri", &short_uri(&out.remote_uri))]);
                    self.start_media(idx);
                } else {
                    tracing::warn!(call_id, "CallConnected with no pending call — ignoring");
                }
            }
            SipEvent::IncomingCall { call_id, from, remote_answer_after } => {
                let caller = extract_user_part(&from);
                if self.config.blocklist.iter().any(|entry| extract_user_part(entry) == caller) {
                    self.accounts[account].handle.reject_call(&call_id);
                    self.record_history(from, Direction::Inbound, unix_now(), CallStatus::Rejected);
                    return;
                }
                let acc = &self.accounts[account].account;
                // "Deny/Auto Answer (Control Button)": both react to the
                // same remote answer-after signal (see
                // `deelip_sip::wire::util::parse_call_info_answer_after`).
                // Deny wins if both are on; both bypass DND/forwarding below
                // (the intercom/paging use case this exists for) -- only the
                // blocklist above and the capacity check below still apply.
                if remote_answer_after.is_some() && acc.deny_incoming_control_button {
                    tracing::debug!(call_id, %from, "Remote auto-answer signal + Deny Incoming (Control Button) active, rejecting");
                    self.accounts[account].handle.reject_call(&call_id);
                    self.record_history(from, Direction::Inbound, unix_now(), CallStatus::Rejected);
                    return;
                }
                let remote_auto_answer = remote_answer_after.is_some() && acc.auto_answer_control_button;
                let dnd = acc.dnd && !remote_auto_answer;
                let forward_always =
                    acc.forward_always.clone().filter(|s| !s.is_empty()).filter(|_| !remote_auto_answer);
                let forward_on_busy =
                    acc.forward_on_busy.clone().filter(|s| !s.is_empty()).filter(|_| !remote_auto_answer);
                let server = acc.server.clone();
                if dnd {
                    tracing::debug!(call_id, %from, "DND active, rejecting incoming call");
                    self.accounts[account].handle.reject_call(&call_id);
                    self.record_history(from, Direction::Inbound, unix_now(), CallStatus::Rejected);
                    return;
                }
                if let Some(target) = forward_always {
                    let target = normalize_target(&target, &server);
                    self.accounts[account].handle.redirect_call(&call_id, target);
                    self.record_history(from, Direction::Inbound, unix_now(), CallStatus::Missed);
                    return;
                }
                let waiting = !self.calls.is_empty();
                if waiting {
                    if let Some(target) = forward_on_busy {
                        let target = normalize_target(&target, &server);
                        self.accounts[account].handle.redirect_call(&call_id, target);
                        self.record_history(from, Direction::Inbound, unix_now(), CallStatus::Missed);
                        return;
                    }
                    // Single Call Mode: reject outright as a plain busy
                    // signal instead of ringing as a call-waiting second
                    // call -- only reached when this account has no
                    // `forward_on_busy` of its own, which takes priority.
                    if self.config.single_call_mode && !remote_auto_answer {
                        self.accounts[account].handle.reject_call(&call_id);
                        self.record_history(from, Direction::Inbound, unix_now(), CallStatus::Rejected);
                        return;
                    }
                }
                // `pending_accept` counts toward capacity too -- it's about
                // to occupy a `calls` slot once `CallConnected` arrives, and
                // being a single slot (not a list), a 2nd accept before then
                // would silently overwrite its tracking, orphaning the first
                // call once its own event lands.
                let occupied = self.calls.len() + usize::from(self.pending_accept.is_some());
                if occupied >= 2 || self.pending_call.is_some() {
                    // Already at capacity (2 concurrent + at most 1 ringing) — decline immediately.
                    self.accounts[account].handle.reject_call(&call_id);
                    return;
                }
                self.status_line = if waiting {
                    tf("status.call_waiting", &[("uri", &short_uri(&from))])
                } else {
                    tf("status.incoming_from", &[("uri", &short_uri(&from))])
                };
                let acc = &self.accounts[account].account;
                let forward = acc.no_answer_forward.clone().filter(|s| !s.is_empty()).map(|target| {
                    let target = normalize_target(&target, &acc.server);
                    (unix_now() + acc.no_answer_timeout_secs as u64, target)
                });
                let auto_answer_at = if remote_auto_answer {
                    Some(unix_now() + remote_answer_after.unwrap_or(0) as u64)
                } else {
                    acc.auto_answer_enabled.then(|| unix_now() + acc.auto_answer_secs as u64)
                };
                self.pending_call =
                    Some(PendingCall { account, call_id, from, start_time: unix_now(), forward, auto_answer_at });
            }
            SipEvent::CallEnded { call_id } => {
                self.on_call_terminated(&call_id, None);
                tracing::debug!(call_id, "Call ended normally");
            }
            SipEvent::CallFailed { call_id, code, reason } => {
                self.on_call_terminated(&call_id, Some((code, reason)));
            }
            SipEvent::CallHeld { call_id } => {
                if let Some(slot) = self.calls.iter_mut().find(|c| c.call_id == call_id) {
                    slot.is_held = true;
                }
                self.refresh_call_status();
            }
            SipEvent::CallResumed { call_id } => {
                if let Some(slot) = self.calls.iter_mut().find(|c| c.call_id == call_id) {
                    slot.is_held = false;
                }
                self.refresh_call_status();
            }
            SipEvent::RemoteHeld { .. } => {
                self.status_line = t("status.remote_held");
            }
            SipEvent::RemoteResumed { .. } => {
                self.status_line = t("status.remote_resumed");
            }
            SipEvent::TransferAccepted { call_id } => {
                tracing::info!(call_id, "Transfer accepted");
                self.status_line = t("status.transfer_accepted");
                // Attended transfer only (blind transfer never sets this):
                // both legs are done once the transferee re-INVITEs the
                // target directly via Replaces, so hang up both ourselves
                // rather than blind transfer's passive wait-for-BYE.
                if let Some(original_idx) = self.attended_transfer_original.take()
                    && self.calls.len() == 2
                {
                    let consult_idx = 1 - original_idx;
                    let (first, second) = if original_idx > consult_idx {
                        (original_idx, consult_idx)
                    } else {
                        (consult_idx, original_idx)
                    };
                    self.do_hangup(first);
                    self.do_hangup(second);
                }
            }
            SipEvent::TransferFailed { call_id, reason } => {
                tracing::warn!(call_id, reason, "Transfer failed");
                self.status_line = tf("status.transfer_failed", &[("reason", &reason)]);
                self.attended_transfer_original = None;
            }
            SipEvent::PresenceSubscribed { uri, expires } => {
                tracing::debug!(uri, expires, "Presence subscribed");
            }
            SipEvent::PresenceSubscribeFailed { uri, reason } => {
                tracing::warn!(uri, reason, "Presence subscribe failed");
                self.presence.insert(uri, PresenceState::Unknown);
            }
            SipEvent::PresenceUpdate { uri, state } => {
                self.presence.insert(uri, state);
            }
            SipEvent::MwiSubscribed { uri, expires } => {
                tracing::debug!(uri, expires, "MWI subscribed");
            }
            SipEvent::MwiSubscribeFailed { uri, reason } => {
                tracing::warn!(uri, reason, "MWI subscribe failed");
            }
            SipEvent::MwiUpdate { state, .. } => {
                self.accounts[account].mwi = Some(state);
            }
            SipEvent::MessageReceived { from, body } => {
                self.messages.push(Message {
                    peer_uri: from.clone(),
                    direction: Direction::Inbound,
                    body: body.clone(),
                    timestamp: unix_now(),
                });
                let _ = self.messages.save(&self.db);
                if self.config.notifications_enabled {
                    crate::platform::notify::notify_message_received(&short_uri(&from), &body);
                }
            }
            SipEvent::MessageSendResult { to, ok, reason } => {
                if !ok {
                    self.status_line = tf(
                        "status.message_send_failed",
                        &[("uri", &short_uri(&to)), ("reason", &reason.unwrap_or_default())],
                    );
                }
            }
        }
    }

    /// Which account subscribes on a contact's behalf: `presence_account`
    /// (matched by username, stable across account reordering) if set, else
    /// the first configured account -- covers the common single-account case
    /// with no extra clicks.
    pub(crate) fn resolve_presence_account(&self, contact: &Contact) -> Option<usize> {
        match &contact.presence_account {
            Some(username) => self.accounts.iter().position(|a| &a.account.username == username),
            None => {
                if self.accounts.is_empty() {
                    None
                } else {
                    Some(0)
                }
            }
        }
    }

    /// Flip DND for the live account at `idx` (into `self.accounts`) --
    /// takes effect immediately, since incoming-call handling reads DND from
    /// `self.accounts[i].account.dnd`, not the `self.config.accounts` Settings
    /// draft. Those two lists aren't the same or in the same order (`config`
    /// includes disabled accounts too), so the matching config entry is
    /// found by username -- same cross-referencing idiom as
    /// `resolve_presence_account` -- and kept in sync so Settings reflects
    /// the change and it survives a restart.
    pub(crate) fn toggle_dnd(&mut self, idx: usize) {
        let new_dnd = !self.accounts[idx].account.dnd;
        self.accounts[idx].account.dnd = new_dnd;
        let username = self.accounts[idx].account.username.clone();
        if let Some(cfg_acc) = self.config.accounts.iter_mut().find(|a| a.username == username) {
            cfg_acc.dnd = new_dnd;
        }
        if self.accounts[idx].account.publish_presence {
            self.accounts[idx].handle.publish_presence(!new_dnd);
        }
        self.save_config_quietly();
    }

    /// Subscribe every `watch_presence` contact resolved to `account`,
    /// called once that account has actually registered (subscribing before
    /// then would just hit the same 401/407 retry path unnecessarily).
    pub(crate) fn subscribe_account_contacts(&mut self, account: usize) {
        let targets: Vec<String> = self
            .contacts
            .contacts
            .iter()
            .filter(|c| c.watch_presence && self.resolve_presence_account(c) == Some(account))
            .map(|c| c.sip_uri.clone())
            .collect();
        for uri in targets {
            self.accounts[account].handle.subscribe_presence(uri);
        }
    }

    /// Subscribe to `account`'s own mailbox MWI state, if `mailbox` is
    /// configured — called once that account has actually registered, same
    /// reasoning as `subscribe_account_contacts`.
    pub(crate) fn subscribe_account_mwi(&mut self, account: usize) {
        let acc = &self.accounts[account].account;
        let Some(mailbox) = acc.mailbox.clone().filter(|s| !s.is_empty()) else {
            return;
        };
        let uri = normalize_target(&mailbox, &acc.server);
        self.accounts[account].handle.subscribe_mwi(uri);
    }

    /// A call in `calls` ended or an outstanding attempt failed — figure out
    /// which of `pending_call` / `pending_accept` / `calls` / `pending_outbound`
    /// `call_id` refers to, tear it down, and record it in history.
    pub(crate) fn on_call_terminated(&mut self, call_id: &str, failure: Option<(u16, String)>) {
        if let Some(pending) = self.pending_call.take() {
            if pending.call_id == call_id {
                self.record_history(pending.from, Direction::Inbound, pending.start_time, CallStatus::Missed);
                self.refresh_call_status();
                return;
            }
            self.pending_call = Some(pending);
        }
        // We told `SipStack` to accept this call but it declined on our
        // behalf (no compatible codec, RTP port allocation failure, etc. --
        // see `accept_call`'s doc comment) before `CallConnected` ever arrived.
        if let Some(pending) = self.pending_accept.take() {
            if pending.call_id == call_id {
                if let Some((code, reason)) = &failure {
                    self.status_line = tf("status.call_failed", &[("code", &code.to_string()), ("reason", reason)]);
                }
                self.record_history(pending.remote_uri, Direction::Inbound, pending.start_time, CallStatus::Rejected);
                self.refresh_call_status();
                return;
            }
            self.pending_accept = Some(pending);
        }
        if let Some(idx) = self.calls.iter().position(|c| c.call_id == call_id) {
            let status = if let Some((code, reason)) = &failure {
                self.status_line = tf("status.call_failed", &[("code", &code.to_string()), ("reason", reason)]);
                CallStatus::Failed
            } else {
                CallStatus::Answered
            };
            let slot = self.remove_call(idx);
            self.record_history(slot.remote_uri, slot.direction, slot.start_time, status);
            // Only recompute the idle/in-call status text on a clean end —
            // on failure, `status_line` above already holds the message the
            // user needs to see, and `refresh_call_status()` would instantly
            // clobber it back to "Ready" before it's ever rendered.
            if failure.is_none() {
                self.refresh_call_status();
            }
            return;
        }
        if let Some(out) = self.pending_outbound.take() {
            if let Some((code, reason)) = &failure {
                self.status_line = tf("status.call_failed", &[("code", &code.to_string()), ("reason", reason)]);
            }
            self.record_history(out.remote_uri, Direction::Outbound, out.start_time, CallStatus::Failed);
            if failure.is_none() {
                self.refresh_call_status();
            }
        }
    }

    /// Remove call `idx`, stopping its media first if it was focused or a
    /// conference leg (there's no "drop one party but keep mixing" mode --
    /// a surviving conference call gets ordinary single-leg media
    /// restarted). The single chokepoint both `do_hangup` and
    /// `on_call_terminated` go through. Returns the removed slot so the
    /// caller can record history from it.
    pub(crate) fn remove_call(&mut self, idx: usize) -> CallSlot {
        let was_conference = self.in_conference;
        if was_conference || self.focused_call == Some(idx) {
            self.stop_focused_media();
        }
        if was_conference {
            self.focused_call = None;
            self.in_conference = false;
        }
        let slot = self.calls.remove(idx);
        self.focused_call = match self.focused_call {
            Some(f) if f == idx => None,
            Some(f) if f > idx => Some(f - 1),
            other => other,
        };
        // Either call ending invalidates a pending attended transfer --
        // both legs must still exist for Complete Transfer to make sense.
        self.attended_transfer_original = None;
        if was_conference && !self.calls.is_empty() {
            self.start_media(0);
        }
        slot
    }

    /// Recompute the top status line from current call state: in-call text
    /// for the focused call, a "held" hint if calls remain but none is
    /// focused, or the idle registration summary once everything's cleared.
    pub(crate) fn refresh_call_status(&mut self) {
        if let Some(idx) = self.focused_call {
            self.status_line = tf("status.in_call", &[("uri", &short_uri(&self.calls[idx].remote_uri))]);
        } else if !self.calls.is_empty() {
            self.status_line = t("status.on_hold");
        } else if self.pending_call.is_none() {
            self.refresh_idle_status();
        }
    }

    /// Recompute the top status bar from the *selected* account's
    /// registration state — a no-op while a call or incoming ring is in
    /// progress, since call-related events drive `status_line` directly
    /// during that time. Call after any registration change or whenever
    /// `selected_account` changes (e.g. the dialer's account picker).
    pub(crate) fn refresh_idle_status(&mut self) {
        if !self.calls.is_empty() || self.pending_call.is_some() {
            return;
        }
        match self.accounts.get(self.selected_account) {
            Some(acc) => {
                self.reg_ok = acc.reg_ok;
                self.status_line = if acc.reg_ok { t("status.ready") } else { t("status.not_registered") };
            }
            // Distinct from genuinely having zero enabled accounts (below):
            // the background spawn task (see `process_account_spawn_events`)
            // is still working, so accounts configured in Settings just
            // haven't finished connecting yet -- don't flash "No accounts
            // configured" while they're on the way.
            None if self.account_spawn_rx.is_some() => {
                self.reg_ok = false;
                self.status_line = t("status.registering");
            }
            None => {
                self.reg_ok = false;
                self.status_line = t("status.no_accounts_configured");
            }
        }
    }

    pub(crate) fn record_history(
        &mut self, remote_uri: String, direction: Direction, start_time: u64, status: CallStatus,
    ) {
        let duration =
            if matches!(status, CallStatus::Answered) { (unix_now().saturating_sub(start_time)) as u32 } else { 0 };
        let is_missed = status == CallStatus::Missed;
        let record = CallRecord { remote_uri, direction, timestamp: start_time, duration_secs: duration, status };
        self.history.push(record);
        let _ = self.history.save(&self.db);
        if is_missed {
            self.unseen_missed_calls += 1;
            self.sync_tray_badge();
        }
    }

    /// Push `unseen_missed_calls` to the tray icon's badge overlay/tooltip.
    /// No-op if the tray failed to start.
    pub(crate) fn sync_tray_badge(&self) {
        if let Some((_, _, badge_tx)) = &self.tray {
            let _ = badge_tx.send(self.unseen_missed_calls);
        }
    }
}
