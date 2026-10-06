//! Cherry-picking onto the open worktree's branch: the confirmation, which lists the commits as
//! the log window does, newest first (they go on from the bottom up), greying those left out,
//! with whether to pick or drop each: for a selection of them at once, or for one by clicking
//! its icon. `-x` is remembered; *Stash changes* is offered only with uncommitted changes.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Color32, Id, Ui, ViewportId, vec2};
use parterre_core::branches::command_text;
use parterre_core::cherry_pick::{self, Preview};
use parterre_core::glyphs;
use parterre_core::log_graph::LogGraph;
use parterre_core::revgraph::GraphOptions;
use parterre_core::{CommitIx, Oid, Repo};

use super::commit_table::{CommitList, CommitTable, Row, Select};
use super::log_window;
use super::merge::list_height;
use super::rebase::Asked;
use crate::dialogs;
use crate::theme::Palette;

/// The commits listed at least, however small the window: the table scrolls.
const MIN_ROWS: usize = 3;

/// Where the last choice of `-x` is remembered.
fn record_origin_id() -> Id {
    Id::new("cherry-pick-record-origin")
}

/// The icon of a pick or a drop, its colour, and what it's called.
fn pick_icon(ui: &Ui, picked: bool) -> (glyphs::Glyph, Color32, &'static str) {
    if picked {
        (glyphs::PICK, ui.visuals().text_color(), "pick")
    } else {
        (glyphs::DROP, ui.visuals().error_fg_color, "drop")
    }
}

pub struct CherryPickDialog {
    pub preview: Preview,
    repo: Arc<Repo>,
    record_origin: bool,
    stash: bool,
    /// The window it was asked from.
    pub opener: ViewportId,
    fresh: bool,
    /// The listed commits, as the log lists them.
    commits: Vec<CommitIx>,
    graph: LogGraph,
    refs: Vec<Vec<usize>>,
    list: CommitList,
}

impl std::fmt::Debug for CherryPickDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CherryPickDialog")
            .field("preview", &self.preview)
            .finish_non_exhaustive()
    }
}

impl CherryPickDialog {
    pub fn new(preview: Preview, repo: Arc<Repo>, ctx: &egui::Context, opener: ViewportId) -> Self {
        let commits: Vec<CommitIx> = preview
            .listed
            .iter()
            .filter_map(|c| repo.lookup(c))
            .collect();
        // The lanes between the listed commits; the rest of their history is outside.
        let rows: HashMap<CommitIx, u32> = commits
            .iter()
            .enumerate()
            .map(|(i, &c)| (c, i as u32))
            .collect();
        let (parents, outside) = commits
            .iter()
            .map(|&c| {
                let parents = &repo.commit(c).parents;
                let listed: Vec<u32> = parents
                    .iter()
                    .filter_map(|p| rows.get(p).copied())
                    .collect();
                let outside = listed.len() < parents.len();
                (listed, outside)
            })
            .unzip();
        CherryPickDialog {
            record_origin: ctx.data_mut(|d| *d.get_persisted_mut_or(record_origin_id(), false)),
            stash: false,
            refs: repo.refs_by_commit(),
            graph: LogGraph::from_parents(parents, outside),
            commits,
            preview,
            repo,
            opener,
            fresh: true,
            list: CommitList::default(),
        }
    }

