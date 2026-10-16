//! Whether `origin` is on a forge is asked of git alone, when a repository is opened. The forge
//! itself is asked only while pull requests are shown and signed in (and once more before
//! deleting a remote branch: `app::remote`). A repository's list is kept for a minute (five if
//! it had none) and asked for again only after that, when the repository is opened again,
//! reloaded (F5) or its refs change; a change while it is kept is loaded once it isn't. A push or
//! a fetch that moves `origin`'s branches is asked about once more a minute later, however fresh
//! the list: the pull request is usually opened just after the push, and after the load the push
//! started (#294). Only turning them on always asks. After a failure the wait doubles from 20 s
//! up to 15 min, and the last list stays shown. There is no polling: an idle window asks nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use eframe::egui;
use parterre_core::git::Git;
use parterre_core::glyphs;
use parterre_forge::{self as forge, Forge, ForgeError, Origin, PullRequests};

use crate::{widgets, worker};

/// What a finished load brought, and whether the user asked for it ([`PullRequestLoader::ask`])
/// rather than parterre loading by itself: only then is it worth telling them.
#[derive(Debug)]
pub enum Loaded {
    Found { count: usize, asked: bool },
    Failed { error: ForgeError, asked: bool },
}

/// What is known about one repository's pull requests.
#[derive(Debug)]
struct Entry {
    last_list: Option<Arc<PullRequests>>,
    /// Turning them on asks earlier.
    ask_again_at: Instant,
    failures_in_a_row: u32,
    last_error: Option<String>,
    needs_sign_in: bool,
}

#[derive(Debug)]
struct Job {
    path: PathBuf,
    user_asked: bool,
    rx: Receiver<Result<PullRequests, ForgeError>>,
}

#[derive(Debug, Default)]
pub struct PullRequestLoader {
    shown_repo: Option<PathBuf>,
    origin: Option<Origin>,
    cache: HashMap<PathBuf, Entry>,
    job: Option<Job>,
    /// A list no longer fresh is loaded again: the repository was opened, or its refs changed.
    /// Kept until that load starts.
    due: bool,
    /// When to ask once more, however fresh the list, since a push or a fetch moved `origin`'s
    /// branches ([`forge::RECHECK_AFTER_PUSH`]).
    recheck_at: Option<Instant>,
    load_now: bool,
    user_asked: bool,
    canned_json: Option<Arc<str>>,
}

impl PullRequestLoader {
    /// A loader that shows the pull requests in `json` instead of asking the forge
    /// ([`Origin::load_canned`]).
    pub fn canned(json: &str) -> PullRequestLoader {
        PullRequestLoader {
            canned_json: Some(json.into()),
            ..PullRequestLoader::default()
        }
    }

    pub fn is_canned(&self) -> bool {
        self.canned_json.is_some()
    }

    pub fn follow(&mut self, path: Option<&Path>) {
        if self.shown_repo.as_deref() == path {
            return;
        }
        self.shown_repo = path.map(Path::to_owned);
        self.origin = path.and_then(|p| forge::origin(&Git::new(p)));
        self.due = true;
        self.recheck_at = None;
    }

    /// The forge repository `origin` points at: pull requests can be shown only if there is
    /// one.
    pub fn origin(&self) -> Option<&Origin> {
        self.origin.as_ref()
    }

    fn entry(&self) -> Option<&Entry> {
        self.cache.get(self.shown_repo.as_ref()?)
    }

    pub fn list(&self) -> Option<&Arc<PullRequests>> {
        self.entry()?.last_list.as_ref()
    }

    pub fn error(&self) -> Option<&str> {
        self.entry()?.last_error.as_deref()
    }

