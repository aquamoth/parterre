//! Keeping branches and their remotes in step (#181): *Pull*, *Push* and *Set upstream…* in the
//! node menu, the question before pulling a diverged branch, the upstream dialog, and the
//! window with git's output while a fetch, pull or push runs.

use eframe::egui::{self, Align, Layout, RichText, Ui, ViewportId, vec2};
use parterre_core::branches::{Action, Catalog, command_text};
use parterre_core::remote::{self, Diverged, Live, Pull, Push, PushState, Reconcile, SetUpstream};
use parterre_core::{Oid, Repo};

use super::branches::{Request, Target, capitalized, loading_reason, target_menu_named};
use crate::{dialogs, menu, widgets};

/// *Pull* on the open worktree's branch, *Push* and *Set upstream…* for the local branches at
/// `commit`, after a separator; nothing without a remote.
pub(super) fn section(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    catalog: &Catalog,
    busy: bool,
) -> Option<Request> {
    let locals: Vec<_> = catalog.locals.iter().filter(|b| b.tip == commit).collect();
    if locals.is_empty() || catalog.remote_names.is_empty() {
        return None;
    }
    let mut request = None;
    menu::separator(ui);
    if let Some(branch) = remote::pull_offered(catalog, commit) {
        let stuck = catalog.stuck().map(|s| s.reason());
        let upstream = branch.upstream.clone().unwrap_or_default();
        let response = ui
            .add_enabled(
                !busy && stuck.is_none(),
                egui::Button::new(format!("Pull {}", branch.name)),
            )
            .on_hover_text(format!("From {upstream}"));
        let response = match stuck {
            Some(reason) => response.on_disabled_hover_text(capitalized(&reason)),
            None => response.on_disabled_hover_text(loading_reason(true)),
        };
        if response.clicked() {
            request = Some(Request::Run(Action::Pull(Box::new(Pull {
                branch: branch.name.clone(),
                head: commit,
                how: None,
            }))));
            ui.close();
        }
    }
    // Each branch's remotes: one asking for a force push ends in "…", for its question.
    let pushes: Vec<(String, Vec<Target>)> = locals
        .iter()
        .map(|b| {
            let targets = remote::push_targets(repo, catalog, &b.name)
                .into_iter()
                .map(|(name, state)| {
                    let push = Request::Run(Action::Push(Box::new(Push {
                        branch: b.name.clone(),
                        tip: b.tip,
                        remote: name.clone(),
                    })));
                    match state {
                        PushState::Force => (format!("{name}…"), push, None),
                        PushState::UpToDate => (name, push, Some("Up to date".to_owned())),
                        PushState::New | PushState::Ahead => (name, push, None),
                    }
                })
                .collect();
            (b.name.clone(), targets)
        })
        .collect();
    let dots = |target: &str| if target.ends_with('…') { "…" } else { "" };
    match pushes.as_slice() {
        [(name, targets)] => target_menu_named(
            ui,
            &format!("Push {name}"),
            |target| format!("Push {name} to {target}"),
            targets,
            busy,
            &mut request,
        ),
        many => menu::plain_submenu(ui, "Push", |ui| {
            for (name, targets) in many {
                let single = |target: &str| format!("{name}{}", dots(target));
                target_menu_named(ui, name, single, targets, busy, &mut request);
            }
        }),
    }
    let upstreams: Vec<Target> = locals
        .iter()
        .map(|b| {
            let request = Request::SetUpstream {
                branch: b.name.clone(),
            };
            (format!("{}…", b.name), request, None)
        })
        .collect();
    target_menu_named(
        ui,
        "Set upstream",
        |name| format!("Set upstream of {name}"),
        &upstreams,
        busy,
        &mut request,
    );
    request
}

/// While a fetch, pull or push runs: the commands, git's output as it comes, and *Cancel*.
/// Locks the other windows. Returns true when cancelled.
pub(super) fn network_window(
    ctx: &egui::Context,
    title: &str,
    live: &Live,
    opener: ViewportId,
) -> bool {
    let steps = live.steps();
    // git's output comes from another thread.
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    let shown = dialogs::Dialog::new("network-operation", title)
        .screen(crate::usage::Screen::Network)
        .width(560.0)
        .opener(opener)
        .modal()
        .undismissable()
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("network-output")
                .max_height(300.0)
                .stick_to_bottom(true)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (command, output) in &steps {
                        ui.label(RichText::new(command).monospace().strong());
                        let output = output.trim_end();
                        if !output.is_empty() {
                            ui.add(egui::Label::new(RichText::new(output).monospace()).wrap());
                        }
                    }
                });
            ui.separator();
            let mut cancel = false;
            let size = vec2(ui.available_width(), 34.0);
            ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
                cancel = widgets::text_button(ui, "Cancel").clicked();
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| ui.spinner());
            });
            cancel
        });
    shown.inner
}