    pub fn cherry_pick(&self) -> cherry_pick::CherryPick {
        self.preview.cherry_pick(self.record_origin, self.stash)
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
        let pick = self.cherry_pick();
        let title = format!("Cherry-pick {} onto {}", pick.name, pick.branch);
        let width = (ctx.content_rect().width() - 80.0).clamp(420.0, 780.0);
        let shown = dialogs::Dialog::new("cherry-pick", &title)
            .screen(crate::usage::Screen::CherryPick)
            .width(width)
            .opener(self.opener)
            .raise(self.fresh)
            .resizable()
            .show(ctx, |ui| {
                // Only the commits scroll: everything else stays in view.
                if let Some(ix) = self.repo.lookup(&self.preview.head)
                    && dialogs::commit_line(ui, self.repo.commit(ix), self.repo.abbrev_len)
                {
                    asked.log = Some(self.preview.head);
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
                self.pick_buttons(ui);
                if ui
                    .checkbox(
                        &mut self.record_origin,
                        "Append \"(cherry picked from commit …)\"",
                    )
                    .on_hover_text(
                        "Add a line saying which commit each one was picked from (git \
                         cherry-pick -x).",
                    )
                    .changed()
                {
                    let id = record_origin_id();
                    let value = self.record_origin;
                    ui.data_mut(|d| d.insert_persisted(id, value));
                }
                if self.preview.dirty {
                    ui.checkbox(&mut self.stash, "Stash changes").on_hover_text(
                        "Set your uncommitted changes aside first, and put them back \
                         afterwards (git stash).",
                    );
                }
                let blocked = self.preview.blocked();
                if let Some(why) = blocked {
                    ui.colored_label(ui.visuals().error_fg_color, why);
                }
                let commands: Vec<String> = cherry_pick::commands(&self.cherry_pick())
                    .iter()
                    .map(|c| command_text(c))
                    .collect();
                dialogs::command_box(ui, &commands);
                let enabled = blocked.is_none() && !busy;
                dialogs::actions(ui, "Cherry-pick", enabled, false, false)
            });
        self.fresh = false;
        asked.answer = Some(if shown.should_close() {
            dialogs::Answer::Cancel
        } else {
            shown.inner
        });
        asked
    }

    /// The selected commits that can be picked.
    fn selected(&self) -> Vec<Oid> {
        self.list
            .many
            .iter()
            .map(|&i| self.repo.commit(self.commits[i]).oid)
            .filter(|&oid| self.preview.picked(oid).is_some())
            .collect()
    }

    fn set_picked(&mut self, picked: bool) {
        for oid in self.selected() {
            self.preview.set_picked(oid, picked);
        }
    }

    /// Whether the selected commits are all picked, or all dropped.
    fn selected_picked(&self) -> Option<bool> {
        let mut picked = self
            .selected()
            .into_iter()
            .filter_map(|o| self.preview.picked(o));
        let first = picked.next()?;
        picked.all(|p| p == first).then_some(first)
    }

    /// Pick and Drop for the selected commits, also by their keys (git's abbreviations in a
    /// todo list), and Ctrl+A to select them all.
    fn pick_buttons(&mut self, ui: &mut Ui) {
        let mut chosen = None;
        ui.input_mut(|i| {
            if i.consume_key(egui::Modifiers::COMMAND, egui::Key::A) {
                self.list.select_all(self.commits.len());
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::P) {
                chosen = Some(true);
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::D) {
                chosen = Some(false);
            }
        });
        let any = !self.selected().is_empty();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_enabled_ui(any, |ui| {
                for (picked, label, tip) in [
                    (true, "Pick", "Cherry-pick it (p)"),
                    (false, "Drop", "Leave it out (d)"),
                ] {
                    let (glyph, color, _) = pick_icon(ui, picked);
                    let button = crate::widgets::icon_text_button(ui, glyph, color, label)
                        .on_hover_text(tip)
                        .on_disabled_hover_text("Select commits first: Ctrl+ or Shift+click");
                    if button.clicked() {
                        chosen = Some(picked);
                    }
                }
            });
        });
        if let Some(picked) = chosen {
            self.set_picked(picked);
        }
    }

    /// The listed commits, as the log window lists them, `height` tall, with whether each is
    /// picked; those left out are greyed, with the reason on hover, as are those dropped.
    /// Returns the commit double-clicked.
    fn commits_table(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        options: &GraphOptions,
        height: f32,
    ) -> Option<Oid> {
        let c = log_window::colors(ui);
        let table = CommitTable {
            id: Id::new("cherry-pick-commits"),
            rows: self.commits.len(),
            graph: &self.graph,
            abbrev_len: self.repo.abbrev_len,
            palette,
            select: Select::Many,
            icons: true,
        };
        let (repo, commits, preview) = (&*self.repo, &self.commits, &self.preview);
        let refs = &self.refs;
        let shared = self.selected_picked();
        let icons = [pick_icon(ui, false), pick_icon(ui, true)];
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
                        let picked = preview.picked(commit.oid);
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
                            icon: picked.map(|p| icons[p as usize]),
                            greyed: picked != Some(true),
                            ..Row::default()
                        }
                    },
                    |ui, i, _| {
                        let pickable = preview.picked(repo.commit(commits[i]).oid).is_some();
                        ui.add_enabled_ui(pickable, |ui| {
                            for (picked, label, key) in [(true, "Pick", "P"), (false, "Drop", "D")]
                            {
                                let mark = crate::menu::Mark::Radio(shared == Some(picked));
                                if crate::menu::item(ui, label, key, mark).clicked() {
                                    chosen = Some(picked);
                                }
                            }
                        });
                    },
                    Some(&mut tip),
                )
            })
            .inner;
        if let Some(picked) = chosen {
            self.set_picked(picked);
        }
        if let Some(i) = clicks.icon {
            let oid = self.repo.commit(self.commits[i]).oid;
            if let Some(picked) = self.preview.picked(oid) {
                self.preview.set_picked(oid, !picked);
            }
        }
        clicks
            .double_clicked
            .map(|i| self.repo.commit(self.commits[i]).oid)
    }
}