    /// The last load failed for want of a sign-in.
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
            self.recheck_at = Some(Instant::now() + forge::RECHECK_AFTER_PUSH);
        }
    }

    /// The user turned pull requests on: load now, and say how it went.
    pub fn ask(&mut self) {
        self.load_now = true;
        self.user_asked = true;
    }

    /// Starts a load if one is due and pull requests are `shown`; returns what a load of the
    /// shown repository that has finished brought.
    pub fn update(&mut self, shown: bool, ctx: &egui::Context) -> Option<Loaded> {
        if shown && self.origin.is_some() && self.job.is_none() {
            let now = Instant::now();
            let (next, failed) = match self.entry() {
                Some(e) => (Some(e.ask_again_at), e.failures_in_a_row > 0),
                None => (None, false),
            };
            let stale = next.is_none_or(|next| now >= next);
            // The recheck doesn't cut the wait after a failure short.
            let recheck_at = self.recheck_at.map(|at| match next {
                Some(next) if failed => at.max(next),
                _ => at,
            });
            let recheck = recheck_at.is_some_and(|at| now >= at);
            if self.load_now || recheck || (self.due && stale) {
                self.load(ctx);
                self.due = false;
                self.load_now = false;
                // The load a push starts right away doesn't stand in for the recheck.
                if recheck {
                    self.recheck_at = None;
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
        let job = self.job.as_ref()?;
        let stopped = || {
            Err(ForgeError::Network(
                self.origin.as_ref().map_or(Forge::Github, Origin::forge),
                "loading them stopped unexpectedly".into(),
            ))
        };
        let result = worker::received(&job.rx, stopped)?;
        let Job {
            path,
            user_asked: asked,
            ..
        } = self.job.take()?;
        let now = Instant::now();
        let entry = self.cache.entry(path.clone()).or_insert(Entry {
            last_list: None,
            ask_again_at: now,
            failures_in_a_row: 0,
            last_error: None,
            needs_sign_in: false,
        });
        let loaded = match result {
            Ok(list) => {
                let count = list.list.len();
                entry.ask_again_at = now + forge::fresh_for(count > 0);
                entry.last_list = Some(Arc::new(list));
                entry.failures_in_a_row = 0;
                entry.last_error = None;
                entry.needs_sign_in = false;
                Loaded::Found { count, asked }
            }
            Err(e) => {
                entry.failures_in_a_row += 1;
                let wait =
                    forge::retry_after(entry.failures_in_a_row).max(e.wait().unwrap_or_default());
                entry.ask_again_at = now + wait;
                entry.last_error = Some(e.to_string());
                entry.needs_sign_in = e.needs_sign_in();
                Loaded::Failed { error: e, asked }
            }
        };
        // Another repository has been opened meanwhile: this one's result waits in the cache.
        (self.shown_repo.as_ref() == Some(&path)).then_some(loaded)
    }

    fn load(&mut self, ctx: &egui::Context) {
        let (Some(path), Some(origin)) = (self.shown_repo.clone(), self.origin.clone()) else {
            return;
        };
        let git = Git::new(&path);
        let canned = self.canned_json.clone();
        let rx = worker::spawn(ctx, move || match canned {
            Some(json) => origin.load_canned(&git, &json),
            None => origin.load(&git),
        });
        self.job = Some(Job {
            path,
            user_asked: std::mem::take(&mut self.user_asked),
            rx,
        });
    }
}

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
    let explanation = error.explain();
    let shown = crate::dialogs::Dialog::new("pull-requests-error", &explanation.title)
        .screen(crate::usage::Screen::PullRequestsError)
        .icon(glyphs::PULL_REQUEST, false)
        .width(380.0)
        .modal()
        .resizable()
        .show(ctx, |ui| {
            let mut answer = DialogAnswer::Open;
            crate::dialogs::fields(ui, |ui| ui.label(&explanation.body));
            if let Some(command) = &explanation.command {
                command_box(ui, command);
            }
            ui.add_space(4.0);
            const INSTALL: &str = "Install the GitHub CLI…";
            let install = matches!(error, ForgeError::NoGh);
            let mut widths = vec![widgets::filled_button_width(ui, "OK", 80.0)];
            if install {
                widths.push(widgets::text_button_width(ui, INSTALL));
            }
            crate::dialogs::button_row(ui, &widths, |ui| {
                if install && widgets::text_button(ui, INSTALL).clicked() {
                    answer = DialogAnswer::Install;
                }
                if widgets::primary_button(ui, "OK", 80.0).clicked() {
                    answer = DialogAnswer::Close;
                }
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter)) {
                answer = DialogAnswer::Close;
            }
            answer
        });
    shown.or_closed(DialogAnswer::Close)
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
                    let id = egui::Id::new("copied").with(command);
                    let done = super::table_style::colors(ui).added;
                    widgets::copy_button(ui, id, || command.to_owned(), "Copy the command", done);
                });
            });
        });
}

/// What the status bar says once `count` open pull requests have loaded from `forge`, of which
/// `here` have their head in the repository.
pub fn loaded_status(forge: Forge, count: usize, here: usize) -> String {
    let open = match count {
        0 => return format!("No open pull requests on {forge}"),
        1 => format!("1 open pull request on {forge}"),
        n => format!("{n} open pull requests on {forge}"),
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
            entry.ask_again_at = Instant::now();
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
        loader.recheck_at = Some(Instant::now());
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
            entry.failures_in_a_row = 3;
        }
        loader.refs_changed(true);
        loader.recheck_at = Some(Instant::now());
        loader.update(true, &ctx);
        assert!(!loader.is_loading());
        expire(&mut loader);
        loader.update(true, &ctx);
        assert!(loader.is_loading());
    }

    #[test]
    fn status_counts_what_can_be_shown() {
        let status = |count, here| loaded_status(Forge::Github, count, here);
        assert_eq!(status(0, 0), "No open pull requests on GitHub");
        assert_eq!(status(1, 1), "1 open pull request on GitHub");
        assert_eq!(
            status(1, 0),
            "1 open pull request on GitHub, on a commit not fetched here"
        );
        assert_eq!(status(5, 5), "5 open pull requests on GitHub");
        assert_eq!(
            status(5, 4),
            "5 open pull requests on GitHub, 1 on a commit not fetched here"
        );
        assert_eq!(
            status(63, 25),
            "63 open pull requests on GitHub, 38 on commits not fetched here"
        );
    }

    #[test]
    fn status_names_azure_devops() {
        assert_eq!(
            loaded_status(Forge::AzureDevops, 0, 0),
            "No open pull requests on Azure DevOps"
        );
        assert_eq!(
            loaded_status(Forge::AzureDevops, 2, 1),
            "2 open pull requests on Azure DevOps, 1 on a commit not fetched here"
        );
    }
}
