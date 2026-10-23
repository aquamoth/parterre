use eframe::egui;
use parterre_core::glyphs;
use parterre_telemetry::{Channel, Download, Install, Ring, Update, UpdateCheck};

use super::settings_window::SettingsPage;
use super::{Note, ParterreApp};
use crate::dialogs::{self, Dialog};
use crate::self_update::{self, Failure, Fallback, Method, Stage};
use crate::widgets;

pub const RELEASE_TOLD_KEY: &str = "parterre-release-told";

/// Read as the `Option` the app's [`eframe::App::save`] stores: read as a `String`, `Some("1.0.0")`
/// didn't parse and every start told of the same release again (#382).
pub fn release_told(storage: &dyn eframe::Storage) -> Option<String> {
    eframe::get_value::<Option<String>>(storage, RELEASE_TOLD_KEY).flatten()
}

/// What an installation in `ring` hears of.
pub(super) fn builds(ring: Ring) -> &'static str {
    match ring {
        Ring::Stable => "stable releases",
        Ring::Rc => "release candidates",
        Ring::Nightly => "nightly builds",
        Ring::Canary => "canary builds",
    }
}

/// The new-release dialog's title, by the ring of the build it tells of.
fn dialog_title(ring: Ring) -> &'static str {
    match ring {
        Ring::Stable => "New release",
        Ring::Rc => "New release candidate",
        Ring::Nightly => "New nightly build",
        Ring::Canary => "New canary build",
    }
}

/// This build's version without its commit: `1.3.4-nightly.20261016`.
fn running_version(version: &str) -> &str {
    version.split(['+', ' ']).next().unwrap_or(version)
}

impl ParterreApp {
    /// The ring the update check asks for: the stored one, as this copy's channel can take it.
    pub(super) fn heard_ring(&self) -> Ring {
        self.telemetry
            .privacy
            .ring()
            .unwrap_or(Ring::Stable)
            .on(Channel::current())
    }

    pub(super) fn update_check(&mut self, ctx: &egui::Context) {
        let prompting = self.telemetry.prompt.is_some();
        // Scripted runs never ask, or their screenshots would depend on it: `--newer-release`
        // stands in.
        if !self.settings.check_for_updates
            || self.automation.is_scripted()
            || !parterre_telemetry::may_check_for_updates(prompting)
        {
            self.update_check = None;
        } else if parterre_telemetry::has_update_check() {
            let ring = self.telemetry.privacy.ring().unwrap_or(Ring::Stable);
            if self.update_check.as_ref().is_none_or(|(r, _)| *r != ring) {
                let ctx = ctx.clone();
                self.update_check =
                    UpdateCheck::start(crate::version(), ring, move || ctx.request_repaint())
                        .map(|check| (ring, check));
            }
        }
        if let Some(update) = self.update_check.as_mut().and_then(|(_, c)| c.newer())
            && tell_once(&mut self.release_told, &update.version)
        {
            self.release_dialog = Some(update.clone());
        }
    }

