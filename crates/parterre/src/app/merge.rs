//! Merging into the open worktree's branch: the dialog, which lists the commits coming in as
//! the log window does, and offers the merge methods, the merge commit's message and stashing
//! uncommitted changes.

use std::sync::Arc;

use eframe::egui::{self, Id, RichText, Ui, ViewportId, vec2};
use parterre_core::branches::command_text;
use parterre_core::log::{LogOptions, LogQuery};
use parterre_core::log_graph::LogGraph;
use parterre_core::merge::{self, Method, Preview};
use parterre_core::revgraph::GraphOptions;
use parterre_core::{CommitIx, Oid, Repo};

use super::commit_table::{CommitList, CommitTable, ROW, Row, Select};
use super::log_window::{self, HEADING};
use crate::dialogs;
use crate::theme::Palette;

/// The commits listed at least, however small the window: the table scrolls.
const MIN_ROWS: usize = 3;

/// A commit table's height for `rows` commits: a little more than the rows, for the spacing
/// around its scroll area.
pub fn list_height(rows: usize) -> f32 {
    HEADING + ROW * rows.max(1) as f32 + 12.0
}

/// The width of the methods' names, before their help.
const METHOD_NAME: f32 = 130.0;

pub struct MergeDialog {
    pub preview: Preview,
    repo: Arc<Repo>,
    /// What the command names the target by: a branch's name, or the full hash.
    target: String,
    pub method: Method,
    pub message: String,
    pub stash: bool,
    /// The window it was asked from.
    pub opener: ViewportId,
    fresh: bool,
    /// The commits coming in, as the log lists them.
    commits: Vec<CommitIx>,
    graph: LogGraph,
    refs: Vec<Vec<usize>>,
    list: CommitList,
}

impl std::fmt::Debug for MergeDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergeDialog")
            .field("preview", &self.preview)
            .field("target", &self.target)
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}

/// What the dialog asks for in a frame.
#[derive(Debug, Default)]
pub struct Asked {
    pub answer: Option<dialogs::Answer>,
    /// A commit to show in the log: the target's, or one double-clicked in the list.
    pub log: Option<Oid>,
}

impl MergeDialog {
    pub fn new(preview: Preview, repo: Arc<Repo>, target: String, opener: ViewportId) -> Self {
        let (commits, graph) = match (repo.lookup(&preview.head), repo.lookup(&preview.theirs)) {
            (Some(head), Some(theirs)) => {
                let list = LogQuery::range(&repo, head, theirs).list(&repo, &LogOptions::default());
                let graph = LogGraph::new(&list);
                (list.commits, graph)
            }
            _ => (Vec::new(), LogGraph::default()),
        };
        MergeDialog {
            method: preview.default_method(),
            message: preview.message.clone(),
            stash: preview.auto_stash,
            refs: repo.refs_by_commit(),
            preview,
            repo,
            target,
            opener,
            fresh: true,
            commits,
            graph,
            list: CommitList::default(),
        }
    }

    pub fn merge(&self) -> merge::Merge {
        self.preview
            .merge(self.target.clone(), self.method, self.stash, &self.message)
    }

    /// The target as the dialog names it: a branch, or a short hash.
    fn name(&self) -> String {
        merge::short_target(&self.merge())
    }

    fn blocked(&self) -> Option<String> {
        self.preview
            .blocked(self.method, self.stash, &self.message, &self.name())
    }

