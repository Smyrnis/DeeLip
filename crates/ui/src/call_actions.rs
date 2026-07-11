use deelip_config::{CallDirection, CallStatus, DtmfMode};

use crate::app::{DeelipApp, PendingAccept, PendingOutbound, Tab};
use crate::helpers::{normalize_target_with_prefix, short_uri, unix_now};
use crate::strings::t;

impl DeelipApp {
    // ── Call actions ─────────────────────────────────────────────────────────

    /// Domain to resolve a bare (non-"@", non-URI) dial-box/transfer/message
    /// target against -- this account's own identity domain, or empty for a
    /// `SipAccount::local_account`. A serverless account has no
    /// local-extension concept: every bare target dialed from one *is* the
    /// destination itself (see `normalize_target_with_prefix`'s empty-domain
    /// case), not a number to resolve against some PBX domain -- unlike
    /// `SipHandle.domain`, which is never empty (it falls back to
    /// `local_ip:local_port` precisely so headers stay valid), so that
    /// fallback alone can't be used to detect this.
    pub(crate) fn dial_domain(&self, acc: usize) -> String {
        if self.accounts[acc].account.local_account {
            String::new()
        } else {
            self.accounts[acc].handle.domain.clone()
        }
    }

    /// Core dialing mechanics shared by ordinary dialing and the attended-
    /// transfer consultation call -- no gating here; callers check their
    /// own preconditions before calling this, mirroring this codebase's
    /// existing per-call-site `can_dial` convention rather than
    /// centralizing it. Takes an explicit account so the consultation call
    /// can be placed from the *same* account as the call being
    /// transferred, rather than whichever account the Dialer tab happens
    /// to have selected. `attempt_ice` is false for the attended-transfer
    /// consultation call -- ICE is scoped to ordinary calls for now (see
    /// `try_gather_ice`'s doc comment), conference/transfer legs keep the
    /// plain STUN/TURN-fallback path unchanged.
    pub(crate) fn place_call(&mut self, acc: usize, target: &str, attempt_ice: bool) {
        let domain = self.dial_domain(acc);
        let prefix = self.accounts[acc].account.dialing_prefix.clone().unwrap_or_default();
        let dial_plan = self.accounts[acc].account.dial_plan.clone();
        let t = normalize_target_with_prefix(target, &domain, &prefix, &dial_plan);
        self.accounts[acc].handle.make_call(&t, attempt_ice);
        self.last_dialed = Some(t.clone());
        self.pending_outbound = Some(PendingOutbound {
            remote_uri: t.clone(),
            start_time: unix_now(),
        });
        self.status_line = format!("Calling {}…", short_uri(&t));
    }

    pub(crate) fn do_call(&mut self, target: Option<String>) {
        let raw = target.unwrap_or_else(|| self.call_target.trim().to_string());
        if raw.is_empty() {
            return;
        }
        let Some(acc) = self.selected_account_idx() else {
            return;
        };
        self.place_call(acc, &raw, true);
    }

    pub(crate) fn do_redial(&mut self) {
        if let Some(target) = self.last_dialed.clone() {
            self.do_call(Some(target));
        }
    }

    /// Switch to the Dialer tab, load `target` into the dial box, and place
    /// the call immediately if idle and registered -- shared by History's
    /// and Contacts' "Call" buttons, which used to each hand-roll this
    /// identical sequence.
    pub(crate) fn dial_from_list(&mut self, target: String) {
        self.tab = Tab::Dialer;
        self.call_target = target.clone();
        let can_dial =
            self.calls.is_empty() && self.pending_call.is_none() && self.pending_outbound.is_none();
        if can_dial && self.reg_ok {
            self.do_call(Some(target));
        }
    }

    /// Open the Messages window scoped to `target` -- the only way the
    /// window is ever opened (there's no tab-bar entry point). Shared by
    /// History's/Contacts'/Directory's "Message" right-click actions and
    /// `DefaultListAction::Message`'s double-click behavior. Re-scopes an
    /// already-open window to a different peer just as well as opening a
    /// fresh one.
    pub(crate) fn message_from_list(&mut self, target: String) {
        self.messages_window_open = true;
        self.messages_window_peer = Some(target);
    }

