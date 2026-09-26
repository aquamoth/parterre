//! Open pull requests from GitHub, loaded on a worker thread while they are shown. Not in
//! TortoiseGit.
//!
//! Whether `origin` is on GitHub is asked of git alone, when a repository is opened. GitHub
//! itself is asked only while pull requests are shown and `gh` is signed in, and as t3code
//! does: a repository's list is kept for a minute (five if it had none) and asked for again
//! only after that, when the repository is opened again or its refs change. F5 and turning
//! them on always ask. After a failure the wait doubles from 20 s up to 15 min, and the last
//! list stays shown. There is no polling.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

use eframe::egui;
use parterre_core::forge::github::{self, GithubRepo};
use parterre_core::forge::{self, ForgeError, PullRequests};
use parterre_core::git::Git;
use parterre_core::glyphs;

use crate::widgets;

/// What a finished load brought, and whether the user asked for it ([`PullRequestLoader::ask`])
/// rather than parterre loading by itself: only then is it worth telling them.
#[derive(Debug)]
pub enum Loaded {
    /// A list of this many pull requests.
    Found {
        count: usize,
        asked: bool,
    },
    Failed {
        error: ForgeError,
        asked: bool,
    },
}

/// What is known about one repository's pull requests.
#[derive(Debug)]
struct Entry {
    /// The last list loaded, kept when a later load fails.
    list: Option<Arc<PullRequests>>,
    /// When to ask GitHub again, at the earliest (F5 aside).
    next: Instant,
    /// Failed loads in a row.
    failures: u32,
    /// Why the last load failed, if it did.
    error: Option<String>,
    /// The last load failed for want of a signed-in `gh`.
    needs_sign_in: bool,
}

/// A load running on a worker thread.
#[derive(Debug)]
struct Job {
    /// The repository it is for.
    path: PathBuf,
    /// Whether the user asked for it.
    asked: bool,
    rx: Receiver<Result<PullRequests, ForgeError>>,
}

#[derive(Debug, Default)]
pub struct PullRequestLoader {
    /// The repository shown.
    path: Option<PathBuf>,
    /// The GitHub repository its `origin` points at, if any.
    origin: Option<GithubRepo>,
    /// Every repository asked about in this run.
    cache: HashMap<PathBuf, Entry>,
    /// The load running.
    job: Option<Job>,
    /// Something happened after which a list that is no longer fresh is loaded again: the
    /// repository was opened, or its refs changed.
    due: bool,
    /// Load whether or not the list is fresh: F5, or pull requests turned on.
    force: bool,
    /// The user asked for pull requests: say how it went.
    asked: bool,
}

impl PullRequestLoader {
    /// Follows the repository shown (`None` for none).
    pub fn follow(&mut self, path: Option<&Path>) {
        if self.path.as_deref() == path {
            return;
        }
        self.path = path.map(Path::to_owned);
        self.origin = path.and_then(|p| github::origin(&Git::new(p)));
        self.due = true;
    }

    /// The GitHub repository `origin` points at: pull requests can be shown only if there is
    /// one.
    pub fn origin(&self) -> Option<&GithubRepo> {
        self.origin.as_ref()
    }

    fn entry(&self) -> Option<&Entry> {
        self.cache.get(self.path.as_ref()?)
    }

    /// The shown repository's last list.
    pub fn list(&self) -> Option<&Arc<PullRequests>> {
        self.entry()?.list.as_ref()
    }

    /// Why the shown repository's last load failed, if it did.
    pub fn error(&self) -> Option<&str> {
        self.entry()?.error.as_deref()
    }

    /// The shown repository's last load failed for want of a signed-in `gh`.
    pub fn needs_sign_in(&self) -> bool {
        self.entry().is_some_and(|e| e.needs_sign_in)
    }

    pub fn is_loading(&self) -> bool {
        self.job.is_some()
    }

    /// The refs changed: load again if the list is no longer fresh.
    pub fn refs_changed(&mut self) {
        self.due = true;
    }

    /// Load again now (F5). The list shown stays until the new one is in.
    pub fn refresh(&mut self) {
        self.force = true;
    }

    /// The user turned pull requests on: load now, and say how it went.
    pub fn ask(&mut self) {
        self.force = true;
        self.asked = true;
    }

    /// Starts a load if one is due and pull requests are `shown`; returns what a load of the
    /// shown repository that has finished brought.
    pub fn update(&mut self, shown: bool, ctx: &egui::Context) -> Option<Loaded> {
        if shown && self.origin.is_some() && self.job.is_none() {
            let stale = self.entry().is_none_or(|e| Instant::now() >= e.next);
            if self.force || (self.due && stale) {
                self.load(ctx);
            }
            self.due = false;
            self.force = false;
        }
        let Job { path, asked, rx } = self.job.as_ref()?;
        let asked = *asked;
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(ForgeError::Network(
                "loading them stopped unexpectedly".into(),
            )),
        };
        let path = path.clone();
        self.job = None;
        let now = Instant::now();
        let entry = self.cache.entry(path.clone()).or_insert(Entry {
            list: None,
            next: now,
            failures: 0,
            error: None,
            needs_sign_in: false,
        });
        let loaded = match result {
            Ok(list) => {
                let count = list.list.len();
                entry.next = now + forge::fresh_for(count > 0);
                entry.list = Some(Arc::new(list));
                entry.failures = 0;
                entry.error = None;
                entry.needs_sign_in = false;
                Loaded::Found { count, asked }
            }
            Err(e) => {
                entry.failures += 1;
                let wait = forge::retry_after(entry.failures).max(e.wait().unwrap_or_default());
                entry.next = now + wait;
                entry.error = Some(e.to_string());
                entry.needs_sign_in = e.needs_sign_in();
                Loaded::Failed { error: e, asked }
            }
        };
        // Another repository has been opened meanwhile: this one's result waits in the cache.
        (self.path.as_ref() == Some(&path)).then_some(loaded)
    }

    fn load(&mut self, ctx: &egui::Context) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let git = Git::new(&path);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(github::load(&git));
            ctx.request_repaint();
        });
        self.job = Some(Job {
            path,
            asked: std::mem::take(&mut self.asked),
            rx,
        });
    }
}