    /// `busy` while another Git operation runs.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        busy: bool,
        palette: &Palette,
        options: &GraphOptions,
    ) -> Asked {
        let mut asked = Asked::default();
        let name = self.name();
        let title = format!("Merge {name} into {}", self.preview.branch);
        let width = (ctx.content_rect().width() - 80.0).clamp(420.0, 780.0);
        let shown = dialogs::Dialog::new("merge-branch", &title)
            .width(width)
            .opener(self.opener)
            .raise(self.fresh)
            .resizable()
            .show(ctx, |ui| {
                // Only the commits scroll: everything else stays in view.
                let grow = Id::new("merge-commits-rest");
                if let Some(ix) = self.repo.lookup(&self.preview.theirs)
                    && dialogs::commit_line(ui, self.repo.commit(ix), self.repo.abbrev_len)
                {
                    asked.log = Some(self.preview.theirs);
                }
                let n = self.commits.len();
                let min = list_height(n.min(MIN_ROWS));
                let height = dialogs::growing(ui, grow, list_height(n), min);
                if let Some(oid) = self.commits_table(ui, palette, options, height) {
                    asked.log = Some(oid);
                }
                let after = ui.cursor().top();
                self.methods(ui, &name);
                ui.add_enabled_ui(self.method.commits(), |ui| {
                    ui.label("Message");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.message)
                            .id_salt("merge-message")
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                })
                .response
                .on_disabled_hover_text("A fast-forward makes no commit");
                if self.preview.stashable(self.method) {
                    let how = if self.preview.outgoing.is_some() {
                        "git rebase --autostash"
                    } else {
                        "git merge --autostash"
                    };
                    let help = format!(
                        "Set your uncommitted changes aside first, and put them back \
                         afterwards ({how})."
                    );
                    ui.checkbox(&mut self.stash, "Stash changes")
                        .on_hover_text(help);
                }
                if let Some(why) = self.blocked() {
                    ui.colored_label(ui.visuals().error_fg_color, why);
                }
                let commands: Vec<String> = merge::commands(&self.merge())
                    .iter()
                    .map(|c| command_text(c))
                    .collect();
                dialogs::command_box(ui, &commands);
                let enabled = self.blocked().is_none() && !busy;
                let answer = dialogs::actions(ui, "Merge", enabled, false, false);
                dialogs::grown(ui, grow, after, height, min);
                answer
            });
        self.fresh = false;
        asked.answer = Some(if shown.should_close() {
            dialogs::Answer::Cancel
        } else {
            shown.inner
        });
        asked
    }

    /// The merge methods, each with what it does; those that make no sense here are greyed
    /// out, with why on hover.
    fn methods(&mut self, ui: &mut Ui, name: &str) {
        for &m in self.preview.methods() {
            let unavailable = self.preview.unavailable(m, name);
            ui.add_enabled_ui(unavailable.is_none(), |ui| {
                ui.horizontal_top(|ui| {
                    let radio = ui
                        .allocate_ui_with_layout(
                            vec2(METHOD_NAME, 18.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_min_width(METHOD_NAME);
                                ui.radio(self.method == m, m.name())
                            },
                        )
                        .inner
                        .on_hover_text(m.flag());
                    let help = ui.add(
                        egui::Label::new(
                            RichText::new(m.help(&self.preview.branch, name))
                                .small()
                                .weak(),
                        )
                        .wrap()
                        .sense(egui::Sense::click()),
                    );
                    if radio.clicked() || help.clicked() {
                        self.method = m;
                    }
                })
            })
            .response
            .on_disabled_hover_text(unavailable.unwrap_or_default());
        }
    }

    /// The commits coming in, as the log window lists them, `height` tall. Returns the one
    /// double-clicked.
    fn commits_table(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        options: &GraphOptions,
        height: f32,
    ) -> Option<Oid> {
        let c = log_window::colors(ui);
        let table = CommitTable {
            id: Id::new("merge-commits"),
            rows: self.commits.len(),
            graph: &self.graph,
            abbrev_len: self.repo.abbrev_len,
            palette,
            select: Select::One,
            icons: false,
        };
        let (repo, commits, refs) = (&*self.repo, &self.commits, &self.refs);
        let list = &mut self.list;
        let clicks = ui
            .allocate_ui(vec2(ui.available_width(), height), |ui| {
                ui.set_min_height(height);
                ui.set_max_height(height);
                table.show(
                    ui,
                    &c,
                    list,
                    |i| {
                        let commit = repo.commit(commits[i]);
                        Row {
                            hash: commit.oid.short(repo.abbrev_len),
                            refs: log_window::badges(
                                repo,
                                &refs[commits[i].ix()],
                                Some(commits[i]),
                                options,
                            ),
                            subject: &commit.subject,
                            author: &commit.author_name,
                            author_email: &commit.author_email,
                            date: &commit.author_date,
                            ..Row::default()
                        }
                    },
                    |_, _, _| {},
                    None,
                )
            })
            .inner;
        clicks
            .double_clicked
            .map(|i| self.repo.commit(self.commits[i]).oid)
    }
}