/// How to pull a branch that has diverged from its upstream, when git's config doesn't say:
/// merge or rebase, as `git pull` itself asks. Remembers nothing.
#[derive(Debug)]
pub struct PullDialog {
    pub diverged: Diverged,
    how: Reconcile,
    pub opener: ViewportId,
    fresh: bool,
}

impl PullDialog {
    pub fn new(diverged: Diverged, opener: ViewportId) -> Self {
        PullDialog {
            diverged,
            how: Reconcile::Merge,
            opener,
            fresh: true,
        }
    }

    pub fn pull(&self) -> Pull {
        self.diverged.pull(self.how)
    }

    /// `busy` while another Git operation runs.
    pub fn show(&mut self, ctx: &egui::Context, busy: bool) -> dialogs::Answer {
        let d = &self.diverged;
        let title = format!("Pull {} into {}", d.upstream, d.pull.branch);
        let shown = dialogs::Dialog::new("pull-diverged", &title)
            .screen(crate::usage::Screen::PullDiverged)
            .width(460.0)
            .opener(self.opener)
            .raise(self.fresh)
            .show(ctx, |ui| {
                let d = &self.diverged;
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{} has diverged from {}",
                        d.pull.branch, d.upstream
                    ));
                    crate::upstreams::counts_ui(ui, d.ahead, d.behind);
                });
                for how in Reconcile::ALL {
                    let flag = command_text(&d.command(how));
                    if ui
                        .radio(self.how == how, how.name())
                        .on_hover_text(flag)
                        .clicked()
                    {
                        self.how = how;
                    }
                }
                dialogs::command_box(ui, &[command_text(&d.command(self.how))]);
                dialogs::actions(ui, "Pull", !busy, false, false)
            });
        self.fresh = false;
        if shown.should_close() {
            dialogs::Answer::Cancel
        } else {
            shown.inner
        }
    }
}

/// A local branch's upstream: one of the remote-tracking branches, or none.
#[derive(Debug)]
pub struct SetUpstreamDialog {
    pub branch: String,
    upstream: Option<String>,
    /// What it is now.
    current: Option<String>,
    choices: Vec<String>,
    pub opener: ViewportId,
    fresh: bool,
}

impl SetUpstreamDialog {
    /// Its upstream to start with: the one it has, else its own name on the first remote that
    /// has it.
    pub fn new(catalog: &Catalog, branch: String, opener: ViewportId) -> Self {
        let current = catalog
            .locals
            .iter()
            .find(|b| b.name == branch)
            .and_then(|b| b.upstream.clone());
        let choices: Vec<String> = catalog
            .remotes
            .iter()
            .map(|r| r.name.clone())
            .filter(|n| !n.ends_with("/HEAD"))
            .collect();
        let mut remotes = catalog.remote_names.clone();
        remotes.sort();
        let own = remotes
            .iter()
            .map(|r| format!("{r}/{branch}"))
            .find(|n| choices.contains(n));
        SetUpstreamDialog {
            upstream: current.clone().or(own),
            current,
            choices,
            branch,
            opener,
            fresh: true,
        }
    }

    /// The upstream chosen, once one is.
    fn set(&self) -> Option<SetUpstream> {
        Some(SetUpstream {
            branch: self.branch.clone(),
            upstream: self.upstream.clone()?,
        })
    }

    pub fn action(&self) -> Option<Action> {
        self.set().map(|set| Action::SetUpstream(Box::new(set)))
    }

