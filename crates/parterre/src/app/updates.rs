//! The update check (#258): while a newer release is out, the ☰ icon is bold blue and the
//! menu ends in *Download X.Y.Z*. No popup, nothing to dismiss; Settings › Privacy turns it
//! off. Decided in #226.

use eframe::egui;
use parterre_telemetry::{Download, Update, UpdateCheck};

use super::ParterreApp;

impl ParterreApp {
    /// Starts or stops asking GitHub as the setting says. Scripted runs don't ask: their pictures
    /// would depend on it. `--newer-release` answers for them.
    pub(super) fn update_check(&mut self, ctx: &egui::Context) {
        if !self.settings.check_for_updates || self.automation.is_active() {
            self.update_check = None;
        } else if self.update_check.is_none() && parterre_telemetry::has_update_check() {
            let ctx = ctx.clone();
            self.update_check = UpdateCheck::start(crate::VERSION, move || ctx.request_repaint());
        }
    }

    /// The newer release the menu offers, if any.
    pub(super) fn newer_release(&mut self) -> Option<Update> {
        if !self.settings.check_for_updates {
            return None;
        }
        match &self.automation.newer_release {
            Some(update) => Some(update.clone()),
            None => self.update_check.as_mut()?.newer().cloned(),
        }
    }

    /// *Download*: the release's file for this channel in the browser, or on the cargo channel
    /// the command copied.
    pub(super) fn download(&mut self, ctx: &egui::Context, download: &Download) {
        match download {
            Download::Open(url) => {
                if let Err(e) = crate::browser::open(url) {
                    self.status = Some((e, true));
                }
            }
            Download::Copy(command) => {
                ctx.copy_text((*command).to_owned());
                self.status = Some((format!("Copied: {command}"), false));
            }
        }
    }
}