/// The dialog and the menus, driven frame by frame in a headless context against real,
/// disposable repositories: clicked by the text on screen, as a person would.
#[cfg(test)]
mod tests {
    use std::path::Path;

    use parterre_core::Oid;
    use parterre_core::branches::Stuck;

    use super::super::branches::{self, Request};
    use super::super::tool_harness::{Harness, banner_texts, git, load, menu, read, write};

    fn commit(dir: &Path, path: &str, text: &str, message: &str) {
        write(dir, path, text);
        git(dir, &["add", path]);
        git(dir, &["commit", "-q", "-m", message]);
    }

    /// main: base → mine; up: base → one → two. With `diverged` false, main has no commit
    /// of its own, so up is a fast-forward away.
    fn repository(diverged: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        // A merge commits, and parterre's own git reads the identity from the repository,
        // not from the harness's environment: CI has no global one.
        git(p, &["config", "user.name", "Test"]);
        git(p, &["config", "user.email", "test@example.com"]);
        commit(p, "file", "base\n", "base");
        git(p, &["switch", "-q", "-c", "up"]);
        commit(p, "one", "one\n", "their one");
        commit(p, "two", "two\n", "their two");
        git(p, &["switch", "-q", "main"]);
        if diverged {
            commit(p, "mine", "mine\n", "mine");
        }
        dir
    }

    fn open(h: &mut Harness) {
        let request = Request::Merge {
            theirs: h.rev("up"),
            target: "up".into(),
        };
        h.ask(request, "Merge up into main");
    }

    fn rev(dir: &Path, r: &str) -> Oid {
        Oid::from_hex(&git(dir, &["rev-parse", r])).unwrap()
    }

    #[test]
    fn the_dialog_lists_the_commits_and_merge_runs_git() {
        let mut h = Harness::new(repository(true));
        open(&mut h);
        for subject in ["their one", "their two"] {
            assert!(h.shows(subject), "{subject}: {:?}", h.texts);
        }
        assert!(!h.shows("mine"), "only the commits coming in");
        assert!(h.shows("Joins up into main with a merge commit."));
        // Rebasing is for merging the open worktree's branch into another.
        assert!(!h.shows("Rebase and fast-forward"));
        assert!(!h.shows("Stash changes"));
        // Diverged: a fast-forward makes no sense, and clicking it picks nothing.
        h.click("Fast-forward");
        h.click("Git command");
        assert!(
            h.shows_part("git merge --no-ff -m 'Merge branch '\\''up'\\''' up"),
            "{:?}",
            h.texts
        );
        let mine = h.rev("main");
        h.click("Merge");
        h.until("the merge commit", |h| {
            git(h.path(), &["rev-list", "--merges", "--count", "main"]) == "1"
        });
        assert_eq!(rev(h.path(), "main^1"), mine);
        assert_eq!(rev(h.path(), "main^2"), h.rev("up"));
        h.until("the notification", |h| {
            h.shows("Merge up into main") && !h.shows("Cancel")
        });
    }

