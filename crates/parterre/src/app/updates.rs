use eframe::egui;
use parterre_telemetry::{Download, Update, UpdateCheck};

use super::{Note, ParterreApp};
use crate::dialogs::{self, Dialog};
use crate::widgets;

pub const RELEASE_TOLD_KEY: &str = "parterre-release-told";

/// Read as the `Option` the app's [`eframe::App::save`] stores: read as a `String`, `Some("1.0.0")`
/// didn't parse and every start told of the same release again (#382).
pub fn release_told(storage: &dyn eframe::Storage) -> Option<String> {
    eframe::get_value::<Option<String>>(storage, RELEASE_TOLD_KEY).flatten()
}

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
        const CLOSE: &str = "Close";
        const DOWNLOAD: &str = "Download";
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
                let widths = [
                    widgets::text_button_width(ui, CLOSE),
                    widgets::filled_button_width(ui, DOWNLOAD, dialogs::PRIMARY_MIN_WIDTH),
                ];
                dialogs::button_row(ui, &widths, |ui| {
                    if widgets::text_button(ui, CLOSE).clicked() {
                        answer = ReleaseAnswer::Close;
                    }
                    let download =
                        widgets::primary_button(ui, DOWNLOAD, dialogs::PRIMARY_MIN_WIDTH);
                    if download.clicked() {
                        answer = ReleaseAnswer::Download;
                    }
                });
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    answer = ReleaseAnswer::Close;
                }
                answer
            });
        let answer = shown.or_closed(ReleaseAnswer::Close);
        match answer {
            ReleaseAnswer::Open => {}
            ReleaseAnswer::Close => self.release_dialog = None,
            ReleaseAnswer::Notes => {
                if let Err(e) = crate::browser::open(&update.notes()) {
                    self.status = Some(Note::Error(e));
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
                    self.status = Some(Note::Error(e));
                }
            }
            Download::Copy(command) => {
                ctx.copy_text((*command).to_owned());
                self.status = Some(Note::Info(format!("Copied: {command}")));
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
    use super::{RELEASE_TOLD_KEY, release_told, tell_once};
    use eframe::Storage as _;

    #[test]
    fn each_release_is_told_of_once() {
        let mut told = None;
        assert!(tell_once(&mut told, "0.8.0"));
        assert!(!tell_once(&mut told, "0.8.0"));
        assert!(tell_once(&mut told, "0.9.0"));
        assert_eq!(told.as_deref(), Some("0.9.0"));
    }

    #[derive(Default)]
    struct Memory(std::collections::HashMap<String, String>);

    impl eframe::Storage for Memory {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn the_release_told_of_is_remembered_across_starts() {
        let mut storage = Memory::default();
        assert_eq!(release_told(&storage), None);
        eframe::set_value(&mut storage, RELEASE_TOLD_KEY, &Some("1.0.0".to_owned()));
        assert_eq!(release_told(&storage).as_deref(), Some("1.0.0"));
        // As 1.0.0-rc1 stored it.
        storage.set_string(RELEASE_TOLD_KEY, r#"Some("1.0.0")"#.to_owned());
        assert_eq!(release_told(&storage).as_deref(), Some("1.0.0"));
        eframe::set_value(&mut storage, RELEASE_TOLD_KEY, &None::<String>);
        assert_eq!(release_told(&storage), None);
    }
}