    pub(super) fn release_dialog(&mut self, ctx: &egui::Context) {
        const CLOSE: &str = "Close";
        self.follow_update(ctx);
        let Some(update) = self.release_dialog.clone() else {
            return;
        };
        let heard = builds(self.heard_ring());
        let this_version = running_version(crate::version());
        let next = self.next_step(&update);
        let stage = self.updating.as_mut().map(|u| u.run.stage().clone());
        let method = self.updating.as_ref().map(|u| u.run.method());
        let primary = match (&next, &stage) {
            (Next::Download | Next::Fetch, _) => Some(("Download", ReleaseAnswer::Next)),
            (Next::Install, _) => Some(("Restart to update", ReleaseAnswer::Next)),
            (_, Some(Stage::Failed(failure))) => Some(match &failure.fallback {
                Some(Fallback::Open(file)) if is_disk_image(file) => {
                    ("Open the disk image", ReleaseAnswer::Fallback)
                }
                Some(Fallback::Open(_)) => ("Open the installer", ReleaseAnswer::Fallback),
                Some(Fallback::Show(_)) => ("Show the download", ReleaseAnswer::Fallback),
                None => ("Release page", ReleaseAnswer::Notes),
            }),
            _ => None,
        };
        let shown = Dialog::new("new-release", dialog_title(update.ring))
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
                    ui.horizontal(|ui| {
                        ui.label(format!("You hear of {heard}."));
                        if ui.link("Change").clicked() {
                            answer = ReleaseAnswer::Change;
                        }
                    });
                    if let Some(stage) = &stage {
                        ui.add_space(6.0);
                        stage_note(ui, stage, method);
                    }
                });
                ui.separator();
                let mut widths = vec![widgets::text_button_width(ui, CLOSE)];
                if let Some((label, _)) = primary {
                    widths.push(widgets::filled_button_width(
                        ui,
                        label,
                        dialogs::PRIMARY_MIN_WIDTH,
                    ));
                }
                dialogs::button_row(ui, &widths, |ui| {
                    if widgets::text_button(ui, CLOSE).clicked() {
                        answer = ReleaseAnswer::Close;
                    }
                    if let Some((label, primary)) = primary
                        && widgets::primary_button(ui, label, dialogs::PRIMARY_MIN_WIDTH).clicked()
                    {
                        answer = primary;
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
            ReleaseAnswer::Close => {
                // A download or an install runs on, followed by the toolbar's button. What has
                // ended is let go, so that the button offers the download again.
                if matches!(stage, Some(Stage::Failed(_) | Stage::Installed { .. })) {
                    self.updating = None;
                }
                self.release_dialog = None;
            }
            ReleaseAnswer::Notes => {
                if let Err(e) = crate::browser::open(&update.notes()) {
                    self.status = Some(Note::Error(e));
                }
            }
            ReleaseAnswer::Change => self.open_settings(SettingsPage::Privacy),
            ReleaseAnswer::Next => {
                self.get_update(ctx, &update);
                if next == Next::Download {
                    self.release_dialog = None;
                }
            }
            ReleaseAnswer::Fallback => {
                let opened = match &stage {
                    Some(Stage::Failed(Failure {
                        fallback: Some(Fallback::Open(file)),
                        ..
                    })) => crate::browser::open_file(file),
                    Some(Stage::Failed(Failure {
                        fallback: Some(Fallback::Show(dir)),
                        ..
                    })) => crate::file_manager::open(dir),
                    _ => Ok(()),
                };
                if let Err(e) = opened {
                    self.status = Some(Note::Error(e));
                }
                self.updating = None;
                self.release_dialog = None;
            }
        }
    }

    /// What the next click on the toolbar's button, the dialog's button or the menu's entry does
    /// for `update` (#433).
    pub(super) fn next_step(&mut self, update: &Update) -> Next {
        if installs(update).is_none() {
            return Next::Download;
        }
        match self.updating.as_mut().map(|u| u.run.stage().clone()) {
            None | Some(Stage::Cancelled) => Next::Fetch,
            Some(Stage::Downloading { done, size }) => Next::Wait { done, size },
            Some(Stage::Downloaded) => Next::Install,
            Some(Stage::Installing | Stage::Restart(_)) => Next::Busy,
            Some(Stage::Failed(_)) => Next::Show { failed: true },
            Some(Stage::Installed { .. }) => Next::Show { failed: false },
        }
    }

    /// Takes the update's next step, as the user clicked for it: the download, then the install
    /// (#433). Where parterre doesn't install it itself, *Download* as before.
    pub(super) fn get_update(&mut self, ctx: &egui::Context, update: &Update) {
        match self.next_step(update) {
            Next::Download => self.download(ctx, &update.download),
            Next::Fetch => self.start_update(ctx, update),
            Next::Install => {
                if let Some(updating) = self.updating.as_mut() {
                    updating.run.install(ctx);
                }
            }
            Next::Show { .. } => self.release_dialog = Some(update.clone()),
            Next::Wait { .. } | Next::Busy => {}
        }
    }

    /// Downloads the update on the channels parterre installs itself on (#431); the toolbar's
    /// button shows how far it is.
    fn start_update(&mut self, ctx: &egui::Context, update: &Update) {
        let Some((install, method)) = installs(update) else {
            return;
        };
        crate::usage::action(crate::usage::Action::DownloadUpdate);
        let run =
            self_update::Updating::start(&update.version, install.clone(), method, ctx.clone());
        self.updating = Some(Updating {
            update: update.clone(),
            run,
        });
    }

    /// Starts the new copy once it is in place and closes this one, and opens the dialog again
    /// on an outcome the user should see.
    fn follow_update(&mut self, ctx: &egui::Context) {
        let Some(updating) = self.updating.as_mut() else {
            return;
        };
        match updating.run.stage().clone() {
            Stage::Restart(target) => {
                let repo = self.repo.as_ref().map(|r| r.path.clone());
                match self_update::relaunch(&target, repo.as_deref()) {
                    // Closing the main window saves the settings.
                    Ok(()) => ctx
                        .send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close),
                    Err(e) => {
                        self.status = Some(Note::Error(format!(
                            "parterre is updated, but couldn't start again: {e}. Start it again \
                             to use the new version."
                        )));
                    }
                }
                self.updating = None;
                self.release_dialog = None;
            }
            Stage::Cancelled => self.updating = None,
            Stage::Failed(_) | Stage::Installed { .. } if self.release_dialog.is_none() => {
                self.release_dialog = Some(updating.update.clone());
            }
            _ => {}
        }
    }

    /// The toolbar's button for a newer release, just right of the find field, and nothing
    /// without one (#433).
    pub(super) fn update_button(&mut self, ui: &mut egui::Ui) {
        let Some(update) = self.newer_release() else {
            return;
        };
        let next = self.next_step(&update);
        let accent = widgets::tones(ui).accent;
        let response = match next {
            Next::Busy => {
                let (rect, response) = ui.allocate_exact_size(
                    egui::Vec2::splat(widgets::BUTTON_SIZE),
                    egui::Sense::hover(),
                );
                ui.put(
                    egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(16.0)),
                    egui::Spinner::new().size(16.0),
                );
                response
            }
            Next::Wait { done, size } => {
                let progress = size.map_or(0.0, |size| done as f32 / size.max(1) as f32);
                widgets::progress_button(ui, glyphs::DOWNLOAD, accent, Some(progress))
            }
            Next::Install => widgets::progress_button(ui, glyphs::RESTART, accent, None),
            Next::Show { failed: true } => {
                let error = ui.visuals().error_fg_color;
                widgets::progress_button(ui, glyphs::WARNING, error, None)
            }
            Next::Show { failed: false } => {
                widgets::progress_button(ui, glyphs::CHECK, accent, None)
            }
            Next::Download | Next::Fetch => {
                widgets::progress_button(ui, glyphs::DOWNLOAD, accent, None)
            }
        };
        let (title, body) = button_tip(next, &update.version);
        if widgets::tip_explained(response, &title, "", &body).clicked() {
            self.get_update(ui.ctx(), &update);
        }
    }

    pub(super) fn newer_release(&mut self) -> Option<Update> {
        if !self.settings.check_for_updates {
            return None;
        }
        match &self.automation.canned_newer_release {
            Some(update) => Some(update.clone()),
            None => self.update_check.as_mut()?.1.newer().cloned(),
        }
    }

    pub(super) fn download(&mut self, ctx: &egui::Context, download: &Download) {
        crate::usage::action(crate::usage::Action::DownloadUpdate);
        match download {
            Download::Install(install) => {
                if let Err(e) = crate::browser::open(&install.url) {
                    self.status = Some(Note::Error(e));
                }
            }
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

/// *Update now* under way, with the release it installs.
#[derive(Debug)]
pub(super) struct Updating {
    update: Update,
    run: self_update::Updating,
}

/// What an update's next click does (#433).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Next {
    /// *Download* as before, where parterre doesn't install it itself.
    Download,
    /// Start the download.
    Fetch,
    /// Nothing while it downloads.
    Wait {
        done: u64,
        size: Option<u64>,
    },
    Install,
    /// Nothing while it installs.
    Busy,
    /// Open the dialog with how it ended.
    Show {
        failed: bool,
    },
}

impl Next {
    /// The menu's entry for `version`.
    pub(super) fn menu_label(self, version: &str) -> String {
        match self {
            Next::Download | Next::Fetch => format!("Download {version}"),
            Next::Wait { .. } => format!("Downloading {version}…"),
            Next::Install => format!("Restart to update to {version}"),
            Next::Busy => format!("Installing {version}…"),
            Next::Show { .. } => format!("Update to {version}…"),
        }
    }
}

/// The toolbar button's tooltip: what it is at, and what a click does.
fn button_tip(next: Next, version: &str) -> (String, String) {
    let out = format!("parterre {version} is out");
    match next {
        Next::Download => (out, "Click to download it.".to_owned()),
        Next::Fetch => (
            out,
            "Click to download it. Once it's downloaded, a click installs it and restarts \
             parterre."
                .to_owned(),
        ),
        Next::Wait { done, size } => (
            format!("Downloading parterre {version}"),
            self_update::progress_text(done, size),
        ),
        Next::Install => (
            format!("Restart to update to {version}"),
            "Downloaded. Click to install it and restart parterre.".to_owned(),
        ),
        Next::Busy => (
            format!("Installing parterre {version}"),
            "parterre restarts when it's done.".to_owned(),
        ),
        Next::Show { failed: true } => (
            format!("The update to {version} failed"),
            "Click to see why.".to_owned(),
        ),
        Next::Show { failed: false } => (
            format!("parterre {version} is installed"),
            "Click for more.".to_owned(),
        ),
    }
}

/// The file `update` installs and how, where parterre installs it itself.
pub(super) fn installs(update: &Update) -> Option<(&Install, Method)> {
    match &update.download {
        Download::Install(install) => Some((install, Method::of(Channel::current())?)),
        _ => None,
    }
}

fn is_disk_image(file: &std::path::Path) -> bool {
    file.extension().is_some_and(|e| e == "dmg")
}

/// Where the update is, under the dialog's fields.
fn stage_note(ui: &mut egui::Ui, stage: &Stage, method: Option<Method>) {
    match stage {
        Stage::Downloading { done, size } => {
            ui.label(format!(
                "Downloading… {}",
                self_update::progress_text(*done, *size)
            ));
            let fraction = size.map_or(0.0, |size| *done as f32 / size.max(1) as f32);
            ui.add(egui::ProgressBar::new(fraction).desired_height(6.0));
        }
        Stage::Installing | Stage::Restart(_) => {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.label(match method {
                    Some(Method::Msi { .. }) => {
                        "Installing… parterre closes, and opens again when it's done."
                    }
                    _ => "Installing…",
                });
            });
        }
        Stage::Downloaded => {
            ui.label("Downloaded. Restart to update when it suits you.");
        }
        Stage::Installed { reboot: false } => {
            ui.label("Installed. Start parterre again to use it.");
        }
        Stage::Installed { reboot: true } => {
            ui.label("Installed. Restart Windows to finish.");
        }
        Stage::Failed(failure) => {
            ui.colored_label(ui.visuals().error_fg_color, &failure.message);
        }
        Stage::Cancelled => {}
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
    Change,
    Next,
    Fallback,
}