    /// With more commits than fit, only the list scrolls: the commit line above it, and the
    /// methods, message and buttons below it, stay on screen.
    #[test]
    fn a_long_list_scrolls_alone() {
        let dir = repository(true);
        let p = dir.path();
        git(p, &["switch", "-q", "up"]);
        for i in 0..80 {
            git(
                p,
                &[
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    &format!("their step {i}"),
                ],
            );
        }
        git(p, &["switch", "-q", "main"]);
        let mut h = Harness::new(dir);
        open(&mut h);
        for _ in 0..10 {
            h.frame();
        }
        let screen = 1000.0;
        for text in [
            "their step 79",
            "Merge commit",
            "Message",
            "Cancel",
            "Merge",
        ] {
            let at = h.at(text);
            assert!(at.y < screen, "{text} at {at:?}");
        }
        assert!(!h.shows("their step 0"), "the list scrolls");
    }

    #[test]
    fn a_fast_forward_is_picked_where_it_would_do() {
        let mut h = Harness::new(repository(false));
        open(&mut h);
        h.click("Git command");
        assert!(h.shows("git merge --ff-only up"), "{:?}", h.texts);
        h.click("Merge commit");
        assert!(h.shows_part("git merge --no-ff -m"), "{:?}", h.texts);
        h.click("Fast-forward");
        assert!(h.shows("git merge --ff-only up"), "{:?}", h.texts);
        h.click("Merge");
        h.until("the fast-forward", |h| h.rev("main") == h.rev("up"));
    }

    #[test]
    fn the_message_is_the_one_typed() {
        let mut h = Harness::new(repository(true));
        open(&mut h);
        h.click("Merge branch 'up'");
        h.key(eframe::egui::Key::End);
        h.type_text(", for the release");
        h.click("Merge");
        h.until("the merge commit", |h| {
            git(h.path(), &["rev-list", "--merges", "--count", "main"]) == "1"
        });
        assert_eq!(
            git(h.path(), &["log", "-1", "--format=%B"]),
            "Merge branch 'up', for the release"
        );
    }

    #[test]
    fn uncommitted_changes_wait_for_the_stash_box() {
        let dir = repository(true);
        write(dir.path(), "mine", "edited\n");
        let mut h = Harness::new(dir);
        let tip = h.rev("main");
        open(&mut h);
        assert!(h.shows("Commit or stash your changes first."));
        h.click("Merge");
        for _ in 0..20 {
            h.frame();
        }
        assert_eq!(h.rev("main"), tip, "Merge is greyed out");
        h.click("Stash changes");
        assert!(!h.shows("Commit or stash your changes first."));
        h.click("Git command");
        assert!(
            h.shows_part("git merge --no-ff --autostash -m"),
            "{:?}",
            h.texts
        );
        h.click("Merge");
        h.until("the notification", |h| {
            h.shows("Merge up into main") && !h.shows("Cancel")
        });
        assert_ne!(h.rev("main"), tip);
        assert_eq!(read(h.path(), "mine"), "edited\n");
    }

    #[test]
    fn a_conflict_says_so_in_orange_and_sticks_the_worktree() {
        let dir = repository(true);
        let p = dir.path();
        git(p, &["switch", "-q", "up"]);
        commit(p, "mine", "theirs\n", "their mine");
        git(p, &["switch", "-q", "main"]);
        let mut h = Harness::new(dir);
        let up = h.rev("up");
        open(&mut h);
        h.click("Merge");
        h.until("the orange notice", |h| {
            h.shows("Merge stopped on conflicts in 1 file")
        });
        assert!(h.shows("Finish or abort it with git, or go to another worktree."));

        let (repo, catalog) = load(h.path());
        assert_eq!(catalog.stuck(), Some(Stuck::InProgress("a merge")));
        let texts = banner_texts(h.path());
        let expected = format!(
            "Merging {} into main: 1 conflicted file",
            up.short(repo.abbrev_len.max(7))
        );
        assert!(texts.contains(&expected), "{texts:?}");
        // Merging is greyed out until it's finished with git.
        let node = |click: Option<&str>| {
            let (repo, catalog) = (&repo, &catalog);
            menu(
                move |ui| branches::node_menu(ui, repo, up, Some(catalog), false, false),
                click,
            )
        };
        assert!(node(None).0.contains(&"Merge into main".to_owned()));
        assert!(node(Some("Merge into main")).1.is_none());
    }

