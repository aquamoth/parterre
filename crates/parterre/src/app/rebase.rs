//! Rebasing the open worktree's branch: the confirmation, which lists the commits being
//! rebased as the log window does, greying those git leaves out, with what to do with each
//! (pick, squash or drop: for a selection of them at once, or for one by clicking its icon),
//! and the banner across the graph while the open worktree is stuck with an operation in
//! progress, such as a rebase, a merge or a cherry-pick.

use std::sync::Arc;

use eframe::egui::{self, Color32, Id, RichText, Ui, ViewportId, vec2};
use parterre_core::branches::{Catalog, Stuck, command_text};
use parterre_core::glyphs;
use parterre_core::log::{LogOptions, LogQuery};
use parterre_core::log_graph::LogGraph;
use parterre_core::rebase::{self, Preview, Todo};
use parterre_core::revgraph::GraphOptions;
use parterre_core::{CommitIx, Oid, Repo};

use super::commit_table::{CommitList, CommitTable, Row, Select};
use super::log_window;
use super::merge::list_height;
use crate::dialogs;
use crate::theme::Palette;

/// What the banner says to do about an operation in progress.
const FINISH: &str = "Finish or abort it with git, or go to another worktree.";

/// The commits listed at least, however small the window: the table scrolls.
const MIN_ROWS: usize = 3;

/// The colour of a worktree stuck with an operation in progress, as its zigzag edge in the
/// graph is drawn.
pub fn stuck_color(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(240, 160, 60)
    } else {
        Color32::from_rgb(170, 90, 0)
    }
}