    /// Start the consultation call for an attended transfer: holds the
    /// focused call and dials `self.attended_target` as a genuine 2nd
    /// outbound call, placed from the *same* account. This is the one path
    /// allowed to dial while a call is already connected — normal dialing
    /// stays blocked by `can_dial` everywhere else, matching the existing
    /// "up to 2 concurrent calls" cap.
    pub(crate) fn do_attended_transfer_dial(&mut self) {
        let Some(idx) = self.focused_call else { return };
        let raw = self.attended_target.trim().to_string();
        if raw.is_empty() {
            return;
        }
        if self.calls.len() != 1 || self.pending_call.is_some() || self.pending_outbound.is_some() {
            return;
        }
        let acc = self.calls[idx].account;
        self.do_hold_slot(idx);
        self.attended_transfer_original = Some(idx);
        self.attended_target.clear();
        self.showing_attended = false;
        self.place_call(acc, &raw, false);
    }

    /// Complete a pending attended transfer: send REFER-with-Replaces on
    /// the original call, referencing the consultation call's dialog.
    /// Both legs are hung up once `TransferAccepted` confirms the far end
    /// accepted it (see `handle_sip_event`), not here.
    pub(crate) fn do_complete_attended_transfer(&mut self) {
        let Some(original_idx) = self.attended_transfer_original else {
            return;
        };
        if self.calls.len() != 2 {
            return;
        }
        let consult_idx = 1 - original_idx;
        let acc = self.calls[original_idx].account;
        let original_call_id = self.calls[original_idx].call_id.clone();
        let consult_call_id = self.calls[consult_idx].call_id.clone();
        self.accounts[acc]
            .handle
            .attended_transfer(&original_call_id, &consult_call_id);
        self.status_line = t("status.completing_transfer");
    }

    pub(crate) fn do_accept(&mut self) {
        // `pending_accept` is a single slot, tracking one in-flight accept
        // at a time -- accepting a 2nd call before the first's
        // `CallConnected` arrives would silently overwrite it, orphaning
        // the first as a connected-but-invisible call the moment its event
        // finally lands (see `CallConnected`'s `pending_accept` matching in
        // `event_handling.rs`). Deliberately a no-op rather than an error:
        // `pending_call` (and its "incoming"/"call waiting" banner) is left
        // untouched, so the user can just try Accept again once the first
        // call visibly connects.
        if self.pending_accept.is_some() {
            return;
        }
        let Some(pending) = self.pending_call.take() else {
            return;
        };
        let acc = pending.account;
        // Codec negotiation / RTP port / ICE / TURN all happen inside
        // `SipStack` now -- if any of that fails, it declines on our behalf
        // and reports back via `SipEvent::CallFailed` (see
        // `on_call_terminated`'s `pending_accept` handling), not here.
        // Deliberately *not* holding/freeing the currently-focused call's
        // media here anymore: that used to happen eagerly, before this
        // could ever fail, so a decline (no compatible codec, RTP port
        // allocation failure) would needlessly put an already-active call
        // on hold with nothing to auto-resume it. Deferred to the
        // `CallConnected` handler (`event_handling.rs`), which only runs
        // once accept has actually succeeded.
        self.accounts[acc].handle.accept_call(&pending.call_id);
        self.pending_accept = Some(PendingAccept {
            call_id: pending.call_id,
            remote_uri: pending.from,
            start_time: pending.start_time,
        });
        self.status_line = "Accepting…".into();
        self.refresh_call_status();
    }

    pub(crate) fn do_reject(&mut self) {
        if let Some(pending) = self.pending_call.take() {
            self.record_history(
                pending.from,
                CallDirection::Inbound,
                pending.start_time,
                CallStatus::Rejected,
            );
            self.accounts[pending.account]
                .handle
                .reject_call(&pending.call_id);
            self.refresh_call_status();
        }
    }