/// What the user did with the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogAnswer {
    Open,
    Close,
    /// Asked for the GitHub CLI's installation page.
    Install,
}

/// The dialog saying why pull requests the user turned on can't be shown: a modal in the
/// look of the menus and popovers, with the command that helps, if one does, ready to copy.
pub fn dialog(ctx: &egui::Context, error: &ForgeError) -> DialogAnswer {
    use egui::{Align, Frame, Layout, Margin, RichText, Vec2};

    let explanation = error.explain();
    let style = {
        let mut style = (*ctx.global_style()).clone();
        crate::menu::popover_style(&mut style);
        style
    };
    let frame = Frame::popup(&style)
        .inner_margin(Margin::same(20))
        .corner_radius(12);
    let mut answer = DialogAnswer::Open;
    let modal = egui::Modal::new(egui::Id::new("pull-requests-error"))
        .frame(frame)
        .backdrop_color(egui::Color32::from_black_alpha(
            if style.visuals.dark_mode { 90 } else { 40 },
        ))
        .show(ctx, |ui| {
            ui.set_style(style.clone());
            ui.set_width(380.0);
            ui.spacing_mut().item_spacing.y = 10.0;
            let t = widgets::tones(ui);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (badge, _) = ui.allocate_exact_size(Vec2::splat(32.0), egui::Sense::hover());
                ui.painter().circle_filled(badge.center(), 16.0, t.on_bg);
                let icon = egui::Rect::from_center_size(badge.center(), Vec2::splat(18.0));
                widgets::paint_glyph(ui.painter(), icon, glyphs::PULL_REQUEST, t.on_fg);
                ui.label(RichText::new(explanation.title).size(16.0).strong());
            });
            ui.label(&explanation.body);
            if let Some(command) = explanation.command {
                command_box(ui, command);
            }
            ui.add_space(4.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if widgets::primary_button(ui, "OK", 80.0).clicked() {
                    answer = DialogAnswer::Close;
                }
                if matches!(error, ForgeError::NoGh)
                    && widgets::text_button(ui, "Install the GitHub CLI…").clicked()
                {
                    answer = DialogAnswer::Install;
                }
            });
        });
    let keys = ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter));
    if modal.should_close() || keys {
        answer = DialogAnswer::Close;
    }
    answer
}

/// A command to run in a terminal, in a field of its own, with a button copying it.
fn command_box(ui: &mut egui::Ui, command: &str) {
    let t = widgets::tones(ui);
    egui::Frame::new()
        .fill(t.seg_bg)
        .corner_radius(8)
        .inner_margin(egui::Margin {
            left: 12,
            right: 4,
            top: 4,
            bottom: 4,
        })
        .show(ui, |ui| {
            // One row as high as the button, the command centred beside it.
            let row = egui::vec2(ui.available_width(), 28.0);
            let layout = egui::Layout::left_to_right(egui::Align::Center);
            ui.allocate_ui_with_layout(row, layout, |ui| {
                ui.label(egui::RichText::new(command).monospace().size(14.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // "Copied" for a moment after a click.
                    let id = egui::Id::new("copied").with(command);
                    let now = ui.input(|i| i.time);
                    let copied_at: Option<f64> = ui.data(|d| d.get_temp(id));
                    let copied = copied_at.is_some_and(|at| now - at < 1.5);
                    if copied {
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(300));
                    }
                    let label = if copied { "Copied" } else { "Copy" };
                    if widgets::text_button(ui, label).clicked() {
                        ui.ctx().copy_text(command.to_owned());
                        ui.data_mut(|d| d.insert_temp(id, now));
                    }
                });
            });
        });
}

/// What the status bar says once `count` open pull requests have loaded, of which `here` have
/// their head in the repository.
pub fn loaded_status(count: usize, here: usize) -> String {
    let open = match count {
        0 => return "No open pull requests on GitHub".to_owned(),
        1 => "1 open pull request on GitHub".to_owned(),
        n => format!("{n} open pull requests on GitHub"),
    };
    match (count, count.saturating_sub(here)) {
        (_, 0) => open,
        (1, _) => format!("{open}, on a commit not fetched here"),
        (_, 1) => format!("{open}, 1 on a commit not fetched here"),
        (_, missing) => format!("{open}, {missing} on commits not fetched here"),
    }
}

#[cfg(test)]
mod tests {
    use super::loaded_status;

    #[test]
    fn status_counts_what_can_be_shown() {
        assert_eq!(loaded_status(0, 0), "No open pull requests on GitHub");
        assert_eq!(loaded_status(1, 1), "1 open pull request on GitHub");
        assert_eq!(
            loaded_status(1, 0),
            "1 open pull request on GitHub, on a commit not fetched here"
        );
        assert_eq!(loaded_status(5, 5), "5 open pull requests on GitHub");
        assert_eq!(
            loaded_status(5, 4),
            "5 open pull requests on GitHub, 1 on a commit not fetched here"
        );
        assert_eq!(
            loaded_status(63, 25),
            "63 open pull requests on GitHub, 38 on commits not fetched here"
        );
    }
}