/// The icon of a todo command, and its colour.
fn todo_icon(ui: &Ui, todo: Todo) -> (glyphs::Glyph, Color32) {
    match todo {
        Todo::Pick => (glyphs::PICK, ui.visuals().text_color()),
        Todo::Squash => (glyphs::SQUASH, crate::widgets::tones(ui).accent),
        Todo::Drop => (glyphs::DROP, ui.visuals().error_fg_color),
    }
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

pub struct RebaseDialog {
    pub preview: Preview,
    repo: Arc<Repo>,
    /// What the command names the target by: a branch's name, or the full hash.
    target: String,
    pub stash: bool,
    /// The window it was asked from.
    pub opener: ViewportId,
    fresh: bool,
    /// The commits on the branch and not in the target, as the log lists them.
    commits: Vec<CommitIx>,
    graph: LogGraph,
    refs: Vec<Vec<usize>>,
    list: CommitList,
}

impl std::fmt::Debug for RebaseDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RebaseDialog")
            .field("preview", &self.preview)
            .field("target", &self.target)
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

impl RebaseDialog {
    pub fn new(preview: Preview, repo: Arc<Repo>, target: String, opener: ViewportId) -> Self {
        let (commits, graph) = match (repo.lookup(&preview.onto), repo.lookup(&preview.head)) {
            (Some(onto), Some(head)) => {
                let list = LogQuery::range(&repo, onto, head).list(&repo, &LogOptions::default());
                let graph = LogGraph::new(&list);
                (list.commits, graph)
            }
            _ => (Vec::new(), LogGraph::default()),
        };
        RebaseDialog {
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

    pub fn rebase(&self) -> rebase::Rebase {
        self.preview.rebase(self.target.clone(), self.stash)
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
        let title = format!(
            "Rebase {} onto {}",
            self.preview.branch,
            rebase::short_target(&self.rebase())
        );
        let width = (ctx.content_rect().width() - 80.0).clamp(420.0, 780.0);
        let shown = dialogs::Dialog::new("rebase-branch", &title)
            .screen(crate::usage::Screen::Rebase)
            .width(width)
            .opener(self.opener)
            .raise(self.fresh)
            .resizable()
            .show(ctx, |ui| {
                // Only the commits scroll: everything else stays in view.
                if let Some(ix) = self.repo.lookup(&self.preview.onto)
                    && dialogs::commit_line(ui, self.repo.commit(ix), self.repo.abbrev_len)
                {
                    asked.log = Some(self.preview.onto);
                }
                let n = self.commits.len();
                let min = list_height(n.min(MIN_ROWS));
                let natural = list_height(n);
                let picked = dialogs::growing(ui, min, |ui, height| {
                    let height = height.unwrap_or(natural);
                    (self.commits_table(ui, palette, options, height), natural)
                });
                if let Some(oid) = picked {
                    asked.log = Some(oid);
                }
                self.todo_buttons(ui);
                if self.preview.dirty {
                    ui.checkbox(&mut self.stash, "Stash changes").on_hover_text(
                        "Set your uncommitted changes aside first, and put them back \
                         afterwards (git rebase --autostash).",
                    );
                }
                if let Some(why) = self.preview.blocked(self.stash) {
                    ui.colored_label(ui.visuals().error_fg_color, why);
                }
                dialogs::command_box(ui, &[command_text(&rebase::command(&self.rebase()))]);
                let enabled = self.preview.blocked(self.stash).is_none() && !busy;
                dialogs::actions(ui, "Rebase", enabled, false, false)
            });
        self.fresh = false;
        asked.answer = Some(if shown.should_close() {
            dialogs::Answer::Cancel
        } else {
            shown.inner
        });
        asked
    }

    /// The selected commits that git replays.
    fn selected(&self) -> Vec<Oid> {
        self.list
            .many
            .iter()
            .map(|&i| self.repo.commit(self.commits[i]).oid)
            .filter(|&oid| self.preview.todo(oid).is_some())
            .collect()
    }

    /// Has the rebase do `todo` with the selected commits.
    fn set_todo(&mut self, todo: Todo) {
        for oid in self.selected() {
            self.preview.set_todo(oid, todo);
        }
    }

    /// What the selected commits share, if anything.
    fn selected_todo(&self) -> Option<Todo> {
        let mut todos = self
            .selected()
            .into_iter()
            .filter_map(|o| self.preview.todo(o));
        let first = todos.next()?;
        todos.all(|t| t == first).then_some(first)
    }

    /// Has the rebase do the next thing with `commit`: pick, squash, drop, and pick again.
    /// Squash is passed over where it can't be.
    fn cycle(&mut self, commit: Oid) {
        let next = match self.preview.todo(commit) {
            Some(Todo::Pick) if self.preview.can_squash(&[commit]) => Todo::Squash,
            Some(Todo::Pick | Todo::Squash) => Todo::Drop,
            Some(Todo::Drop) => Todo::Pick,
            None => return,
        };
        self.preview.set_todo(commit, next);
    }

    /// Whether the selected commits can be squashed: each has a commit kept before it.
    fn squashable(&self) -> bool {
        self.preview.can_squash(&self.selected())
    }

    /// Pick, Squash and Drop for the selected commits, also by their keys (git's abbreviations
    /// in the todo list), and Ctrl+A to select them all. Squash only where it can.
    fn todo_buttons(&mut self, ui: &mut Ui) {
        let mut chosen = None;
        ui.input_mut(|i| {
            if i.consume_key(egui::Modifiers::COMMAND, egui::Key::A) {
                self.list.many = (0..self.commits.len()).collect();
            }
            for (todo, key) in [
                (Todo::Pick, egui::Key::P),
                (Todo::Squash, egui::Key::S),
                (Todo::Drop, egui::Key::D),
            ] {
                if i.consume_key(egui::Modifiers::NONE, key) {
                    chosen = Some(todo);
                }
            }
        });
        let any = !self.selected().is_empty();
        let squashable = self.squashable();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_enabled_ui(any, |ui| {
                for todo in Todo::ALL {
                    let (tip, disabled) = match todo {
                        Todo::Pick => ("Replay it as it is (p)", None),
                        Todo::Squash => (
                            "Meld it into the commit before it (s)",
                            Some("Nothing kept before it to squash into"),
                        ),
                        Todo::Drop => ("Leave it out (d)", None),
                    };
                    let enabled = todo != Todo::Squash || squashable;
                    let (glyph, color) = todo_icon(ui, todo);
                    let label = capitalized(todo.word());
                    let button = ui
                        .add_enabled_ui(enabled, |ui| {
                            crate::widgets::icon_text_button(ui, glyph, color, &label)
                        })
                        .inner
                        .on_hover_text(tip)
                        .on_disabled_hover_text(match disabled {
                            Some(why) if any => why,
                            _ => "Select commits first: Ctrl+ or Shift+click",
                        });
                    if button.clicked() {
                        chosen = Some(todo);
                    }
                }
            });
        });
        if let Some(todo) = chosen
            && (todo != Todo::Squash || squashable)
        {
            self.set_todo(todo);
        }
    }

    /// The commits being rebased, as the log window lists them, `height` tall, with what the
    /// rebase does with each; those git leaves out are greyed, with the reason on hover, as are
    /// those dropped. Returns the commit double-clicked.
    fn commits_table(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        options: &GraphOptions,
        height: f32,
    ) -> Option<Oid> {
        let c = log_window::colors(ui);
        let table = CommitTable {
            id: Id::new("rebase-commits"),
            rows: self.commits.len(),
            graph: &self.graph,
            abbrev_len: self.repo.abbrev_len,
            palette,
            select: Select::Many,
            icons: true,
        };
        let (repo, commits, preview) = (&*self.repo, &self.commits, &self.preview);
        let refs = &self.refs;
        let shared = self.selected_todo();
        let squashable = self.squashable();
        let icons = Todo::ALL.map(|t| todo_icon(ui, t));
        let mut chosen = None;
        let mut tip = |ui: &mut Ui, i: usize| {
            if let Some(skipped) = preview.skipped(repo.commit(commits[i]).oid) {
                ui.label(skipped.reason());
            }
        };
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
                            icon: preview.todo(commit.oid).map(|t| {
                                let (glyph, color) = icons[t as usize];
                                (glyph, color, t.word())
                            }),
                            greyed: preview.skipped(commit.oid).is_some()
                                || preview.todo(commit.oid) == Some(Todo::Drop),
                            ..Row::default()
                        }
                    },
                    |ui, i, _| {
                        let replayed = preview.todo(repo.commit(commits[i]).oid).is_some();
                        ui.add_enabled_ui(replayed, |ui| {
                            for (todo, key) in Todo::ALL.into_iter().zip(["P", "S", "D"]) {
                                let mark = crate::menu::Mark::Radio(shared == Some(todo));
                                let label = capitalized(todo.word());
                                let enabled = todo != Todo::Squash || squashable;
                                let item = ui.add_enabled_ui(enabled, |ui| {
                                    crate::menu::item(ui, &label, key, mark)
                                });
                                if item.inner.clicked() {
                                    chosen = Some(todo);
                                }
                            }
                        });
                    },
                    Some(&mut tip),
                )
            })
            .inner;
        if let Some(todo) = chosen {
            self.set_todo(todo);
        }
        if let Some(i) = clicks.icon {
            self.cycle(self.repo.commit(self.commits[i]).oid);
        }
        clicks
            .double_clicked
            .map(|i| self.repo.commit(self.commits[i]).oid)
    }
}

