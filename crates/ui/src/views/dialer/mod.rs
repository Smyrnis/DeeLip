//! Dialer view: the idle dial pad, the in-call screen, and the Transfer
//! Call / Keypad pop-out windows, split by concern across
//! `idle.rs`/`in_call.rs`/`transfer_window.rs`/`dtmf_window.rs`. All still
//! just `impl DeelipApp` blocks -- the split is purely organizational, same
//! precedent as `sip-core/src/call/lifecycle/` and `views/settings/`
//! (cross-file inherent-method calls like `self.show_dialer_idle(ui)` work
//! regardless of which file defines the method).

mod dtmf_window;
mod idle;
mod in_call;
mod redirect_window;
mod transfer_window;

use egui::Ui;

use crate::app::DeelipApp;

impl DeelipApp {
    pub(crate) fn show_dialer(&mut self, ui: &mut Ui) {
        let idle = self.calls.is_empty() && self.pending_call.is_none() && self.pending_outbound.is_none();
        if idle {
            self.show_dialer_idle(ui);
        } else {
            self.show_dialer_in_call(ui);
        }
    }
}
