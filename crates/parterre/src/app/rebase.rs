//! Rebasing the open worktree's branch: the confirmation, which lists the commits being
//! rebased as the log window does, greying those git leaves out, and the banner across the
//! graph while the open worktree is stuck with an operation in progress.

use std::sync::Arc;

use eframe::egui::{self, Color32, Id, RichText, Ui, ViewportId, vec2};
use parterre_core::branches::{Catalog, command_text};
use parterre_core::log::{LogOptions, LogQuery};
use parterre_core::log_graph::LogGraph;
use parterre_core::rebase::{self, Preview};
use parterre_core::revgraph::GraphOptions;
use parterre_core::{CommitIx, Oid, Repo};

use super::commit_table::{CommitList, CommitTable, ROW, Row};
use super::log_window::{self, HEADING};
use crate::dialogs;
use crate::theme::Palette;

/// The commits listed before the table scrolls.
const MAX_ROWS: usize = 12;

/// The colour of a worktree stuck with an operation in progress, as its zigzag edge in the
/// graph is drawn.
pub fn stuck_color(ui: &Ui) -> Color32 {
    if ui.visuals().dark_mode {
        Color32::from_rgb(240, 160, 60)
    } else {
        Color32::from_rgb(170, 90, 0)
    }
}

/// Why an operation isn't offered while the open worktree is stuck.
pub fn stuck_reason(what: &str) -> String {
    format!("{} is in progress in this worktree", capitalized(what))
}

fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
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
            .width(width)
            .opener(self.opener)
            .raise(self.fresh)
            .show(ctx, |ui| {
                dialogs::fields(ui, |ui| {
                    if let Some(ix) = self.repo.lookup(&self.preview.onto)
                        && dialogs::commit_line(ui, self.repo.commit(ix), self.repo.abbrev_len)
                    {
                        asked.log = Some(self.preview.onto);
                    }
                    ui.add_space(6.0);
                    if let Some(oid) = self.commits_table(ui, palette, options) {
                        asked.log = Some(oid);
                    }
                    if self.preview.dirty {
                        ui.add_space(6.0);
                        ui.checkbox(&mut self.stash, "Stash changes").on_hover_text(
                            "Set your uncommitted changes aside first, and put them back \
                             afterwards (git rebase --autostash).",
                        );
                        if let Some(why) = self.preview.blocked(self.stash) {
                            ui.colored_label(ui.visuals().error_fg_color, why);
                        }
                    }
                });
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

    /// The commits being rebased, as the log window lists them; those git leaves out are
    /// greyed, with the reason on hover. Returns the commit double-clicked.
    fn commits_table(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        options: &GraphOptions,
    ) -> Option<Oid> {
        let c = log_window::colors(ui);
        let table = CommitTable {
            id: Id::new("rebase-commits"),
            rows: self.commits.len(),
            graph: &self.graph,
            abbrev_len: self.repo.abbrev_len,
            palette,
            pairs: false,
        };
        // A little more than the rows, for the spacing around the scroll area.
        let height = HEADING + ROW * self.commits.len().clamp(1, MAX_ROWS) as f32 + 12.0;
        let (repo, commits, preview) = (&*self.repo, &self.commits, &self.preview);
        let refs = &self.refs;
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
                            greyed: preview.skipped(commit.oid).is_some(),
                            ..Row::default()
                        }
                    },
                    |_, _, _| {},
                    Some(&mut tip),
                )
            })
            .inner;
        clicks
            .double_clicked
            .map(|i| self.repo.commit(self.commits[i]).oid)
    }
}

/// The banner across the graph while the open worktree has an operation in progress: what's
/// stopped there, its conflicted files on hover, and *Open in terminal*. Call before the
/// central panel. Returns why the terminal didn't open, if it didn't.
pub fn banner(ui: &mut Ui, repo: &Repo, catalog: &Catalog) -> Option<String> {
    let what = catalog.stuck()?;
    let open = catalog.worktrees.iter().find(|w| w.open);
    let mut text = match open.and_then(|w| w.rebasing.as_ref()) {
        Some(r) => {
            let branch = r.branch.as_deref().unwrap_or("HEAD");
            // Git records only the commit, and `git status` names it the same way.
            let onto = r
                .onto
                .map(|o| format!(" onto {}", o.short(repo.abbrev_len.max(7))))
                .unwrap_or_default();
            format!("Rebasing {branch}{onto} stopped at {}/{}", r.done, r.total)
        }
        None => stuck_reason(what),
    };
    let files = &catalog.conflicted;
    if !files.is_empty() {
        text.push_str(&format!(": {}", plural(files.len(), "conflicted file")));
    }
    let (fill, color) = if ui.visuals().dark_mode {
        (
            Color32::from_rgb(75, 45, 10),
            Color32::from_rgb(255, 200, 130),
        )
    } else {
        (
            Color32::from_rgb(255, 232, 196),
            Color32::from_rgb(110, 55, 0),
        )
    };
    let stroke = stuck_color(ui);
    let mut error = None;
    egui::Panel::top("operation-in-progress")
        .frame(
            egui::Frame::new()
                .fill(fill)
                .stroke(egui::Stroke::new(1.0, stroke))
                .inner_margin(egui::Margin::symmetric(12, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let button = 130.0;
                ui.allocate_ui(vec2(ui.available_width() - button, 0.0), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("⚠").color(color).strong());
                        let label = ui.label(RichText::new(&text).color(color).strong());
                        if !files.is_empty() {
                            label.on_hover_text(files.join("\n"));
                        }
                        ui.label(
                            RichText::new(
                                "Finish or abort it with git, or go to another worktree.",
                            )
                            .color(color),
                        );
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Open in terminal").clicked()
                        && let Err(e) = crate::file_manager::open_terminal(&catalog.root)
                    {
                        error = Some(e);
                    }
                });
            });
        });
    error
}

