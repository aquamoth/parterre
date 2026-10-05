//! Reverting a commit on the open worktree's branch, from a log row's menu: the dialog, with the
//! commit, the message git words for the revert, and stashing uncommitted changes first. Then,
//! when parterre stashed them, the question whether to put them back.

use std::sync::Arc;

use eframe::egui::{self, Id, RichText, Ui, ViewportId, vec2};
use parterre_core::branches::{Stashed, command_text};
use parterre_core::revert::{self, Preview};
use parterre_core::{Oid, Repo};

use super::branches::caution;
use crate::dialogs;

/// The message field's id, to tell whether Enter is typed into it.
fn message_id() -> Id {
    Id::new("revert-message")
}

/// The message's rows as the dialog opens, and the fewest it shrinks to.
const MESSAGE_ROWS: usize = 4;
const MIN_MESSAGE_ROWS: usize = 2;

/// The message field's height for `rows` lines, with its margins.
fn message_height(ui: &Ui, rows: usize) -> f32 {
    rows as f32 * ui.text_style_height(&egui::TextStyle::Body) + 4.0
}

/// The message field, the dialog's growing part: `height` tall, scrolling past it, or as tall as
/// its lines. Returns that height of its own.
fn message_box(ui: &mut Ui, message: &mut String, height: Option<f32>) -> ((), f32) {
    let natural = message_height(ui, message.lines().count().max(MESSAGE_ROWS));
    let edit = egui::TextEdit::multiline(message)
        .id(message_id())
        .desired_rows(MESSAGE_ROWS)
        .desired_width(f32::INFINITY);
    match height {
        None => {
            let height = ui.add(edit).rect.height();
            ((), height)
        }
        Some(height) => {
            egui::ScrollArea::vertical()
                .id_salt("revert-message-lines")
                .min_scrolled_height(height)
                .max_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| ui.add(edit.min_size(vec2(0.0, height))));
            ((), natural)
        }
    }
}

/// The most the refused files take before they scroll: the message keeps the room.
const MAX_REFUSED: f32 = 120.0;

#[derive(Debug)]
pub struct RevertDialog {
    pub preview: Preview,
    repo: Arc<Repo>,
    pub message: String,
    pub stash: bool,
    /// The files git would refuse to revert over are listed.
    show_files: bool,
    /// The window it was asked from.
    pub opener: ViewportId,
    fresh: bool,
}

/// What the dialog asks for in a frame.
#[derive(Debug, Default)]
pub struct Asked {
    pub answer: Option<dialogs::Answer>,
    /// The commit's hash was clicked: show it in the log.
    pub log: Option<Oid>,
}

impl RevertDialog {
    pub fn new(preview: Preview, repo: Arc<Repo>, opener: ViewportId) -> Self {
        RevertDialog {
            message: preview.message.clone(),
            stash: preview.stashable(),
            preview,
            repo,
            show_files: false,
            opener,
            fresh: true,
        }
    }

    pub fn revert(&self) -> revert::Revert {
        self.preview.revert(self.stash, &self.message)
    }