/// What the banner's button asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BannerClick {
    /// *Compare with working tree*: HEAD against the working tree, as the graph's menu opens it.
    Compare(parterre_core::Oid),
}

/// The banner across the graph while the open worktree is stuck: what it needs (its
/// conflicted files, listed on hover, or what stopped), and *Compare with working tree*,
/// where each file opens in the right tool. Call before the central panel.
pub fn banner(ui: &mut Ui, repo: &Repo, catalog: &Catalog) -> Option<BannerClick> {
    let stuck = catalog.stuck()?;
    let open = catalog.worktrees.iter().find(|w| w.open);
    let files = &catalog.conflicted;
    let picking = open.and_then(|w| w.picking.as_ref());
    // How far a rebase or a cherry-pick of several got.
    let progress = match open.and_then(|w| w.rebasing.as_ref()) {
        Some(r) => Some(format!("Rebase stopped at {}/{}", r.done, r.total)),
        None => picking
            .filter(|p| p.total > 1)
            .map(|p| format!("Cherry-pick stopped at {}/{}", p.done, p.total)),
    };
    let operation = match (
        open.and_then(|w| w.merging),
        picking,
        open.and_then(|w| w.reverting),
    ) {
        (Some(_), _, _) => Some("Merge"),
        (_, Some(_), _) => Some("Cherry-pick"),
        (_, _, Some(_)) => Some("Revert"),
        _ => None,
    };
    let (title, detail) = if !files.is_empty() {
        (plural(files.len(), "conflicted file"), progress)
    } else if let Some(progress) = progress {
        (progress, None)
    } else if let Some(operation) = operation {
        // Stopped with no conflicts: a hook refused to commit it, or a pick came out empty.
        (format!("{operation} not committed"), None)
    } else {
        (stuck.reason(), None)
    };
    let hint = match stuck {
        Stuck::Conflicts if files.len() == 1 => "Resolve it with git, or go to another worktree.",
        Stuck::Conflicts => "Resolve them with git, or go to another worktree.",
        Stuck::InProgress(_) => FINISH,
    };
    let stashed = catalog
        .stashed_for_revert
        .as_ref()
        .map(|entry| format!("Your changes are stashed in {entry}."));
    let accent = stuck_color(ui);
    let (fill, border) = if ui.visuals().dark_mode {
        (
            Color32::from_rgb(52, 41, 24),
            Color32::from_rgb(105, 78, 34),
        )
    } else {
        (
            Color32::from_rgb(255, 248, 232),
            Color32::from_rgb(236, 208, 150),
        )
    };
    let text = ui.visuals().text_color();
    let weak = ui.visuals().weak_text_color();
    let mut click = None;
    let panel = egui::Panel::top("operation-in-progress")
        .frame(
            egui::Frame::new()
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, border))
                .inner_margin(egui::Margin {
                    left: 18,
                    right: 12,
                    top: 7,
                    bottom: 7,
                }),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(30.0);
                let (icon, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
                crate::widgets::paint_glyph(ui.painter(), icon, glyphs::WARNING, accent);
                ui.add_space(4.0);
                let title = ui.label(RichText::new(&title).color(text).strong());
                // The commit a cherry-pick stopped at, by its subject, and the files.
                let subject = picking
                    .and_then(|p| repo.lookup(&p.commit))
                    .map(|c| repo.commit(c).subject.clone());
                let hover: Vec<String> = subject.into_iter().chain(files.clone()).collect();
                if !hover.is_empty() {
                    title.on_hover_text(hover.join("\n"));
                }
                for line in detail.iter().chain(stashed.iter()) {
                    ui.label(RichText::new(line).color(text));
                }
                ui.label(RichText::new(hint).color(weak));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(head) = catalog.head
                        && crate::widgets::primary_button(ui, "Compare with working tree", 0.0)
                            .clicked()
                    {
                        click = Some(BannerClick::Compare(head));
                    }
                });
            });
        });
    // A stripe down its left edge in the stuck colour, as the graph draws a stuck worktree.
    let rect = panel.response.rect;
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, vec2(4.0, rect.height())),
        0.0,
        accent,
    );
    click
}