    /// `busy` while another Git operation runs.
    pub fn show(&mut self, ctx: &egui::Context, busy: bool) -> dialogs::Answer {
        let title = format!("Set upstream of {}", self.branch);
        let shown = dialogs::Dialog::new("set-upstream", &title)
            .screen(crate::usage::Screen::SetUpstream)
            .width(420.0)
            .opener(self.opener)
            .raise(self.fresh)
            .show(ctx, |ui| {
                dialogs::fields(ui, |ui| {
                    ui.label("Upstream");
                    dialogs::choice(
                        ui,
                        "set-upstream-choice",
                        &mut self.upstream,
                        "None",
                        &self.choices,
                    );
                    // An upstream pruned from the remote isn't among the choices.
                    if let Some(current) = &self.current
                        && !self.choices.contains(current)
                    {
                        ui.weak(format!("Now {current}, which is gone."));
                    }
                });
                let set = self.set();
                let commands: Vec<String> = set
                    .iter()
                    .map(|set| command_text(&remote::set_upstream_command(set)))
                    .collect();
                dialogs::command_box(ui, &commands);
                let changed = set.is_some() && self.upstream != self.current;
                dialogs::actions(ui, "Set", changed && !busy, false, false)
            });
        self.fresh = false;
        if shown.should_close() {
            dialogs::Answer::Cancel
        } else {
            shown.inner
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use eframe::egui;
    use parterre_core::branches::Action;
    use parterre_core::remote::{Live, Pull, Push};

    use super::super::branches::Request;
    use super::super::tool_harness::{Harness, collect, git, load, menu, write};

    /// A bare `origin`, and a clone of it on `main` at one commit, tracking `origin/main`.
    fn cloned() -> (tempfile::TempDir, tempfile::TempDir) {
        let origin = tempfile::tempdir().unwrap();
        git(origin.path(), &["init", "-q", "--bare", "-b", "main"]);
        let work = tempfile::tempdir().unwrap();
        let p = work.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.name", "Test"]);
        git(p, &["config", "user.email", "test@example.com"]);
        write(p, "base", "base\n");
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "Base"]);
        git(
            p,
            &["remote", "add", "origin", origin.path().to_str().unwrap()],
        );
        git(p, &["push", "-q", "-u", "origin", "main"]);
        (origin, work)
    }

    /// Commits `file` on `main` in a clone of its own, pushed to `origin`.
    fn pushed_elsewhere(origin: &Path, file: &str) -> String {
        let other = tempfile::tempdir().unwrap();
        let p = other.path();
        git(p, &["clone", "-q", origin.to_str().unwrap(), "."]);
        git(p, &["config", "user.name", "Other"]);
        git(p, &["config", "user.email", "other@example.com"]);
        write(p, file, "theirs\n");
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", &format!("Other {file}")]);
        git(p, &["push", "-q", "origin", "main"]);
        git(p, &["rev-parse", "HEAD"])
    }

    fn texts_of(p: &Path, commit: &str) -> (Vec<String>, Option<Request>) {
        let (repo, catalog) = load(p);
        let oid = repo.commit(repo.resolve(commit).unwrap()).oid;
        menu(|ui| super::section(ui, &repo, oid, &catalog, false), None)
    }

    #[test]
    fn the_open_worktrees_branch_pulls_and_pushes_from_its_node() {
        let (_origin, work) = cloned();
        let p = work.path();
        let (texts, _) = texts_of(p, "main");
        for item in ["Pull main", "Push main to origin", "Set upstream of main…"] {
            assert!(texts.iter().any(|t| t == item), "{item}: {texts:?}");
        }
        // Up to date: greyed out, so clicking asks for nothing.
        let (repo, catalog) = load(p);
        let head = repo.commit(repo.resolve("main").unwrap()).oid;
        let (_, asked) = menu(
            |ui| super::section(ui, &repo, head, &catalog, false),
            Some("Push main to origin"),
        );
        assert!(asked.is_none());

        git(p, &["commit", "-q", "--allow-empty", "-m", "Mine"]);
        let (repo, catalog) = load(p);
        let head = repo.commit(repo.resolve("main").unwrap()).oid;
        let (_, asked) = menu(
            |ui| super::section(ui, &repo, head, &catalog, false),
            Some("Push main to origin"),
        );
        let Some(Request::Run(Action::Push(push))) = asked else {
            panic!("a push: {asked:?}")
        };
        assert_eq!(
            (push.branch.as_str(), push.remote.as_str()),
            ("main", "origin")
        );
    }

    #[test]
    fn a_branch_with_no_upstream_or_remote_has_no_pull() {
        let (_origin, work) = cloned();
        let p = work.path();
        git(p, &["switch", "-q", "-c", "feature"]);
        git(p, &["commit", "-q", "--allow-empty", "-m", "Feature"]);
        let (texts, _) = texts_of(p, "feature");
        assert!(!texts.iter().any(|t| t.starts_with("Pull")), "{texts:?}");
        assert!(
            texts.iter().any(|t| t == "Push feature to origin"),
            "{texts:?}"
        );
        git(p, &["remote", "remove", "origin"]);
        let (texts, _) = texts_of(p, "feature");
        assert!(texts.is_empty(), "{texts:?}");
    }

    #[test]
    fn a_push_that_needs_a_force_says_so_with_an_ellipsis() {
        let (origin, work) = cloned();
        let p = work.path();
        pushed_elsewhere(origin.path(), "theirs");
        git(p, &["fetch", "-q", "origin"]);
        git(p, &["commit", "-q", "--allow-empty", "-m", "Mine"]);
        let (texts, _) = texts_of(p, "main");
        assert!(
            texts.iter().any(|t| t == "Push main to origin…"),
            "{texts:?}"
        );
    }