#[cfg(test)]
mod tests {
    use super::{RELEASE_TOLD_KEY, builds, dialog_title, release_told, running_version, tell_once};
    use eframe::Storage as _;
    use parterre_telemetry::{Ring, Update};

    #[test]
    fn the_dialog_names_the_build_and_what_the_installation_hears_of() {
        let title = |tag| dialog_title(Update::to(tag).unwrap().ring);
        assert_eq!(title("v1.3.6"), "New release");
        assert_eq!(title("1.3.6-rc"), "New release candidate");
        assert_eq!(title("1.3.6-nightly.20261017"), "New nightly build");
        assert_eq!(title("1.3.6-canary"), "New canary build");
        let heard = Ring::ALL.map(builds);
        assert_eq!(
            heard,
            [
                "stable releases",
                "release candidates",
                "nightly builds",
                "canary builds"
            ]
        );
    }

    #[test]
    fn the_running_version_is_told_without_its_commit() {
        assert_eq!(
            running_version("1.3.4-nightly.20261016+a1b2c3d"),
            "1.3.4-nightly.20261016"
        );
        assert_eq!(running_version("1.2.3-dev+a1b2c3d.dirty"), "1.2.3-dev");
        assert_eq!(running_version("1.2.3"), "1.2.3");
    }

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