/// The confirmation, the menus and the banner, driven frame by frame in a headless context
/// against real, disposable repositories: clicked by the text on screen, as a person would.
#[cfg(test)]
mod tests {
    use std::path::Path;

    use eframe::egui;
    use parterre_core::Oid;
    use parterre_core::branches::Stuck;

    use super::super::branches::{self, Request};
    use super::super::tool_harness::{
        Harness, banner_click, banner_texts, before_merge, git, load, menu, merge_stops, read,
        stuck_merge, write,
    };

    fn commit(dir: &Path, path: &str, text: &str, message: &str) {
        write(dir, path, text);
        git(dir, &["add", path]);
        git(dir, &["commit", "-q", "-m", message]);
    }

    /// main: base → fix → feature → merge of side; up: base → the same fix → theirs.
    fn repository() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        // A rebase commits, and parterre's own git reads the identity from the repository,
        // not from the harness's environment: CI has no global one.
        git(p, &["config", "user.name", "Test"]);
        git(p, &["config", "user.email", "test@example.com"]);
        commit(p, "file", "base\n", "base");
        git(p, &["branch", "up"]);
        commit(p, "fix", "fixed\n", "fix");
        commit(p, "feature", "feature\n", "feature");
        git(p, &["switch", "-q", "-c", "side"]);
        commit(p, "side", "side\n", "side work");
        git(p, &["switch", "-q", "main"]);
        git(p, &["merge", "-q", "--no-ff", "-m", "Merge side", "side"]);
        git(p, &["branch", "-q", "-D", "side"]);
        git(p, &["switch", "-q", "up"]);
        commit(p, "fix", "fixed\n", "the same fix");
        commit(p, "theirs", "theirs\n", "theirs");
        git(p, &["switch", "-q", "main"]);
        dir
    }

    fn open(h: &mut Harness) -> String {
        let onto = h.rev("up");
        let title = "Rebase main onto up".to_owned();
        let request = Request::Rebase {
            onto,
            target: "up".into(),
        };
        h.ask(request, &title);
        title
    }

    #[test]
    fn the_confirmation_lists_the_commits_and_rebase_runs_git() {
        let mut h = Harness::new(repository());
        let title = open(&mut h);
        for subject in ["Merge side", "side work", "feature", "fix"] {
            assert!(h.shows(subject), "{subject}: {:?}", h.texts);
        }
        assert!(!h.shows("Stash changes"));
        h.click("Git command");
        assert!(h.shows_part("git rebase up"), "{:?}", h.texts);
        h.click("Rebase");
        h.until("the branch is rebased", |h| {
            git(h.path(), &["rev-list", "--count", "up..main"]) == "2"
        });
        h.until("the notification", |h| {
            h.shows(&title) && !h.shows("Cancel")
        });
    }

    #[test]
    fn uncommitted_changes_wait_for_the_stash_box() {
        let dir = repository();
        write(dir.path(), "feature", "edited\n");
        let mut h = Harness::new(dir);
        let tip = h.rev("main");
        open(&mut h);
        assert!(h.shows("Stash changes"));
        assert!(h.shows("Commit or stash your changes first."));
        h.click("Rebase");
        for _ in 0..20 {
            h.frame();
        }
        assert_eq!(h.rev("main"), tip, "Rebase is greyed out");
        h.click("Stash changes");
        assert!(!h.shows("Commit or stash your changes first."));
        h.click("Git command");
        assert!(h.shows_part("git rebase --autostash up"), "{:?}", h.texts);
        h.click("Rebase");
        // Git moves the branch before it puts the changes back: wait for it to finish.
        h.until("the notification", |h| {
            h.shows("Rebase main onto up") && !h.shows("Cancel")
        });
        assert_ne!(h.rev("main"), tip);
        assert_eq!(read(h.path(), "feature"), "edited\n");
    }

    #[test]
    fn a_conflict_says_so_in_orange() {
        let dir = repository();
        let p = dir.path();
        git(p, &["switch", "-q", "up"]);
        commit(p, "feature", "theirs\n", "their feature");
        git(p, &["switch", "-q", "main"]);
        let mut h = Harness::new(dir);
        open(&mut h);
        h.click("Rebase");
        h.until("the orange notice", |h| {
            h.shows("Rebase stopped on conflicts in 1 file")
        });
        assert!(h.shows("Finish or abort it with git, or go to another worktree."));
    }

    /// The commits' subjects in `range`, newest first, and the full message of the newest.
    fn log(dir: &Path, range: &str) -> (Vec<String>, String) {
        let subjects = git(dir, &["log", "--format=%s", range]);
        let message = git(dir, &["log", "-1", "--format=%B", range]);
        (subjects.lines().map(str::to_owned).collect(), message)
    }

    /// Rebases, and waits for the branch to have `n` commits of its own.
    fn rebase(h: &mut Harness, n: usize) {
        h.click("Rebase");
        h.until("the branch is rebased", |h| {
            git(h.path(), &["rev-list", "--count", "up..main"]) == n.to_string()
        });
    }

    #[test]
    fn a_commit_squashed_by_its_key_makes_it_interactive() {
        let mut h = Harness::new(repository());
        open(&mut h);
        h.click("Git command");
        assert!(h.shows("git rebase up"), "{:?}", h.texts);
        // Git replays feature and side work: the fix is already in up, the merge flattened.
        h.click("side work");
        h.key(egui::Key::S);
        assert!(h.shows("git rebase --interactive up"), "{:?}", h.texts);
        // Dropping feature and picking it again: side work melds into it once more.
        h.click("feature");
        h.key(egui::Key::D);
        h.key(egui::Key::P);
        rebase(&mut h, 1);
        let (subjects, message) = log(h.path(), "up..main");
        assert_eq!(subjects, ["feature"]);
        assert_eq!(message, "feature\n\nside work");
    }

    #[test]
    fn a_squash_into_a_dropped_commit_is_a_pick() {
        let mut h = Harness::new(repository());
        open(&mut h);
        h.click("side work");
        h.click("Squash");
        h.click("feature");
        h.click("Drop");
        rebase(&mut h, 1);
        let (subjects, message) = log(h.path(), "up..main");
        assert_eq!(subjects, ["side work"]);
        assert_eq!(message, "side work");
    }

    #[test]
    fn many_commits_are_dropped_at_once() {
        let mut h = Harness::new(repository());
        open(&mut h);
        // A range from the merge to the fix: Drop changes only those git replays.
        h.click("Merge side");
        h.click_with("fix", egui::Modifiers::SHIFT);
        h.click("Drop");
        // Ctrl+click takes feature out of the selection; Pick puts side work back.
        h.click_with("feature", egui::Modifiers::COMMAND);
        h.click("Pick");
        rebase(&mut h, 1);
        assert_eq!(log(h.path(), "up..main").0, ["side work"]);
    }

    #[test]
    fn squash_is_greyed_out_with_nothing_kept_before_it() {
        let mut h = Harness::new(repository());
        open(&mut h);
        h.click("Git command");
        // feature is the first commit replayed; with side work too, the same.
        h.click("feature");
        h.click("Squash");
        h.key(egui::Key::S);
        h.click_with("side work", egui::Modifiers::COMMAND);
        h.click("Squash");
        assert!(h.shows("git rebase up"), "{:?}", h.texts);
        assert!(!h.shows_part("Cannot squash"));
        // side work alone has feature before it.
        h.click("side work");
        h.click("Squash");
        assert!(h.shows("git rebase --interactive up"), "{:?}", h.texts);
    }

    #[test]
    fn clicking_an_icon_cycles_its_commit_alone() {
        let mut h = Harness::new(repository());
        open(&mut h);
        h.click("Git command");
        // The icon column starts where the dialog's content does, as the line above the list.
        let left = h.texts.iter().find(|(t, _)| t == "at").unwrap().1.left();
        let icon = |h: &Harness, subject: &str| egui::pos2(left + 12.0, h.at(subject).y);
        // feature, first: pick, then drop (it can't be squashed), then pick again.
        let at = icon(&h, "feature");
        h.click_at(at, 1);
        assert!(h.shows("git rebase --interactive up"), "{:?}", h.texts);
        h.click_at(at, 1);
        assert!(h.shows("git rebase up"), "{:?}", h.texts);
        // side work, clicked twice quickly: squash and drop, and no double click opens the log.
        let at = icon(&h, "side work");
        h.click_at(at, 2);
        assert!(h.tool.log_request.is_none());
        // Then pick, and squash.
        h.click_at(at, 1);
        h.click_at(at, 1);
        rebase(&mut h, 1);
        assert_eq!(log(h.path(), "up..main").1, "feature\n\nside work");
    }

    #[test]
    fn double_clicking_a_commit_opens_the_log() {
        let mut h = Harness::new(repository());
        open(&mut h);
        let feature = h.rev("main^1");
        let at = h.at("feature");
        h.click_at(at, 2);
        let (_, commits, _) = h.tool.log_request.take().expect("the log is asked for");
        assert_eq!(commits, [feature]);
    }

    fn rev(dir: &Path, r: &str) -> Oid {
        Oid::from_hex(&git(dir, &["rev-parse", r])).unwrap()
    }

    #[test]
    fn the_menus_offer_it_only_where_it_would_really_rebase() {
        let dir = repository();
        let p = dir.path();
        git(p, &["branch", "behind", "main~1"]);
        let (repo, catalog) = load(p);
        let node = |at: &str, click: Option<&str>| {
            let (repo, catalog, commit) = (&repo, &catalog, rev(p, at));
            menu(
                move |ui| {
                    branches::node_menu(ui, repo, commit, &[commit], Some(catalog), false, false)
                },
                click,
            )
        };
        let short = |at: &str| rev(p, at).short(repo.abbrev_len.max(7));
        // A branch and its commit: a submenu.
        assert!(node("up", None).0.contains(&"Rebase main onto".to_owned()));
        // Already in main: nothing to do.
        assert!(
            !node("behind", None)
                .0
                .iter()
                .any(|t| t.starts_with("Rebase"))
        );
        // A commit with no branch on it (as the log's rows have it too): the commit alone.
        let item = format!("Rebase main onto {}", short("up~1"));
        let (texts, asked) = node("up~1", Some(&item));
        assert!(texts.contains(&item), "{texts:?}");
        match asked {
            Some(Request::Rebase { onto, target }) => {
                assert_eq!(onto, rev(p, "up~1"));
                assert_eq!(target, onto.to_hex());
            }
            other => panic!("expected a rebase: {other:?}"),
        }
    }

    /// An autostash that couldn't be put back leaves conflicted files, and no operation in
    /// progress: the worktree is stuck all the same.
    #[test]
    fn conflicts_left_by_the_autostash_show_the_banner_and_grey_switching() {
        let dir = repository();
        let p = dir.path();
        // up changes the file the uncommitted edit is to; old's own commits don't touch it.
        git(p, &["switch", "-q", "up"]);
        commit(p, "file", "theirs\n", "their file");
        git(p, &["switch", "-q", "-c", "old", "main~1"]);
        git(p, &["branch", "other", "main"]);
        write(p, "file", "my edit\n");
        let mut h = Harness::new(dir);
        let onto = h.rev("up");
        let request = Request::Rebase {
            onto,
            target: "up".into(),
        };
        h.ask(request, "Rebase old onto up");
        h.click("Stash changes");
        h.click("Rebase");
        h.until("the orange notice", |h| h.shows("Rebased old onto up"));
        assert!(h.shows_part("conflicted in 1 file"), "{:?}", h.texts);
        let p = h.path();
        assert_eq!(git(p, &["stash", "list"]).lines().count(), 1);
        let texts = banner_texts(p);
        assert!(texts.contains(&"1 conflicted file".to_owned()), "{texts:?}");
        assert!(texts.contains(&"Resolve it with git, or go to another worktree.".to_owned()));

        let (repo, catalog) = load(p);
        let main = rev(p, "main");
        let (texts, asked) = menu(
            |ui| branches::node_menu(ui, &repo, main, &[main], Some(&catalog), false, false),
            Some("Switch to"),
        );
        // main, other and the commit detached: one greyed-out item for all of them.
        assert!(texts.contains(&"Switch to".to_owned()), "{texts:?}");
        assert!(asked.is_none(), "switching is greyed out");
    }

    #[test]
    fn a_stuck_worktree_greys_out_what_would_change_it_and_shows_a_banner() {
        let dir = repository();
        let p = dir.path();
        git(p, &["switch", "-q", "up"]);
        commit(p, "feature", "theirs\n", "their feature");
        git(p, &["switch", "-q", "main"]);
        git(p, &["branch", "other", "up"]);
        let up = rev(p, "up");
        let out = std::process::Command::new("git")
            .current_dir(p)
            .args(["rebase", "up"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_EDITOR", ":")
            .output()
            .unwrap();
        assert!(!out.status.success(), "the rebase stops on its conflict");
        let (repo, catalog) = load(p);
        assert_eq!(catalog.stuck(), Some(Stuck::InProgress("a rebase")));

        // Rebase and Switch to are shown, but clicking them asks for nothing.
        let node = |click: Option<&str>| {
            let (repo, catalog) = (&repo, &catalog);
            menu(
                move |ui| branches::node_menu(ui, repo, up, &[up], Some(catalog), false, false),
                click,
            )
        };
        let (texts, _) = node(None);
        assert!(texts.contains(&"Rebase main onto".to_owned()), "{texts:?}");
        assert!(texts.contains(&"Switch to".to_owned()), "{texts:?}");
        assert!(node(Some("Rebase main onto")).1.is_none());
        assert!(node(Some("Switch to")).1.is_none());
        // Creating a branch stays.
        assert!(matches!(
            node(Some("Create branch here…")).1,
            Some(Request::Create { .. })
        ));
        let reset = {
            let catalog = &catalog;
            menu(
                move |ui| branches::reset_item(ui, up, Some(catalog), false),
                Some("Reset main to here…"),
            )
        };
        assert!(reset.1.is_none());

        let texts = banner_texts(p);
        // The fix is dropped and the merge flattened: feature, the first of two, conflicts.
        assert!(texts.contains(&"1 conflicted file".to_owned()), "{texts:?}");
        assert!(
            texts.contains(&"Rebase stopped at 1/2".to_owned()),
            "{texts:?}"
        );
    }

    #[test]
    fn the_banner_opens_head_against_the_working_tree() {
        let dir = stuck_merge();
        let head = Oid::from_hex(&git(dir.path(), &["rev-parse", "HEAD"])).unwrap();
        assert_eq!(
            banner_click(dir.path(), "Compare with working tree"),
            Some(super::BannerClick::Compare(head))
        );
    }

    #[test]
    fn opening_a_stuck_worktree_shows_the_banner_at_once() {
        let mut h = Harness::new(stuck_merge());
        h.until("the worktree is looked at", |h| h.tool.catalog.is_some());
        assert!(h.tool.banner_shown());
    }

    #[test]
    fn a_change_from_outside_shows_the_banner_once_it_lasts() {
        let mut h = Harness::new(before_merge());
        h.until("the worktree is looked at", |h| h.tool.catalog.is_some());
        assert!(!h.tool.banner_shown());
        merge_stops(h.path());
        h.reload();
        h.until("the merge is seen", |h| {
            h.tool.catalog.as_ref().is_some_and(|c| c.stuck().is_some())
        });
        let seen = h.time;
        assert!(!h.tool.banner_shown(), "not at once");
        let start = std::time::Instant::now();
        while !h.tool.banner_shown() {
            assert!(start.elapsed().as_secs() < 20, "the banner never shows");
            std::thread::sleep(std::time::Duration::from_millis(5));
            h.frame();
        }
        assert!(h.time - seen >= parterre_core::banner::WAIT);
    }

    #[test]
    fn a_change_from_outside_that_is_over_in_time_never_shows_the_banner() {
        let mut h = Harness::new(before_merge());
        h.until("the worktree is looked at", |h| h.tool.catalog.is_some());
        merge_stops(h.path());
        h.reload();
        h.until("the merge is seen", |h| {
            h.tool.catalog.as_ref().is_some_and(|c| c.stuck().is_some())
        });
        git(h.path(), &["merge", "--abort"]);
        h.until("the abort is seen", |h| {
            h.tool.catalog.as_ref().is_some_and(|c| c.stuck().is_none())
        });
        assert!(!h.tool.banner_shown());
        let until = h.time + 2.0 * parterre_core::banner::WAIT;
        while h.time < until {
            h.frame();
            assert!(!h.tool.banner_shown());
        }
    }

    #[test]
    fn resolving_in_another_tool_is_seen_without_a_reload() {
        let mut h = Harness::new(stuck_merge());
        h.until("the worktree is looked at", |h| h.tool.catalog.is_some());
        assert_eq!(h.tool.catalog.as_ref().unwrap().conflicted.len(), 2);
        git(h.path(), &["add", "text.txt"]);
        h.until("the index is looked at again", |h| {
            h.tool.catalog.as_ref().unwrap().conflicted.len() == 1
        });
        assert!(h.tool.conflicts_changed);
    }

    #[test]
    fn finishing_a_file_keeps_the_banner_up_while_git_runs() {
        let mut h = Harness::new(stuck_merge());
        h.until("the worktree is looked at", |h| h.tool.catalog.is_some());
        assert!(h.tool.banner_shown());
        let (_, conflicts) =
            parterre_core::conflicts::list(&parterre_core::git::Git::new(h.path())).unwrap();
        let gone = conflicts
            .into_iter()
            .find(|c| c.path == "gone.txt")
            .unwrap();
        let resolve = parterre_core::conflicts::Resolve {
            conflict: gone,
            answer: parterre_core::conflicts::Answer::Ours,
            item: String::new(),
        };
        let ctx = h.ctx.clone();
        h.tool.request(
            &ctx,
            Request::Run(parterre_core::branches::Action::Resolve(Box::new(resolve))),
            egui::ViewportId::ROOT,
        );
        while h.tool.busy() {
            assert!(h.tool.banner_shown(), "never hidden while it runs");
            h.frame();
        }
        h.reload();
        h.until("the worktree is looked at again", |h| {
            h.tool.catalog.as_ref().unwrap().conflicted == ["text.txt"]
        });
        assert!(h.tool.banner_shown(), "still merging");
        assert_eq!(read(h.path(), "gone.txt"), "main\n");
    }
}