    pub(crate) fn do_hangup(&mut self, idx: usize) {
        let call_id = self.calls[idx].call_id.clone();
        let acc = self.calls[idx].account;
        self.accounts[acc].handle.hang_up(&call_id);
        let slot = self.remove_call(idx);
        self.record_history(
            slot.remote_uri,
            slot.direction,
            slot.start_time,
            CallStatus::Answered,
        );
        self.refresh_call_status();
    }

    /// Send the hold re-INVITE for `idx` (optimistic — doesn't wait for the
    /// confirming `SipEvent::CallHeld`). Doesn't touch `media`/`focused_call`;
    /// callers that are actually switching audio away from this call do that
    /// themselves (see `do_hold_slot`/`do_accept`/`do_swap_to`).
    pub(crate) fn send_hold(&mut self, idx: usize) {
        let call_id = self.calls[idx].call_id.clone();
        let acc = self.calls[idx].account;
        self.calls[idx].is_held = true;
        self.accounts[acc].handle.hold_call(&call_id);
    }

    pub(crate) fn send_resume(&mut self, idx: usize) {
        let call_id = self.calls[idx].call_id.clone();
        let acc = self.calls[idx].account;
        self.accounts[acc].handle.resume_call(&call_id);
    }

    /// Hold call `idx` — if it's the focused one, its media stops and no
    /// call has live audio until the user swaps back to something.
    pub(crate) fn do_hold_slot(&mut self, idx: usize) {
        self.send_hold(idx);
        if self.focused_call == Some(idx) {
            if let Some(engine) = self.media.take() {
                self.rt.block_on(engine.stop());
            }
            if let Some(v) = self.video.take() {
                self.rt.block_on(v.engine.stop());
            }
            self.focused_call = None;
        }
        self.refresh_call_status();
    }

    /// Switch live audio to call `idx`: holds whatever's currently focused
    /// (there's at most one other call), then resumes and restarts media
    /// for `idx` using its originally-negotiated `CallMediaReady` (the
    /// negotiated RTP endpoint doesn't change between hold and resume, so
    /// there's nothing new to learn from a resume re-INVITE's response).
    pub(crate) fn do_swap_to(&mut self, idx: usize) {
        if self.focused_call == Some(idx) {
            return;
        }
        if let Some(cur) = self.focused_call {
            self.send_hold(cur);
            if let Some(engine) = self.media.take() {
                self.rt.block_on(engine.stop());
            }
            if let Some(v) = self.video.take() {
                self.rt.block_on(v.engine.stop());
            }
            self.focused_call = None;
        }
        self.send_resume(idx);
        self.calls[idx].is_held = false;
        self.start_media(idx);
        self.refresh_call_status();
    }

    /// `selected_account` clamped to a valid index — `None` if there are no
    /// accounts at all (nothing to call from).
    pub(crate) fn selected_account_idx(&self) -> Option<usize> {
        if self.accounts.is_empty() {
            return None;
        }
        Some(self.selected_account.min(self.accounts.len() - 1))
    }

    pub(crate) fn do_dtmf(&self, digit: char) {
        let Some(idx) = self.focused_call else { return };
        let call = &self.calls[idx];
        let mode = self.accounts[call.account].account.dtmf_mode;
        // `Auto` picks per-call from the already-negotiated media: RFC 2833
        // if the far end offered a telephone-event payload type, else SIP
        // INFO (doesn't depend on SDP negotiation at all, so it's always
        // available as the fallback).
        let mode = match mode {
            DtmfMode::Auto if call.media.dtmf_type.is_some() => DtmfMode::Rfc2833,
            DtmfMode::Auto => DtmfMode::SipInfo,
            other => other,
        };
        match mode {
            DtmfMode::Rfc2833 => {
                if let Some(engine) = &self.media {
                    engine.send_dtmf(digit);
                }
            }
            DtmfMode::Inband => {
                if let Some(engine) = &self.media {
                    engine.send_dtmf_inband(digit);
                }
            }
            DtmfMode::SipInfo => {
                self.accounts[call.account]
                    .handle
                    .send_dtmf_info(&call.call_id, digit);
            }
            DtmfMode::Auto => unreachable!("resolved to Rfc2833 or SipInfo above"),
        }
    }