    /// `busy` while another Git operation runs.
    pub fn show(&mut self, ctx: &egui::Context, busy: bool) -> Asked {
        let mut asked = Asked::default();
        let p = &self.preview;
        let short = p.commit.short(self.repo.abbrev_len.max(7));
        let title = format!("Revert {short} in {}", p.name());
        let shown = dialogs::Dialog::new("revert-commit", &title)
            .screen(crate::usage::Screen::Revert)
            .width(520.0)
            .opener(self.opener)
            .raise(self.fresh)
            .resizable()
            .show(ctx, |ui| {
                let p = &self.preview;
                if let Some(ix) = self.repo.lookup(&p.commit)
                    && dialogs::commit_line(ui, self.repo.commit(ix), self.repo.abbrev_len)
                {
                    asked.log = Some(p.commit);
                }
                ui.label("Message");
                let message = &mut self.message;
                let min = message_height(ui, MIN_MESSAGE_ROWS);
                dialogs::growing(ui, min, |ui, height| message_box(ui, message, height));
                ui.add_enabled_ui(p.stashable(), |ui| {
                    ui.checkbox(&mut self.stash, "Stash before revert")
                        .on_hover_text(
                            "Set your uncommitted changes aside in a stash entry first. \
                                 Afterwards parterre asks whether to put them back.",
                        );
                })
                .response
                .on_disabled_hover_text("There are no uncommitted changes");
                let stash = self.stash && p.stashable();
                if let Some(refusal) = p.refusal(stash) {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(ui.visuals().error_fg_color, refusal);
                        let label = if self.show_files { "Hide" } else { "Show" };
                        if ui.link(label).clicked() {
                            self.show_files = !self.show_files;
                        }
                    });
                    if self.show_files {
                        egui::ScrollArea::vertical()
                            .id_salt("revert-refused")
                            .max_height(MAX_REFUSED)
                            .show(ui, |ui| {
                                for file in p.refused(stash) {
                                    ui.label(RichText::new(file).monospace().small());
                                }
                            });
                    }
                } else if let Some(caution_text) = p.caution(stash) {
                    caution(ui, |ui| {
                        ui.label(caution_text);
                    });
                }
                if let Some(why) = p.blocked(stash, &self.message)
                    && p.refusal(stash).is_none()
                {
                    ui.colored_label(ui.visuals().error_fg_color, why);
                }
                let revert = self.revert();
                let commands: Vec<String> = revert::commands(&revert)
                    .iter()
                    .map(|c| command_text(c))
                    .collect();
                dialogs::command_box(ui, &commands);
                let stash = self.stash && self.preview.stashable();
                let enabled = self.preview.blocked(stash, &self.message).is_none() && !busy;
                let answer = dialogs::actions(ui, "Revert", enabled, false, false);
                // Enter runs it, but in the message it's a new line.
                let typing = ui.memory(|m| m.has_focus(message_id()));
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                if answer == dialogs::Answer::Open && enter && !typing && enabled {
                    dialogs::Answer::Primary
                } else {
                    answer
                }
            });
        self.fresh = false;
        asked.answer = Some(if shown.should_close() {
            dialogs::Answer::Cancel
        } else {
            shown.inner
        });
        asked
    }
}

/// After a revert parterre stashed changes for: put them back, or keep them in the stash.
#[derive(Debug)]
pub struct RestoreDialog {
    pub stash: Stashed,
    pub opener: ViewportId,
    fresh: bool,
}

impl RestoreDialog {
    pub fn new(stash: Stashed, opener: ViewportId) -> Self {
        RestoreDialog {
            stash,
            opener,
            fresh: true,
        }
    }

    /// *Primary* restores them (Enter), *Cancel* keeps them stashed (Esc).
    pub fn show(&mut self, ctx: &egui::Context, busy: bool) -> dialogs::Answer {
        let shown = dialogs::Dialog::new("restore-stash", "Restore stashed changes?")
            .screen(crate::usage::Screen::RestoreStash)
            .modal()
            .resizable()
            .opener(self.opener)
            .raise(self.fresh)
            .show(ctx, |ui| {
                dialogs::fields(ui, |ui| {
                    for file in &self.stash.files {
                        ui.label(RichText::new(file).monospace());
                    }
                });
                let pop = ["stash", "pop", &self.stash.name].map(str::to_owned);
                dialogs::command_box(ui, &[command_text(&pop)]);
                let answer = keep_stashed(ui, !busy);
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                if answer == dialogs::Answer::Open && enter && !busy {
                    dialogs::Answer::Primary
                } else {
                    answer
                }
            });
        self.fresh = false;
        if shown.should_close() {
            dialogs::Answer::Cancel
        } else {
            shown.inner
        }
    }
}

/// *Restore* and *Keep stashed*, as [`dialogs::actions`] lays them out.
fn keep_stashed(ui: &mut egui::Ui, enabled: bool) -> dialogs::Answer {
    let mut answer = dialogs::Answer::Open;
    let size = egui::vec2(ui.available_width(), 34.0);
    ui.allocate_ui_with_layout(
        size,
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            let restore = ui
                .add_enabled_ui(enabled, |ui| {
                    crate::widgets::primary_button(ui, "Restore", 90.0)
                })
                .inner;
            if restore.clicked() {
                answer = dialogs::Answer::Primary;
            }
            let keep = crate::widgets::text_button(ui, "Keep stashed");
            if keep.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                answer = dialogs::Answer::Cancel;
            }
        },
    );
    answer
}

