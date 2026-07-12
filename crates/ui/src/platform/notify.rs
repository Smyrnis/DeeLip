//! Desktop notifications (via D-Bus, using the pure-Rust `notify-rust` +
//! `zbus` stack). See `docs/crates/ui.md`'s "Platform integration" section for why
//! each notification gets its own background thread.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};

use crate::platform::tray::CtxSlot;
use crate::strings::{t, tf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationAction {
    Accept,
    Reject,
}

type ActionChannel = (Sender<(String, NotificationAction)>, Mutex<Receiver<(String, NotificationAction)>>);

fn action_channel() -> &'static ActionChannel {
    static CHANNEL: OnceLock<ActionChannel> = OnceLock::new();
    CHANNEL.get_or_init(|| {
        let (tx, rx) = channel();
        (tx, Mutex::new(rx))
    })
}

/// Fire a "you have an incoming call" desktop notification with Accept/
/// Reject buttons. Best-effort — failures (no notification daemon running,
/// a daemon that ignores actions entirely, etc.) are logged, not fatal; the
/// call is still perfectly answerable/rejectable from the app UI either way.
pub fn notify_incoming_call(call_id: &str, from: &str, ctx_slot: CtxSlot) {
    let body = tf("notify.incoming_call_body", &[("from", from)]);
    let mut notification = notify_rust::Notification::new();
    notification
        .summary("DeeLip")
        .body(&body)
        .action("accept", &t("common.accept_button"))
        .action("reject", &t("common.reject_button"));

    match notification.show() {
        Ok(handle) => {
            let call_id = call_id.to_string();
            let tx = action_channel().0.clone();
            std::thread::spawn(move || {
                handle.wait_for_action(|action| {
                    let resolved = match action {
                        "accept" => Some(NotificationAction::Accept),
                        "reject" => Some(NotificationAction::Reject),
                        // "default" (body click) and "__closed" (dismissed
                        // with no action) intentionally do nothing -- only
                        // an explicit button press should change call state.
                        _ => None,
                    };
                    if let Some(action) = resolved {
                        let _ = tx.send((call_id, action));
                        if let Some(ctx) = ctx_slot.lock().unwrap().as_ref() {
                            ctx.request_repaint_of(egui::ViewportId::ROOT);
                        }
                    }
                });
            });
        }
        Err(e) => tracing::warn!("Desktop notification failed: {e}"),
    }
}

/// Fire a plain "you have a new message" desktop notification — no action
/// buttons, just informational (unlike `notify_incoming_call`, there's no
/// call-state decision to make from the notification itself). Best-effort,
/// same as above.
pub fn notify_message_received(from: &str, body: &str) {
    let mut notification = notify_rust::Notification::new();
    notification.summary(&tf("notify.message_from", &[("from", from)])).body(body);
    match notification.show() {
        Ok(_) => tracing::debug!("Message notification shown for {from}"),
        Err(e) => tracing::warn!("Desktop notification failed: {e}"),
    }
}

/// Drain every action received since the last call — call once per frame.
/// Each entry's `call_id` should be checked against whatever's actually
/// still pending before acting on it: a notification's action can arrive
/// after its call already ended some other way (timed out, hung up
/// remotely, answered from the app itself).
pub fn poll_actions() -> Vec<(String, NotificationAction)> {
    let (_, rx) = action_channel();
    let rx = rx.lock().unwrap();
    let mut out = Vec::new();
    while let Ok(item) = rx.try_recv() {
        out.push(item);
    }
    out
}
