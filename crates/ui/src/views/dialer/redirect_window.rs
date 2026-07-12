use egui::{RichText, Ui};

use crate::app::{DeelipApp, SharedApp};
use crate::helpers::show_pop_out_window;
use crate::strings::t;

use super::transfer_window::transfer_target_editor;

impl DeelipApp {
    /// Redirect an unanswered incoming call (302 Moved Temporarily) to another
    /// destination without picking it up -- same real-separate-OS-window idiom
    /// as `show_transfer_window`, but sourcing `self.pending_call` instead of a
    /// connected `self.focused_call`, since `do_transfer` can't be reused here
    /// (see `call_actions.rs::do_redirect_pending_call`'s own doc comment).
    pub(crate) fn show_redirect_window(&mut self, ctx: &egui::Context, self_app: SharedApp) {
        show_pop_out_window(
            self,
            ctx,
            self_app,
            "deelip_redirect_window",
            format!("DeeLip {}", t("dialer.redirect_window_title")),
            [320.0, 600.0],
            [320.0, 600.0],
            false,
            |app| app.showing_redirect,
            |app| app.showing_redirect = false,
            |_app| t("dialer.redirect_window_title"),
            |app, ui| app.show_redirect_window_content(ui),
        );
    }

    fn show_redirect_window_content(&mut self, ui: &mut Ui) {
        let palette = self.palette;
        ui.add_space(4.0);
        if let Some(pending) = &self.pending_call {
            let (name, _is_name) = self.caller_display(&pending.from);
            ui.label(RichText::new(t("dialer.redirect_caller_label")).color(palette.ink_muted).small());
            ui.label(RichText::new(name).color(palette.ink));
        }
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        transfer_target_editor(ui, palette, &mut self.redirect_target);
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            if ui.button(format!("{}  {}", egui_phosphor::regular::EXPORT, t("dialer.redirect_button"))).clicked() {
                self.do_redirect_pending_call();
            }
        });
    }
}