/// The dialogs and the menu item, driven frame by frame in a headless context against real,
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

    /// main: base → change file → later, and side, off base.
    fn repository() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        // Parterre's own git reads the identity from the repository: CI has no global one.
        git(p, &["config", "user.name", "Test"]);
        git(p, &["config", "user.email", "test@example.com"]);
        commit(p, "file", "one\n", "base");
        commit(p, "other", "other\n", "other");
        git(p, &["branch", "side"]);
        commit(p, "file", "two\n", "change file");
        commit(p, "later", "later\n", "later");
        dir
    }

    fn rev(dir: &Path, r: &str) -> Oid {
        Oid::from_hex(&git(dir, &["rev-parse", r])).unwrap()
    }

    fn short(h: &Harness, r: &str) -> String {
        h.rev(r).short(h.repo.abbrev_len.max(7))
    }

    fn open(h: &mut Harness) {
        let title = format!("Revert {} in main", short(h, "main~1"));
        h.ask(
            Request::Revert {
                commit: h.rev("main~1"),
            },
            &title,
        );
    }

    fn row_menu(dir: &Path, at: &str, log: bool) -> Vec<String> {
        let (repo, catalog) = load(dir);
        let commit = rev(dir, at);
        menu(
            |ui| {
                if log {
                    branches::row_node_menu(
                        ui,
                        &repo,
                        commit,
                        &[commit],
                        Some(&catalog),
                        false,
                        false,
                    )
                } else {
                    branches::node_menu(ui, &repo, commit, &[commit], Some(&catalog), false, false)
                }
            },
            None,
        )
        .0
    }

    #[test]
    fn only_the_log_offers_it_for_commits_on_the_branch() {
        let dir = repository();
        let p = dir.path();
        let item = "Revert in main…".to_owned();
        assert!(row_menu(p, "main~1", true).contains(&item));
        assert!(row_menu(p, "main", true).contains(&item));
        assert!(
            !row_menu(p, "main~1", false).contains(&item),
            "not the graph"
        );
        git(p, &["switch", "-q", "side"]);
        commit(p, "side", "side\n", "side");
        assert!(
            row_menu(p, "main~1", true)
                .iter()
                .all(|t| !t.starts_with("Revert"))
        );
        let detached = "Revert in HEAD…".to_owned();
        git(p, &["switch", "-q", "--detach", "main"]);
        assert!(row_menu(p, "main~1", true).contains(&detached));
    }

    #[test]
    fn revert_runs_git_and_leaves_the_log_to_show_it() {
        let mut h = Harness::new(repository());
        let tip = h.rev("main");
        open(&mut h);
        assert!(h.shows("Message"));
        assert!(h.shows_part("Revert \"change file\""), "{:?}", h.texts);
        h.click("Git command");
        let full = h.rev("main~1").to_hex();
        assert!(
            h.shows(&format!("git revert --no-edit {full}")),
            "{:?}",
            h.texts
        );
        h.click("Revert");
        h.until("the revert", |h| h.rev("main~1") == tip);
        assert_eq!(read(h.path(), "file"), "one\n");
        h.until("done", |h| h.tool.reverted.is_some());
        let reverted = h.tool.reverted.as_ref().unwrap();
        assert_eq!((reverted.from, reverted.to), (tip, h.rev("main")));
        for _ in 0..5 {
            h.frame();
        }
        assert!(!h.shows_part("Revert "), "no notification: {:?}", h.texts);
    }

    #[test]
    fn the_message_is_the_one_typed() {
        let mut h = Harness::new(repository());
        open(&mut h);
        let field = h
            .texts
            .iter()
            .find(|(t, _)| t.starts_with("Revert \"change file\""))
            .expect("the message")
            .1
            .center();
        h.click_at(field, 1);
        h.modifiers = eframe::egui::Modifiers::COMMAND;
        h.key(eframe::egui::Key::A);
        h.modifiers = eframe::egui::Modifiers::NONE;
        h.type_text("Undo the change to file");
        h.click("Git command");
        let full = h.rev("main~1").to_hex();
        assert!(
            h.shows(&format!("git revert --edit {full}")),
            "{:?}",
            h.texts
        );
        h.click("Revert");
        h.until("the revert", |h| h.tool.reverted.is_some());
        assert_eq!(
            git(h.path(), &["log", "-1", "--format=%B"]),
            "Undo the change to file"
        );
    }

    #[test]
    fn without_changes_there_is_nothing_to_stash() {
        let mut h = Harness::new(repository());
        open(&mut h);
        assert!(h.shows("Stash before revert"));
        h.click("Stash before revert");
        h.click("Git command");
        assert!(!h.shows_part("git stash push"), "{:?}", h.texts);
    }

    #[test]
    fn changes_the_revert_touches_are_stashed_and_restored_on_request() {
        let dir = repository();
        write(dir.path(), "file", "mine\n");
        write(dir.path(), "other", "also mine\n");
        let mut h = Harness::new(dir);
        open(&mut h);
        h.click("Git command");
        assert!(h.shows_part("git stash push -m"), "ticked: {:?}", h.texts);
        // Unticked, git would refuse.
        h.click("Stash before revert");
        let refusal = format!("{} changes 1 file with local changes", short(&h, "main~1"));
        assert!(h.shows(&refusal), "{:?}", h.texts);
        h.click("Show");
        assert!(h.shows("file"));
        let tip = h.rev("main");
        h.click("Revert");
        for _ in 0..10 {
            h.frame();
        }
        assert_eq!(h.rev("main"), tip, "Revert is greyed out");
        h.click("Stash before revert");
        assert!(!h.shows(&refusal));
        h.click("Revert");
        h.until("the question", |h| h.shows("Restore stashed changes?"));
        assert!(h.shows("other"), "{:?}", h.texts);
        assert_eq!(read(h.path(), "other"), "other\n");
        h.click("Restore");
        // The file the revert changed conflicts: git says so, and keeps the entry. The notice
        // waits for git, which removes each file it writes back before writing it (#217).
        h.until("the orange notice", |h| {
            h.shows("Restoring stashed changes conflicted in 1 file")
        });
        assert_eq!(read(h.path(), "other"), "also mine\n");
        assert_eq!(git(h.path(), &["stash", "list"]).lines().count(), 1);
    }

    #[test]
    fn keep_stashed_leaves_the_entry() {
        let dir = repository();
        write(dir.path(), "other", "mine\n");
        let mut h = Harness::new(dir);
        open(&mut h);
        h.click("Revert");
        h.until("the question", |h| h.shows("Restore stashed changes?"));
        h.click("Keep stashed");
        for _ in 0..5 {
            h.frame();
        }
        assert!(!h.shows("Restore stashed changes?"), "{:?}", h.texts);
        assert_eq!(read(h.path(), "other"), "other\n");
        assert!(git(h.path(), &["stash", "list"]).contains("autostash for reverting"));
    }

    #[test]
    fn unstashed_changes_elsewhere_are_a_caution() {
        let dir = repository();
        write(dir.path(), "other", "mine\n");
        let mut h = Harness::new(dir);
        open(&mut h);
        h.click("Stash before revert");
        assert!(
            h.shows("1 file with local changes stays in the worktree"),
            "{:?}",
            h.texts
        );
        h.click("Revert");
        h.until("the revert", |h| h.tool.reverted.is_some());
        assert_eq!(read(h.path(), "other"), "mine\n");
        assert!(!h.shows("Restore stashed changes?"));
    }

    #[test]
    fn a_conflict_says_so_and_names_the_stash() {
        let dir = repository();
        let p = dir.path();
        commit(p, "file", "three\n", "change file again");
        write(p, "other", "mine\n");
        let mut h = Harness::new(dir);
        let reverted = h.rev("main~2");
        let title = format!("Revert {} in main", short(&h, "main~2"));
        h.ask(Request::Revert { commit: reverted }, &title);
        h.click("Revert");
        h.until("the orange notice", |h| {
            h.shows("Revert stopped on conflicts in 1 file")
        });
        assert!(!h.shows("Restore stashed changes?"));
        let (_, catalog) = load(h.path());
        assert_eq!(catalog.stuck(), Some(Stuck::InProgress("a revert")));
        let texts = banner_texts(h.path());
        assert!(texts.contains(&"1 conflicted file".to_owned()), "{texts:?}");
        assert!(
            texts.contains(&"Your changes are stashed in stash@{0}.".to_owned()),
            "{texts:?}"
        );
        // Greyed out until it's finished with git.
        let menu_texts = row_menu(h.path(), "main~1", true);
        assert!(
            menu_texts.contains(&"Revert in main…".to_owned()),
            "{menu_texts:?}"
        );
    }

    #[test]
    fn nothing_to_revert_says_so() {
        let dir = repository();
        let p = dir.path();
        commit(p, "file", "one\n", "undo by hand");
        let mut h = Harness::new(dir);
        let title = format!("Revert {} in main", short(&h, "main~2"));
        h.ask(
            Request::Revert {
                commit: h.rev("main~2"),
            },
            &title,
        );
        h.click("Revert");
        let expected = format!(
            "Nothing to revert: {}'s changes are already undone",
            h.rev("main~2").short(7)
        );
        h.until("the orange notice", |h| h.shows(&expected));
        assert_eq!(load(h.path()).1.stuck(), None);
    }
}
