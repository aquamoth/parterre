//! Open pull requests from GitHub, loaded on a worker thread while they are shown. Not in
//! TortoiseGit.
//!
//! Whether `origin` is on GitHub is asked of git alone, when a repository is opened. Here,
//! GitHub itself is asked only while pull requests are shown (the question before deleting a
//! remote branch asks once more, shown or not: `app::remote`) and `gh` is signed in, and as t3code
//! does: a repository's list is kept for a minute (five if it had none) and asked for again
//! only after that, when the repository is opened again, reloaded (F5) or its refs change; a
//! change while it is kept is loaded once it isn't. A push or a fetch that moves `origin`'s
//! branches is also asked about once more a minute later, however fresh the list: the pull
//! request is usually opened just after the push, and after the load the push started (#294).
//! Only turning them on always asks: pressing F5 again and again asks no more often. After a failure the wait doubles
//! from 20 s up to 15 min, and the last list stays shown. There is no polling: an idle window
//! asks nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

use eframe::egui;
use parterre_core::git::Git;
use parterre_core::glyphs;
use parterre_forge::github::{self, GithubRepo};
use parterre_forge::{self as forge, ForgeError, PullRequests};

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
    /// When to ask GitHub again, at the earliest (turning them on aside).
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
    /// repository was opened, or its refs changed. Kept until that load starts.
    due: bool,
    /// When to ask once more, however fresh the list, since a push or a fetch moved `origin`'s
    /// branches ([`forge::RECHECK_AFTER_PUSH`]).
    recheck: Option<Instant>,
    /// Load whether or not the list is fresh: pull requests turned on.
    force: bool,
    /// The user asked for pull requests: say how it went.
    asked: bool,
    /// Pull requests to show instead of asking GitHub (`--pull-requests-from`), as JSON.
    canned: Option<Arc<str>>,
}

impl PullRequestLoader {
    /// A loader that shows the pull requests in `json` instead of asking GitHub
    /// ([`github::load_canned`]).
    pub fn canned(json: &str) -> PullRequestLoader {
        PullRequestLoader {
            canned: Some(json.into()),
            ..PullRequestLoader::default()
        }
    }

    /// The pull requests come from a file, not from GitHub.
    pub fn is_canned(&self) -> bool {
        self.canned.is_some()
    }