    #[test]
    fn fetching_shows_a_notification_once_done() {
        let (origin, work) = cloned();
        let theirs = pushed_elsewhere(origin.path(), "theirs");
        let mut h = Harness::new(work);
        h.until("the catalogue loads", |h| h.tool.catalog.is_some());
        assert_eq!(h.tool.fetch_blocked(), None);
        let ctx = h.ctx.clone();
        h.tool
            .request(&ctx, Request::Run(Action::Fetch), egui::ViewportId::ROOT);
        h.until("fetched", |h| h.tool.reload.is_some() && h.shows("Fetch"));
        assert_eq!(git(h.path(), &["rev-parse", "origin/main"]), theirs);
    }

    #[test]
    fn a_diverged_pull_asks_merge_or_rebase_then_rebases() {
        let (origin, work) = cloned();
        let theirs = pushed_elsewhere(origin.path(), "theirs");
        let p = work.path();
        git(p, &["commit", "-q", "--allow-empty", "-m", "Mine"]);
        let mut h = Harness::new(work);
        h.until("the catalogue loads", |h| h.tool.catalog.is_some());
        let pull = Pull {
            branch: "main".into(),
            head: h.rev("main"),
            how: None,
        };
        let request = Request::Run(Action::Pull(Box::new(pull)));
        h.ask(request, "Pull origin/main into main");
        h.click("Git command");
        assert!(h.shows_part("--no-rebase"), "merge first: {:?}", h.texts);
        h.click("Rebase");
        assert!(h.shows_part("--rebase") && !h.shows_part("--no-rebase"));
        h.click("Pull");
        h.until("rebased", |h| {
            git(h.path(), &["rev-parse", "main~1"]) == theirs
        });
    }

    #[test]
    fn a_rebased_branch_force_pushes_after_a_confirmation() {
        let (origin, work) = cloned();
        let p = work.path();
        git(p, &["switch", "-q", "-c", "feature"]);
        write(p, "feature", "feature\n");
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "Feature"]);
        git(p, &["push", "-q", "-u", "origin", "feature"]);
        git(p, &["commit", "-q", "--amend", "-m", "Feature, reworded"]);
        let mut h = Harness::new(work);
        h.until("the catalogue loads", |h| h.tool.catalog.is_some());
        let push = Push {
            branch: "feature".into(),
            tip: h.rev("feature"),
            remote: "origin".into(),
        };
        h.ask(
            Request::Run(Action::Push(Box::new(push))),
            "Force push feature to origin?",
        );
        assert!(h.shows("1 replaced commit"), "{:?}", h.texts);
        assert!(!h.shows("Force push anyway"));
        h.click("Force push");
        let tip = h.rev("feature").to_hex();
        h.until("pushed", |_| {
            git(origin.path(), &["rev-parse", "refs/heads/feature"]) == tip
        });
    }

    #[test]
    fn setting_an_upstream_offers_the_branchs_own_name() {
        let (_origin, work) = cloned();
        let p = work.path();
        git(p, &["switch", "-q", "-c", "feature"]);
        git(p, &["push", "-q", "origin", "feature"]);
        let mut h = Harness::new(work);
        h.until("the catalogue loads", |h| h.tool.catalog.is_some());
        let request = Request::SetUpstream {
            branch: "feature".into(),
        };
        h.ask(request, "Set upstream of feature");
        assert!(h.shows("origin/feature"), "{:?}", h.texts);
        h.click("Set");
        h.until("set", |h| {
            let format = "--format=%(upstream:short)";
            git(h.path(), &["for-each-ref", format, "refs/heads/feature"]) == "origin/feature"
        });
    }

    #[test]
    fn the_network_window_shows_each_command_with_gits_output() {
        let live = Live::default();
        live.start(&["fetch".into(), "--all".into()]);
        live.push(b"Receiving objects: 50%\rReceiving objects: 100%, done.\n");
        let ctx = egui::Context::default();
        let mut texts = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                super::network_window(ui.ctx(), "Fetch", &live, egui::ViewportId::ROOT);
            });
            output.textures_delta.clear();
            texts.clear();
            for clipped in &output.shapes {
                collect(&clipped.shape, &mut texts);
            }
        }
        let texts: Vec<_> = texts.into_iter().map(|(t, _)| t).collect();
        for text in [
            "git fetch --all",
            "Receiving objects: 100%, done.",
            "Cancel",
        ] {
            assert!(texts.iter().any(|t| t == text), "{text}: {texts:?}");
        }
    }
}