    #[test]
    fn a_merge_a_hook_refused_is_not_committed() {
        let dir = repository(true);
        let p = dir.path();
        let hook = p.join(".git/hooks/pre-merge-commit");
        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut h = Harness::new(dir);
        let up = h.rev("up");
        open(&mut h);
        h.click("Merge");
        h.until("the orange notice", |h| h.shows("Merge not committed"));
        let (repo, _) = load(h.path());
        let texts = banner_texts(h.path());
        let expected = format!(
            "Merging {} into main: not committed",
            up.short(repo.abbrev_len.max(7))
        );
        assert!(texts.contains(&expected), "{texts:?}");
    }

    #[test]
    fn double_clicking_a_commit_opens_the_log() {
        let mut h = Harness::new(repository(true));
        open(&mut h);
        let one = h.rev("up~1");
        let at = h.at("their one");
        h.click_at(at, 2);
        let (_, commits, _) = h.tool.log_request.take().expect("the log is asked for");
        assert_eq!(commits, [one]);
    }

    #[test]
    fn the_menus_offer_it_only_where_there_is_something_to_merge() {
        let dir = repository(true);
        let p = dir.path();
        git(p, &["branch", "behind", "main~1"]);
        let (repo, catalog) = load(p);
        let node = |at: &str, click: Option<&str>| {
            let (repo, catalog, commit) = (&repo, &catalog, rev(p, at));
            menu(
                move |ui| branches::node_menu(ui, repo, commit, Some(catalog), false, false),
                click,
            )
        };
        // A branch and its commit: a submenu.
        assert!(node("up", None).0.contains(&"Merge into main".to_owned()));
        // Already in main: nothing to merge.
        assert!(
            !node("behind", None)
                .0
                .iter()
                .any(|t| t.starts_with("Merge") && !t.starts_with("Merge main into"))
        );
        // A commit with no branch on it (as the log's rows have it too): the commit alone.
        let short = rev(p, "up~1").short(repo.abbrev_len.max(7));
        let item = format!("Merge {short} into main…");
        let (texts, asked) = node("up~1", Some(&item));
        assert!(texts.contains(&item), "{texts:?}");
        match asked {
            Some(Request::Merge { theirs, target }) => {
                assert_eq!(theirs, rev(p, "up~1"));
                assert_eq!(target, theirs.to_hex());
            }
            other => panic!("expected a merge: {other:?}"),
        }
    }

    /// The open worktree on `feature` (base → one → two), and `main` (base → mine) checked
    /// out in a linked worktree, in the second folder returned.
    fn pull_request() -> (tempfile::TempDir, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.name", "Test"]);
        git(p, &["config", "user.email", "test@example.com"]);
        commit(p, "file", "base\n", "base");
        git(p, &["switch", "-q", "-c", "feature"]);
        commit(p, "one", "one\n", "my one");
        commit(p, "two", "two\n", "my two");
        git(p, &["switch", "-q", "main"]);
        commit(p, "mine", "mine\n", "main's own");
        git(p, &["switch", "-q", "feature"]);
        let other = tempfile::tempdir().unwrap();
        let main = other.path().join("main");
        git(
            p,
            &["worktree", "add", "-q", main.to_str().unwrap(), "main"],
        );
        (dir, other)
    }

    fn open_into(h: &mut Harness, into: &str) {
        let request = Request::MergeInto { into: into.into() };
        h.ask(request, &format!("Merge feature into {into}"));
    }