    pub(crate) fn is_muted(&self) -> bool {
        self.media.as_ref().is_some_and(|m| m.is_muted())
    }

    pub(crate) fn do_mute_toggle(&self) {
        if let Some(engine) = &self.media {
            engine.set_muted(!engine.is_muted());
        }
    }

    /// Whether the focused call's `MediaEngine` is currently recording --
    /// true either because auto-record is on, or because the user manually
    /// started it with `do_record_toggle` below.
    pub(crate) fn is_recording(&self) -> bool {
        self.media.as_ref().is_some_and(|m| m.is_recording())
    }

    /// Manual per-call Record button -- independent of the global
    /// auto-record setting, same shape as `do_mute_toggle`. Also persists
    /// onto the focused `CallSlot` (not just the live `MediaEngine`) so a
    /// later hold/resume -- which tears down and rebuilds the engine from
    /// scratch (see `do_hold_slot`/`do_swap_to`) -- restarts recording (or
    /// not) exactly as the user last manually set it here, instead of
    /// `start_media` falling back to the global auto-record setting again.
    pub(crate) fn do_record_toggle(&mut self) {
        if let Some(engine) = &self.media {
            let new_state = !engine.is_recording();
            engine.set_recording(new_state);
            if let Some(idx) = self.focused_call {
                self.calls[idx].recording_enabled = new_state;
            }
        }
    }

    /// Live in-call speaker/mic volume sliders -- `1.0` (unity gain) when
    /// there's no active call, same "no-op without an engine" shape as
    /// `is_muted`.
    pub(crate) fn output_gain(&self) -> f32 {
        self.media.as_ref().map_or(1.0, |m| m.output_gain())
    }
    pub(crate) fn set_output_gain(&self, gain: f32) {
        if let Some(engine) = &self.media {
            engine.set_output_gain(gain);
        }
    }
    pub(crate) fn input_gain(&self) -> f32 {
        self.media.as_ref().map_or(1.0, |m| m.input_gain())
    }
    pub(crate) fn set_input_gain(&self, gain: f32) {
        if let Some(engine) = &self.media {
            engine.set_input_gain(gain);
        }
    }

    /// Blind-transfer the focused call to `self.transfer_target`.
    pub(crate) fn do_transfer(&mut self) {
        let Some(idx) = self.focused_call else { return };
        let raw = self.transfer_target.trim().to_string();
        if raw.is_empty() {
            return;
        }
        let acc = self.calls[idx].account;
        let domain = self.dial_domain(acc);
        let prefix = self.accounts[acc].account.dialing_prefix.clone().unwrap_or_default();
        let dial_plan = self.accounts[acc].account.dial_plan.clone();
        let target = normalize_target_with_prefix(&raw, &domain, &prefix, &dial_plan);
        let call_id = self.calls[idx].call_id.clone();
        self.accounts[acc].handle.blind_transfer(&call_id, target);
        self.status_line = "Transferring…".into();
        self.transfer_target.clear();
        self.showing_transfer = false;
    }

    /// If the pending incoming call has a no-answer-forward deadline and
    /// it's elapsed, redirect it (302) instead of leaving it ringing forever.
    /// Called once per frame from `update()`.
    pub(crate) fn check_pending_call_timeout(&mut self) {
        let Some(pending) = &self.pending_call else {
            return;
        };
        let now = unix_now();
        if let Some(at) = pending.auto_answer_at {
            if now >= at {
                self.do_accept();
                return;
            }
        }
        let Some((deadline, target)) = &pending.forward else {
            return;
        };
        if now < *deadline {
            return;
        }
        let target = target.clone();
        let Some(pending) = self.pending_call.take() else {
            return;
        };
        self.accounts[pending.account]
            .handle
            .redirect_call(&pending.call_id, target);
        self.record_history(
            pending.from,
            CallDirection::Inbound,
            pending.start_time,
            CallStatus::Missed,
        );
        self.refresh_call_status();
    }
}