/// The confirmation and the menus, driven frame by frame in a headless context against real,
/// disposable repositories: clicked by the text on screen, as a person would.
#[cfg(test)]
mod tests {
    use std::path::Path;

    use eframe::egui;
    use parterre_core::Oid;
    use parterre_core::cherry_pick::Picks;

    use super::super::branches::{self, Request};
    use super::super::tool_harness::{Harness, banner_texts, git, init, load, menu, read, write};

    fn commit(dir: &Path, path: &str, text: &str, message: &str) {
        write(dir, path, text);
        git(dir, &["add", path]);
        git(dir, &["commit", "-q", "-m", message]);
    }

    fn rev(dir: &Path, r: &str) -> Oid {
        Oid::from_hex(&git(dir, &["rev-parse", r])).unwrap()
    }

    /// main: base → fix → mine; up: base → the same fix → one → side work, merged → two.
    fn repository() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        init(p);
        commit(p, "file", "base\n", "base");
        git(p, &["branch", "up"]);
        commit(p, "fix", "fixed\n", "fix");
        commit(p, "mine", "mine\n", "mine");
        git(p, &["switch", "-q", "up"]);
        commit(p, "fix", "fixed\n", "the same fix");
        commit(p, "one", "one\n", "one");
        git(p, &["switch", "-q", "-c", "side"]);
        commit(p, "side", "side\n", "side work");
        git(p, &["switch", "-q", "up"]);
        git(p, &["merge", "-q", "--no-ff", "-m", "Merge side", "side"]);
        git(p, &["branch", "-q", "-D", "side"]);
        commit(p, "two", "two\n", "two");
        git(p, &["switch", "-q", "main"]);
        dir
    }

    fn open(h: &mut Harness) {
        let request = Request::CherryPick {
            picks: Picks::Lacking(h.rev("up")),
            name: Some("up".into()),
        };
        h.ask(request, "Cherry-pick up onto main");
    }

    /// The subjects on main since `mine`, newest first.
    fn picked(dir: &Path) -> Vec<String> {
        git(dir, &["log", "--format=%s", "mine_tip..main"])
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn tag_mine(dir: &Path) {
        git(dir, &["tag", "mine_tip", "main"]);
    }

    #[test]
    fn the_graph_offers_what_main_lacks_named_after_the_branch() {
        let dir = repository();
        let p = dir.path();
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
        let (texts, asked) = node("up", Some("Cherry-pick up onto main…"));
        assert!(
            texts.contains(&"Cherry-pick up onto main…".to_owned()),
            "{texts:?}"
        );
        match asked {
            Some(Request::CherryPick { picks, name }) => {
                assert_eq!(picks, Picks::Lacking(rev(p, "up")));
                assert_eq!(name.as_deref(), Some("up"));
            }
            other => panic!("expected a cherry-pick: {other:?}"),
        }
        // A commit with no branch: its short hash.
        let short = rev(p, "up~1").short(repo.abbrev_len.max(7));
        let item = format!("Cherry-pick {short} onto main…");
        assert!(node("up~1", None).0.contains(&item));
        // On main already: nothing to pick.
        let texts = node("main~1", None).0;
        assert!(
            !texts.iter().any(|t| t.starts_with("Cherry-pick")),
            "{texts:?}"
        );
    }

    #[test]
    fn the_log_offers_its_selection() {
        let dir = repository();
        let p = dir.path();
        let (repo, catalog) = load(p);
        let row = |selection: &[Oid], click: Option<&str>| {
            let (repo, catalog) = (&repo, &catalog);
            menu(
                move |ui| {
                    let commit = selection[0];
                    branches::row_node_menu(
                        ui,
                        repo,
                        commit,
                        selection,
                        Some(catalog),
                        false,
                        false,
                    )
                },
                click,
            )
        };
        let two = [rev(p, "up"), rev(p, "up~1^2")];
        let (texts, asked) = row(&two, Some("Cherry-pick 2 commits onto main…"));
        // Not the graph's: the branch's name isn't offered.
        assert!(
            !texts.contains(&"Cherry-pick up onto main…".to_owned()),
            "{texts:?}"
        );
        match asked {
            Some(Request::CherryPick { picks, name: None }) => {
                assert_eq!(picks, Picks::Chosen(two.to_vec()));
            }
            other => panic!("expected the selection: {other:?}"),
        }
        let one = [rev(p, "up")];
        let short = one[0].short(repo.abbrev_len.max(7));
        assert!(
            row(&one, None)
                .0
                .contains(&format!("Cherry-pick {short} onto main…"))
        );
        // All on main already.
        let on_main = [rev(p, "main~1")];
        assert!(
            !row(&on_main, None)
                .0
                .iter()
                .any(|t| t.starts_with("Cherry-pick"))
        );
    }

    #[test]
    fn the_confirmation_lists_the_commits_and_cherry_pick_runs_git() {
        let dir = repository();
        tag_mine(dir.path());
        let mut h = Harness::new(dir);
        open(&mut h);
        for subject in ["two", "Merge side", "side work", "one", "the same fix"] {
            assert!(h.shows(subject), "{subject}: {:?}", h.texts);
        }
        assert!(!h.shows("Stash changes"));
        h.click("Git command");
        let hex = |h: &Harness, r: &str| h.rev(r).to_hex();
        let command = format!(
            "git cherry-pick {} {} {}",
            hex(&h, "up~2"),
            hex(&h, "up~1^2"),
            hex(&h, "up")
        );
        // The box wraps it.
        let shown = h
            .texts
            .iter()
            .find(|(t, _)| t.starts_with("git cherry-pick"));
        let shown = shown.map(|(t, _)| t.replace('\n', ""));
        assert_eq!(shown, Some(command));
        h.click("Cherry-pick");
        h.until("the commits are picked", |h| picked(h.path()).len() == 3);
        assert_eq!(picked(h.path()), ["two", "side work", "one"]);
        h.until("the notification", |h| {
            h.shows("Cherry-pick up onto main") && !h.shows("Cancel")
        });
    }

    #[test]
    fn dropped_commits_are_left_out_and_dropping_all_is_refused() {
        let dir = repository();
        tag_mine(dir.path());
        let mut h = Harness::new(dir);
        open(&mut h);
        h.click("Git command");
        let side = h.rev("up~1^2").to_hex();
        h.click("side work");
        h.key(egui::Key::D);
        assert!(!h.shows_part(&side), "{:?}", h.texts);
        // All of them: nothing left to pick.
        h.modifiers = egui::Modifiers::COMMAND;
        h.key(egui::Key::A);
        h.modifiers = egui::Modifiers::NONE;
        h.click("Drop");
        assert!(h.shows("Pick at least one commit."));
        // Picked again by the menu's button, and one dropped by its icon.
        h.click("Pick");
        assert!(h.shows_part(&side));
        let left = h.texts.iter().find(|(t, _)| t == "at").unwrap().1.left();
        let at = egui::pos2(left + 12.0, h.at("one").y);
        h.click_at(at, 1);
        h.click("Cherry-pick");
        h.until("the commits are picked", |h| picked(h.path()).len() == 2);
        assert_eq!(picked(h.path()), ["two", "side work"]);
    }

    #[test]
    fn x_is_remembered() {
        let dir = repository();
        tag_mine(dir.path());
        let mut h = Harness::new(dir);
        open(&mut h);
        h.click("Git command");
        let label = "Append \"(cherry picked from commit …)\"";
        h.click(label);
        assert!(h.shows_part("git cherry-pick -x "), "{:?}", h.texts);
        h.click("Cancel");
        h.frame();
        open(&mut h);
        assert!(h.shows_part("git cherry-pick -x "), "{:?}", h.texts);
        h.click("Cherry-pick");
        h.until("the commits are picked", |h| picked(h.path()).len() == 3);
        let message = git(h.path(), &["log", "-1", "--format=%B", "main"]);
        assert!(message.contains("(cherry picked from commit"), "{message}");
    }

    #[test]
    fn uncommitted_changes_can_be_stashed_around_it() {
        let dir = repository();
        tag_mine(dir.path());
        write(dir.path(), "mine", "edited\n");
        let mut h = Harness::new(dir);
        open(&mut h);
        h.click("Git command");
        assert!(!h.shows_part("git stash"));
        h.click("Stash changes");
        assert!(
            h.shows("git stash push -m 'parterre: before cherry-pick'"),
            "{:?}",
            h.texts
        );
        assert!(h.shows("git stash pop"));
        h.click("Cherry-pick");
        h.until("the notification", |h| {
            h.shows("Cherry-pick up onto main") && !h.shows("Cancel")
        });
        assert_eq!(picked(h.path()).len(), 3);
        assert_eq!(read(h.path(), "mine"), "edited\n");
        assert_eq!(git(h.path(), &["stash", "list"]), "");
    }

    #[test]
    fn a_stopped_cherry_pick_shows_a_banner_and_greys_the_menus() {
        let dir = repository();
        let p = dir.path();
        // up's one now conflicts with main's mine.
        git(p, &["switch", "-q", "up"]);
        commit(p, "mine", "theirs\n", "their mine");
        commit(p, "three", "three\n", "three");
        git(p, &["switch", "-q", "main"]);
        let mut h = Harness::new(dir);
        let request = Request::CherryPick {
            picks: Picks::Chosen(vec![h.rev("up"), h.rev("up~1"), h.rev("up~2")]),
            name: None,
        };
        h.ask(request, "Cherry-pick 3 commits onto main");
        h.click("Cherry-pick");
        h.until("the orange notice", |h| {
            h.shows("Cherry-pick stopped on conflicts in 1 file")
        });
        let p = h.path();
        let texts = banner_texts(p);
        assert!(texts.contains(&"1 conflicted file".to_owned()), "{texts:?}");
        assert!(
            texts.contains(&"Cherry-pick stopped at 2/3".to_owned()),
            "{texts:?}"
        );

        let (repo, catalog) = load(p);
        let up = rev(p, "up");
        let (texts, asked) = menu(
            |ui| branches::node_menu(ui, &repo, up, &[up], Some(&catalog), false, false),
            Some("Cherry-pick up onto main…"),
        );
        assert!(
            texts.contains(&"Cherry-pick up onto main…".to_owned()),
            "{texts:?}"
        );
        assert!(asked.is_none(), "greyed out");
    }
}
