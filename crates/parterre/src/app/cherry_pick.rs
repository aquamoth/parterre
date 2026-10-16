//! The confirmation lists the commits newest first, as the log window does: they go on from
//! the bottom up.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, Id, Key, Ui, ViewportId};
use parterre_core::branches::command_text;
use parterre_core::cherry_pick::{self, Preview};
use parterre_core::glyphs;
use parterre_core::log_graph::LogGraph;
use parterre_core::revgraph::GraphOptions;
use parterre_core::{CommitIx, Oid, Repo};

use super::branches::Asked;
use super::commit_table::Row;
use super::rebase::{
    Choice, ListedCommits, choice_buttons, choice_menu, icon_of, listing_dialog_width, shared,
    stash_checkbox,
};
use crate::dialogs;
use crate::theme::Palette;

/// Where the last choice of `-x` is remembered.
fn record_origin_id() -> Id {
    Id::new("cherry-pick-record-origin")
}

#[derive(Debug)]
pub struct CherryPickDialog {
    preview: Preview,
    record_origin: bool,
    stash: bool,
    pub opener: ViewportId,
    fresh: bool,
    listed: ListedCommits,
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
        let graph = LogGraph::from_parents(parents, outside);
        CherryPickDialog {
            record_origin: ctx.data_mut(|d| *d.get_persisted_mut_or(record_origin_id(), false)),
            stash: false,
            listed: ListedCommits::new(repo, Id::new("cherry-pick-commits"), commits, graph),
            preview,
            opener,
            fresh: true,
        }
    }

    pub fn cherry_pick(&self) -> cherry_pick::CherryPick {
        self.preview.cherry_pick(self.record_origin, self.stash)
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        busy: bool,
        palette: &Palette,
        options: &GraphOptions,
    ) -> Asked {
        let pick = self.cherry_pick();
        let title = format!("Cherry-pick {} onto {}", pick.name, pick.branch);
        let mut show_in_log = None;
        let shown = dialogs::Dialog::new("cherry-pick", &title)
            .screen(crate::usage::Screen::CherryPick)
            .width(listing_dialog_width(ctx))
            .opener(self.opener)
            .raise(self.fresh)
            .resizable()
            .show(ctx, |ui| {
                // Only the commits scroll: everything else stays in view.
                if dialogs::commit_line(ui, self.listed.repo(), self.preview.head) {
                    show_in_log = Some(self.preview.head);
                }
                if let Some(oid) = self.commits_table(ui, palette, options) {
                    show_in_log = Some(oid);
                }
                self.choose(ui);
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
                    stash_checkbox(ui, &mut self.stash, "git stash");
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
                let primary =
                    dialogs::Primary::new("Cherry-pick").enabled(blocked.is_none() && !busy);
                dialogs::actions(ui, primary)
            });
        self.fresh = false;
        Asked {
            answer: shown.answer(),
            show_in_log,
        }
    }

    /// The selected commits that can be picked.
    fn selected(&self) -> Vec<Oid> {
        self.listed
            .selected()
            .filter(|&oid| self.preview.picked(oid).is_some())
            .collect()
    }

    fn set_picked(&mut self, picked: bool) {
        for oid in self.selected() {
            self.preview.set_picked(oid, picked);
        }
    }

    /// Pick and Drop, as git's todo list abbreviates them.
    fn choices(ui: &Ui) -> [Choice<bool>; 2] {
        [
            Choice {
                value: true,
                glyph: glyphs::PICK,
                color: ui.visuals().text_color(),
                word: "pick",
                key: Key::P,
                tip: "Cherry-pick it (p)",
                unavailable: None,
            },
            Choice {
                value: false,
                glyph: glyphs::DROP,
                color: ui.visuals().error_fg_color,
                word: "drop",
                key: Key::D,
                tip: "Leave it out (d)",
                unavailable: None,
            },
        ]
    }

    fn choose(&mut self, ui: &mut Ui) {
        self.listed.select_all_shortcut(ui);
        let any = !self.selected().is_empty();
        if let Some(picked) = choice_buttons(ui, any, &Self::choices(ui)) {
            self.set_picked(picked);
        }
    }

    /// The listed commits, with whether each is picked; those left out are greyed, as are those
    /// dropped. Returns the commit double-clicked.
    fn commits_table(
        &mut self,
        ui: &mut Ui,
        palette: &Palette,
        options: &GraphOptions,
    ) -> Option<Oid> {
        let choices = Self::choices(ui);
        let shared_pick = shared(
            self.selected()
                .into_iter()
                .filter_map(|o| self.preview.picked(o)),
        );
        let preview = &self.preview;
        let mut chosen = None;
        let clicked = self.listed.show_choosing(
            ui,
            palette,
            options,
            |oid, row| {
                let picked = preview.picked(oid);
                Row {
                    icon: picked.and_then(|p| icon_of(&choices, p)),
                    greyed: picked != Some(true),
                    ..row
                }
            },
            |ui, oid| {
                let pickable = preview.picked(oid).is_some();
                if let Some(picked) = choice_menu(ui, pickable, shared_pick, &choices) {
                    chosen = Some(picked);
                }
            },
            |oid| preview.skipped(oid).map(|s| s.reason()),
        );
        if let Some(picked) = chosen {
            self.set_picked(picked);
        }
        if let Some(oid) = clicked.icon
            && let Some(picked) = self.preview.picked(oid)
        {
            self.preview.set_picked(oid, !picked);
        }
        clicked.double_clicked
    }
}

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
        // As the Git menu, and so the graph's, offers it.
        let offer = |at: &str| {
            let commit = rev(p, at);
            let refs = branches::refs_at(&repo, commit, &catalog);
            branches::cherry_pick_offer(&repo, commit, &refs, &catalog)
        };
        let (name, branch, asked) = offer("up").expect("offered");
        assert_eq!((name.as_str(), branch.as_str()), ("up", "main"));
        match asked {
            Request::CherryPick { picks, name } => {
                assert_eq!(picks, Picks::Lacking(rev(p, "up")));
                assert_eq!(name.as_deref(), Some("up"));
            }
            other => panic!("expected a cherry-pick: {other:?}"),
        }
        // A commit with no branch: its short hash.
        let short = rev(p, "up~1").short(repo.abbrev_len.max(7));
        assert_eq!(offer("up~1").expect("offered").0, short);
        // On main already: nothing to pick.
        assert!(offer("main~1").is_none());
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

    /// The commits are one stop for Tab: a click on one gives the table the keyboard, Down
    /// moves in it, and Tab leaves it at once rather than row by row.
    #[test]
    fn the_commits_are_one_stop_for_tab() {
        let mut h = Harness::new(repository());
        open(&mut h);
        let table = Some(egui::Id::new("cherry-pick-commits").with("stop"));
        let focused = |h: &Harness| h.ctx.memory(|m| m.focused());
        h.click("one");
        assert_eq!(focused(&h), table);
        h.key(egui::Key::ArrowDown);
        assert_eq!(focused(&h), table, "Down stays in the table");
        h.key(egui::Key::Tab);
        assert!(focused(&h).is_some() && focused(&h) != table);
    }

    /// Shift+F10 (the context-menu key's stand-in) opens the selected commit's menu, with the
    /// keyboard on its first item; Esc gives the keyboard back to the table.
    #[test]
    fn the_context_menu_key_opens_the_selected_commits_menu() {
        let mut h = Harness::new(repository());
        open(&mut h);
        let table = Some(egui::Id::new("cherry-pick-commits").with("stop"));
        let focused = |h: &Harness| h.ctx.memory(|m| m.focused());
        // The dialog's own buttons say Pick and Drop too; the menu's are more of them.
        let count = |h: &Harness, text: &str| h.texts.iter().filter(|(t, _)| t == text).count();
        h.click("one");
        let (picks, drops) = (count(&h, "Pick"), count(&h, "Drop"));
        h.modifiers = egui::Modifiers::SHIFT;
        h.key(egui::Key::F10);
        h.modifiers = egui::Modifiers::NONE;
        assert_eq!(count(&h, "Pick"), picks + 1, "{:?}", h.texts);
        assert_eq!(count(&h, "Drop"), drops + 1);
        let item = h.texts.iter().rev().find(|(t, _)| t == "Pick").unwrap().1;
        let on_pick = focused(&h)
            .and_then(|id| h.ctx.read_response(id))
            .is_some_and(|r| r.rect.contains(item.center()));
        assert!(on_pick, "the keyboard is on the first item");
        h.key(egui::Key::Escape);
        assert_eq!(count(&h, "Drop"), drops, "Esc closes it");
        assert_eq!(focused(&h), table, "and the table has the keyboard again");
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
        let item = format!(
            "Cherry-pick {} onto main…",
            up.short(repo.abbrev_len.max(7))
        );
        let (texts, asked) = menu(
            |ui| branches::row_node_menu(ui, &repo, up, &[up], Some(&catalog), false, false),
            Some(&item),
        );
        assert!(texts.contains(&item), "{texts:?}");
        assert!(asked.is_none(), "greyed out");
    }
}
