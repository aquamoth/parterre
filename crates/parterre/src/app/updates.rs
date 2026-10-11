use eframe::egui;
use parterre_telemetry::{Download, Update, UpdateCheck};

use super::ParterreApp;
use crate::dialogs::{self, Dialog};
use crate::widgets;

pub const RELEASE_TOLD_KEY: &str = "parterre-release-told";

impl ParterreApp {
    pub(super) fn update_check(&mut self, ctx: &egui::Context) {
        let prompting = self.telemetry.prompt.is_some();
        // Scripted runs never ask, or their screenshots would depend on it: `--newer-release`
        // stands in.
        if !self.settings.check_for_updates
            || self.automation.is_scripted()
            || !parterre_telemetry::may_check_for_updates(prompting)
        {
            self.update_check = None;
        } else if self.update_check.is_none() && parterre_telemetry::has_update_check() {
            let ctx = ctx.clone();
            self.update_check = UpdateCheck::start(crate::VERSION, move || ctx.request_repaint());
        }
        if let Some(update) = self.update_check.as_mut().and_then(|c| c.newer())
            && tell_once(&mut self.release_told, &update.version)
        {
            self.release_dialog = Some(update.clone());
        }
    }

    pub(super) fn release_dialog(&mut self, ctx: &egui::Context) {
        let Some(update) = &self.release_dialog else {
            return;
        };
        let this_version = crate::VERSION.split(' ').next().unwrap_or(crate::VERSION);
        let shown = Dialog::new("new-release", "New release")
            .screen(crate::usage::Screen::NewRelease)
            .width(360.0)
            .show(ctx, |ui| {
                let mut answer = ReleaseAnswer::Open;
                dialogs::fields(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!("parterre {} is out", update.version))
                            .size(16.0)
                            .strong(),
                    );
                    ui.label(format!("This is {this_version}."));
                    if ui.link("Release notes").clicked() {
                        answer = ReleaseAnswer::Notes;
                    }
                });
                ui.separator();
                let row = egui::vec2(ui.available_width(), 34.0);
                let layout = egui::Layout::right_to_left(egui::Align::Center);
                ui.allocate_ui_with_layout(row, layout, |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if widgets::primary_button(ui, "Download", 90.0).clicked() {
                        answer = ReleaseAnswer::Download;
                    }
                    if widgets::text_button(ui, "Close").clicked() {
                        answer = ReleaseAnswer::Close;
                    }
                });
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    answer = ReleaseAnswer::Close;
                }
                answer
            });
        let answer = if shown.should_close() {
            ReleaseAnswer::Close
        } else {
            shown.inner
        };
        match answer {
            ReleaseAnswer::Open => {}
            ReleaseAnswer::Close => self.release_dialog = None,
            ReleaseAnswer::Notes => {
                if let Err(e) = crate::browser::open(&update.notes()) {
                    self.status = Some((e, true));
                }
            }
            ReleaseAnswer::Download => {
                let download = update.download.clone();
                self.download(ctx, &download);
                self.release_dialog = None;
            }
        }
    }

    pub(super) fn newer_release(&mut self) -> Option<Update> {
        if !self.settings.check_for_updates {
            return None;
        }
        match &self.automation.canned_newer_release {
            Some(update) => Some(update.clone()),
            None => self.update_check.as_mut()?.newer().cloned(),
        }
    }

    pub(super) fn download(&mut self, ctx: &egui::Context, download: &Download) {
        crate::usage::action(crate::usage::Action::DownloadUpdate);
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

fn tell_once(told: &mut Option<String>, version: &str) -> bool {
    if told.as_deref() == Some(version) {
        return false;
    }
    *told = Some(version.to_owned());
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReleaseAnswer {
    Open,
    Close,
    Notes,
    Download,
}

#[cfg(test)]
mod tests {
    use super::tell_once;

    #[test]
    fn each_release_is_told_of_once() {
        let mut told = None;
        assert!(tell_once(&mut told, "0.8.0"));
        assert!(!tell_once(&mut told, "0.8.0"));
        assert!(tell_once(&mut told, "0.9.0"));
        assert_eq!(told.as_deref(), Some("0.9.0"));
    }
}