/// The confirmation, the menus and the banner, driven frame by frame in a headless context
/// against real, disposable repositories: clicked by the text on screen, as a person would.
#[cfg(test)]
mod tests {
    use std::path::Path;

    use eframe::egui::{self, Pos2, Rect};
    use parterre_core::branches::Catalog;
    use parterre_core::{Oid, Repo};

    use super::super::branches::{self, Request};
    use super::super::tool_harness::{Harness, collect, git, read, write};

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
        h.until("the branch is rebased", |h| h.rev("main") != tip);
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

    /// The texts a menu shows, and what clicking `click` (if given) asks for.
    fn menu(
        f: impl Fn(&mut egui::Ui) -> Option<Request>,
        click: Option<&str>,
    ) -> (Vec<String>, Option<Request>) {
        let ctx = egui::Context::default();
        let frame = |events: Vec<egui::Event>, texts: &mut Vec<(String, Rect)>| {
            let mut asked = None;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| asked = f(ui));
            output.textures_delta.clear();
            texts.clear();
            for clipped in &output.shapes {
                collect(&clipped.shape, texts);
            }
            asked
        };
        let mut texts = Vec::new();
        frame(Vec::new(), &mut texts);
        let mut asked = None;
        if let Some(text) = click {
            let at = texts
                .iter()
                .find(|(t, _)| t == text)
                .unwrap_or_else(|| panic!("no {text:?}: {texts:?}"))
                .1
                .center();
            for pressed in [true, false] {
                let events = vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ];
                asked = asked.or(frame(events, &mut texts));
            }
        }
        (texts.into_iter().map(|(t, _)| t).collect(), asked)
    }

    fn load(dir: &Path) -> (Repo, Catalog) {
        (
            parterre_core::git::load_repo(dir).unwrap(),
            Catalog::load(dir).unwrap(),
        )
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
        let node = |at: &str| {
            let (repo, catalog, commit) = (&repo, &catalog, rev(p, at));
            menu(
                move |ui| branches::node_menu(ui, repo, commit, Some(catalog), false, false),
                None,
            )
            .0
        };
        let row = |at: &str, click: Option<&str>| {
            let (repo, catalog, commit) = (&repo, &catalog, rev(p, at));
            menu(
                move |ui| branches::rebase_item(ui, repo, commit, Some(catalog), false),
                click,
            )
        };
        assert!(node("up").contains(&"Rebase main onto up".to_owned()));
        // Already in main: nothing to do.
        assert!(!node("behind").iter().any(|t| t.starts_with("Rebase")));
        let short = rev(p, "up").short(repo.abbrev_len.max(7));
        let (texts, asked) = row("up", Some(&format!("Rebase main onto {short}")));
        assert_eq!(texts, [format!("Rebase main onto {short}")]);
        match asked {
            Some(Request::Rebase { onto, target }) => {
                assert_eq!(onto, rev(p, "up"));
                assert_eq!(target, onto.to_hex());
            }
            other => panic!("expected a rebase: {other:?}"),
        }
        assert!(row("main~1", None).0.is_empty());
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
        assert_eq!(catalog.stuck(), Some("a rebase"));

        // Rebase and Switch to are shown, but clicking them asks for nothing.
        let node = |click: Option<&str>| {
            let (repo, catalog) = (&repo, &catalog);
            menu(
                move |ui| branches::node_menu(ui, repo, up, Some(catalog), false, false),
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
                Some("Reset to here…"),
            )
        };
        assert!(reset.1.is_none());

        let ctx = egui::Context::default();
        let mut texts = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            super::banner(ui, &repo, &catalog);
        });
        output.textures_delta.clear();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut texts);
        }
        let texts: Vec<String> = texts.into_iter().map(|(t, _)| t).collect();
        let short = up.short(repo.abbrev_len.max(7));
        // The fix is dropped and the merge flattened: feature, the first of two, conflicts.
        let expected = format!("Rebasing main onto {short} stopped at 1/2: 1 conflicted file");
        assert!(texts.contains(&expected), "{texts:?}");
    }
}
