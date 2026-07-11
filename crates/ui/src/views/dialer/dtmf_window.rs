use crate::app::{DeelipApp, SharedApp};
use crate::helpers::{phone_keypad, show_pop_out_window};
use crate::strings::t;

impl DeelipApp {
    /// In-call DTMF keypad as a real separate OS window, same `Deferred`-
    /// viewport pattern as `show_transfer_window` (see `show_messages_window`'s
    /// doc comment for the full rationale) -- previously rendered inline as
    /// a card in the main window, inconsistent with Transfer/Contacts once
    /// those were promoted to real windows.
    pub(crate) fn show_dtmf_window(&mut self, ctx: &egui::Context, self_app: SharedApp) {
        show_pop_out_window(
            self,
            ctx,
            self_app,
            "deelip_dtmf_window",
            format!("DeeLip {}", t("dialer.keypad_window_title")),
            [260.0, 360.0],
            [240.0, 340.0],
            false,
            |app| app.showing_dtmf,
            |app| app.showing_dtmf = false,
            |_app| t("dialer.keypad_window_title"),
            |app, ui| {
                let palette = app.palette;
                phone_keypad(ui, palette, |digit| app.do_dtmf(digit));
            },
        );
    }
}