    /// Follows the repository shown (`None` for none).
    pub fn follow(&mut self, path: Option<&Path>) {
        if self.path.as_deref() == path {
            return;
        }
        self.path = path.map(Path::to_owned);
        self.origin = path.and_then(|p| github::origin(&Git::new(p)));
        self.due = true;
        self.recheck = None;
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

    /// The refs changed: load again once the list is no longer fresh. If `origin`'s branches
    /// moved ([`forge::origin_moved`]), ask once more a little later, fresh or not.
    pub fn refs_changed(&mut self, origin_moved: bool) {
        self.due = true;
        if origin_moved {
            self.recheck = Some(Instant::now() + forge::RECHECK_AFTER_PUSH);
        }
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
            let now = Instant::now();
            let (next, failed) = match self.entry() {
                Some(e) => (Some(e.next), e.failures > 0),
                None => (None, false),
            };
            let stale = next.is_none_or(|next| now >= next);
            // The recheck doesn't cut the wait after a failure short.
            let recheck_at = self.recheck.map(|at| match next {
                Some(next) if failed => at.max(next),
                _ => at,
            });
            let recheck = recheck_at.is_some_and(|at| now >= at);
            if self.force || recheck || (self.due && stale) {
                self.load(ctx);
                self.due = false;
                self.force = false;
                // The load a push starts right away doesn't stand in for the recheck.
                if recheck {
                    self.recheck = None;
                }
            } else if let Some(at) = [recheck_at, next.filter(|_| self.due)]
                .into_iter()
                .flatten()
                .min()
            {
                // An idle window draws no frames: wake up for what is waiting.
                ctx.request_repaint_after(at.saturating_duration_since(now));
            }
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
        let canned = self.canned.clone();
        std::thread::spawn(move || {
            let loaded = match canned {
                Some(json) => github::load_canned(&git, &json),
                None => github::load(&git),
            };
            let _ = tx.send(loaded);
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

/// The dialog saying why pull requests the user turned on can't be shown: a modal over the main
/// window, with the command that helps, if one does, ready to copy.
pub fn dialog(ctx: &egui::Context, error: &ForgeError) -> DialogAnswer {
    use egui::{Align, Layout};

    let explanation = error.explain();
    let shown = crate::dialogs::Dialog::new("pull-requests-error", explanation.title)
        .screen(crate::usage::Screen::PullRequestsError)
        .icon(glyphs::PULL_REQUEST, false)
        .width(380.0)
        .modal()
        .resizable()
        .show(ctx, |ui| {
            let mut answer = DialogAnswer::Open;
            crate::dialogs::fields(ui, |ui| ui.label(&explanation.body));
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
            if ui.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter)) {
                answer = DialogAnswer::Close;
            }
            answer
        });
    if shown.should_close() {
        DialogAnswer::Close
    } else {
        shown.inner
    }
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
                    // A check mark for a moment after a click.
                    let id = egui::Id::new("copied").with(command);
                    let now = ui.input(|i| i.time);
                    let copied_at: Option<f64> = ui.data(|d| d.get_temp(id));
                    let copied = copied_at.is_some_and(|at| now - at < 1.5);
                    if copied {
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(300));
                    }
                    let done = super::log_window::colors(ui).added;
                    if widgets::copy_button(ui, copied, done)
                        .on_hover_text("Copy the command")
                        .clicked()
                    {
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
    use super::*;

    /// A loader of an empty list (as GitHub answers before the pull request is opened), for a
    /// repository whose `origin` is on GitHub, with its first load done.
    fn loaded(dir: &Path, ctx: &egui::Context) -> PullRequestLoader {
        for args in [
            &["init", "-q"][..],
            &["remote", "add", "origin", "https://github.com/o/r.git"],
        ] {
            let ok = std::process::Command::new(parterre_core::git::program())
                .args(args)
                .current_dir(dir)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        }
        let mut loader = PullRequestLoader::canned("[]");
        loader.follow(Some(dir));
        assert!(matches!(
            finish(&mut loader, ctx),
            Loaded::Found { count: 0, .. }
        ));
        loader
    }

    /// Runs `update` until the load it started is in.
    fn finish(loader: &mut PullRequestLoader, ctx: &egui::Context) -> Loaded {
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(loaded) = loader.update(true, ctx) {
                return loaded;
            }
            assert!(Instant::now() < deadline, "the load never finished");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Lets the list's freshness run out.
    fn expire(loader: &mut PullRequestLoader) {
        for entry in loader.cache.values_mut() {
            entry.next = Instant::now();
        }
    }

    #[test]
    fn a_ref_change_while_the_list_is_fresh_is_loaded_later() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut loader = loaded(dir.path(), &ctx);
        // A commit: the empty list is kept for five minutes.
        loader.refs_changed(false);
        assert!(loader.update(true, &ctx).is_none());
        assert!(!loader.is_loading());
        expire(&mut loader);
        loader.update(true, &ctx);
        assert!(loader.is_loading(), "the change was never loaded");
    }

    /// #294: the push of a pull request's branch changes the refs a few seconds before
    /// `gh pr create` opens it, so the load the push starts finds none.
    #[test]
    fn a_push_is_asked_about_again_a_minute_later() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut loader = loaded(dir.path(), &ctx);
        expire(&mut loader);
        // The push: loaded at once, before the pull request is opened.
        loader.refs_changed(true);
        assert!(matches!(
            finish(&mut loader, &ctx),
            Loaded::Found { count: 0, .. }
        ));
        assert!(loader.update(true, &ctx).is_none());
        assert!(!loader.is_loading());
        // A minute later, though the empty list is kept for five.
        loader.recheck = Some(Instant::now());
        loader.update(true, &ctx);
        assert!(loader.is_loading(), "the push was not asked about again");
        finish(&mut loader, &ctx);
        // Once.
        assert!(loader.update(true, &ctx).is_none());
        assert!(!loader.is_loading());
    }

    /// The recheck after a push doesn't cut the wait after failed loads short.
    #[test]
    fn a_push_waits_out_failures() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut loader = loaded(dir.path(), &ctx);
        for entry in loader.cache.values_mut() {
            entry.failures = 3;
        }
        loader.refs_changed(true);
        loader.recheck = Some(Instant::now());
        loader.update(true, &ctx);
        assert!(!loader.is_loading());
        expire(&mut loader);
        loader.update(true, &ctx);
        assert!(loader.is_loading());
    }

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
