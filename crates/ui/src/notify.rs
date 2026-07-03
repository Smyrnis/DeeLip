//! Desktop notifications (via D-Bus, using the pure-Rust `notify-rust` +
//! `zbus` stack — no native libdbus/libnotify dependency needed).

/// Fire a "you have an incoming call" desktop notification. Best-effort —
/// failures (no notification daemon running, etc.) are logged, not fatal.
pub fn notify_incoming_call(from: &str) {
    let body = format!("Incoming call from {from}");
    if let Err(e) = notify_rust::Notification::new()
        .summary("DeeLip")
        .body(&body)
        .show()
    {
        tracing::warn!("Desktop notification failed: {e}");
    }
}