    #[test]
    fn merging_the_open_branch_into_another_rebases_it_and_runs_there() {
        let (dir, _other) = pull_request();
        let mut h = Harness::new(dir);
        let mine = h.rev("main");
        open_into(&mut h, "main");
        for subject in ["my one", "my two"] {
            assert!(h.shows(subject), "{subject}: {:?}", h.texts);
        }
        assert!(!h.shows("main's own"), "only the commits going in");
        assert!(h.shows("Replays feature's commits on top of main, then moves main up to them."));
        h.click("Rebase and fast-forward");
        h.click("Git command");
        assert!(h.shows("git rebase main"), "{:?}", h.texts);
        // Long temporary folders (macOS) wrap the line, so not up to its end.
        assert!(h.shows_part(" merge --ff-only "), "{:?}", h.texts);
        h.click("Merge");
        h.until("main moves up to feature", |h| {
            h.rev("main") == h.rev("feature")
        });
        assert_eq!(h.rev("feature~2"), mine);
        h.until("the notification", |h| {
            h.shows("Merge feature into main") && !h.shows("Cancel")
        });
    }

    #[test]
    fn the_stash_box_shows_for_the_rebase_methods_only() {
        let (dir, _other) = pull_request();
        write(dir.path(), "one", "edited\n");
        let mut h = Harness::new(dir);
        open_into(&mut h, "main");
        // A merge commit runs in main's worktree: this one's changes stay as they are.
        assert!(!h.shows("Stash changes"));
        assert!(!h.shows("Commit or stash your changes first."));
        h.click("Semi-linear merge");
        assert!(h.shows("Commit or stash your changes first."));
        h.click("Stash changes");
        assert!(!h.shows("Commit or stash your changes first."));
        h.click("Git command");
        assert!(h.shows("git rebase --autostash main"), "{:?}", h.texts);
    }

    #[test]
    fn the_menus_offer_merging_the_open_branch_into_the_branches_on_a_node() {
        let (dir, _other) = pull_request();
        let p = dir.path();
        git(p, &["branch", "has-it", "feature"]);
        let (repo, catalog) = load(p);
        let node = |at: &str, click: Option<&str>| {
            let (repo, catalog, commit) = (&repo, &catalog, rev(p, at));
            menu(
                move |ui| branches::node_menu(ui, repo, commit, Some(catalog), false, false),
                click,
            )
        };
        let (texts, asked) = node("main", Some("Merge feature into main…"));
        assert!(
            texts.contains(&"Merge into feature".to_owned()),
            "{texts:?}"
        );
        match asked {
            Some(Request::MergeInto { into }) => assert_eq!(into, "main"),
            other => panic!("expected merging into main: {other:?}"),
        }
        // Already has feature's commits.
        assert!(
            !node("has-it", None)
                .0
                .iter()
                .any(|t| t.starts_with("Merge feature into"))
        );
        // Two branches on the node: a submenu.
        git(p, &["branch", "release", "main"]);
        let (repo, catalog) = load(p);
        let (texts, _) = menu(
            |ui| branches::node_menu(ui, &repo, rev(p, "main"), Some(&catalog), false, false),
            None,
        );
        assert!(
            texts.contains(&"Merge feature into".to_owned()),
            "{texts:?}"
        );
    }

    #[test]
    fn merging_into_another_branch_is_greyed_out_while_this_worktree_is_stuck() {
        let (dir, _other) = pull_request();
        let p = dir.path();
        git(p, &["switch", "-q", "-c", "side", "main"]);
        commit(p, "one", "side\n", "side one");
        git(p, &["switch", "-q", "feature"]);
        let out = std::process::Command::new("git")
            .current_dir(p)
            .args(["merge", "-q", "side"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "it conflicts");
        let (repo, catalog) = load(p);
        let item = "Merge feature into main…";
        let (texts, asked) = menu(
            |ui| branches::node_menu(ui, &repo, rev(p, "main"), Some(&catalog), false, false),
            Some(item),
        );
        assert!(texts.contains(&item.to_owned()), "{texts:?}");
        assert!(asked.is_none(), "greyed out");
    }
}
