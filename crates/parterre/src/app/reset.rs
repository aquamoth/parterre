//! On the left, the files the reset concerns as the picked mode leaves them, in the log window's
//! changed-files table; on the right, what happens to the branch, git's modes, what's lost and
//! the command.

use std::sync::Arc;

use eframe::egui::{self, Id, RichText, Ui, ViewportId, vec2};
use parterre_core::branches::command_text;
use parterre_core::file_diff::FileDiffSpec;
use parterre_core::reset::{self, FileOutcome, Mode, Preview};
use parterre_core::text::plural;
use parterre_core::{Oid, Repo};

use super::file_table::{FileTable, StatusBadge, StatusBadges};
use super::table_style::{self, Colors};
use crate::{dialogs, widgets};

const TABLE_INITIAL_WIDTH: f32 = 520.0;
const TABLE_MIN_WIDTH: f32 = 240.0;
/// Between the files table and the side, with the divider in it.
const GAP_WIDTH: f32 = 40.0;
const SIDE_WIDTH: f32 = 420.0;
const MODE_NAME_WIDTH: f32 = 70.0;

/// Remembered as the Git command section is.
fn files_shown_id() -> Id {
    Id::new("reset-files-shown")
}

/// How the side is laid out: its width, and the room left above its Git command, which keeps
/// the buttons at the window's bottom.
#[derive(Clone, Copy, Debug)]
struct Side {
    width: f32,
    room: f32,
}

#[derive(Debug)]
pub struct ResetDialog {
    pub preview: Arc<Preview>,
    repo: Arc<Repo>,
    pub mode: Mode,
    pub opener: ViewportId,
    fresh: bool,
    /// Asked for again while open: left out for a frame, so its window opens anew in front.
    reopen: bool,
    table: FileTable,
}

/// What the dialog asks for in a frame.
#[derive(Debug)]
pub struct Asked {
    pub answer: dialogs::Answer,
    /// The log of these commits: the lost ones (exactly them), or the target.
    pub log: Option<(Vec<Oid>, bool)>,
    pub diffs: Vec<FileDiffSpec>,
}

/// What the side's controls are set to, or asked for, in a frame.
struct SideState {
    mode: Mode,
    log: Option<(Vec<Oid>, bool)>,
    toggle_files: bool,
}

impl SideState {
    fn new(mode: Mode) -> Self {
        SideState {
            mode,
            log: None,
            toggle_files: false,
        }
    }
}

impl ResetDialog {
    /// `reopen` when one is open already, to bring it to the front.
    pub fn new(preview: Preview, repo: Arc<Repo>, opener: ViewportId, reopen: bool) -> Self {
        ResetDialog {
            mode: preview.default_mode(),
            preview: Arc::new(preview),
            repo,
            opener,
            fresh: true,
            reopen,
            table: FileTable::default(),
        }
    }

    /// The same reset read again, after the repository changed: the mode stays.
    pub fn refresh(&mut self, preview: Preview, repo: Arc<Repo>) {
        self.preview = Arc::new(preview);
        self.repo = repo;
    }

    pub fn show(&mut self, ctx: &egui::Context, busy: bool) -> Asked {
        let mut diffs = Vec::new();
        if std::mem::take(&mut self.reopen) {
            ctx.request_repaint();
            return Asked {
                answer: dialogs::Answer::Open,
                log: None,
                diffs,
            };
        }
        let abbrev = self.repo.abbrev_len.max(7);
        let title = format!(
            "Reset {} to {}",
            self.preview.branch,
            self.preview.target.short(abbrev)
        );
        let files_shown = ctx.data_mut(|d| *d.get_persisted_mut_or(files_shown_id(), true));
        let width = if files_shown {
            TABLE_INITIAL_WIDTH + GAP_WIDTH + SIDE_WIDTH
        } else {
            SIDE_WIDTH
        };
        let mut state = SideState::new(self.mode);
        let shown = dialogs::Dialog::new("reset-branch", &title)
            .screen(crate::usage::Screen::Reset)
            .width(width)
            .opener(self.opener)
            .raise(self.fresh)
            .resizable()
            // A width with the files pane, and another without.
            .remember_as(("reset-branch", files_shown))
            .show(ctx, |ui| {
                // Before the table takes Enter to open the files picked in it.
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                if !files_shown {
                    let side = Side {
                        width: ui.available_width(),
                        room: 0.0,
                    };
                    let answer = self.side(ui, busy, &mut state, side);
                    return self.enter(answer, enter, false, state.mode, busy);
                }
                // The side laid out unseen first: the files pane is at least as tall.
                let mut sizer = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt("reset-sizing")
                        .max_rect(ui.available_rect_before_wrap())
                        .layout(egui::Layout::top_down(egui::Align::Min))
                        .sizing_pass()
                        .invisible(),
                );
                let side = Side {
                    width: SIDE_WIDTH,
                    room: 0.0,
                };
                self.side(&mut sizer, busy, &mut SideState::new(state.mode), side);
                let natural = sizer.min_rect().height();
                // The files pane grows; the side keeps its buttons at the bottom.
                dialogs::growing(ui, natural, |ui, height| {
                    let height = height.unwrap_or(natural).max(natural);
                    let answer = ui
                        .horizontal_top(|ui| {
                            // The files pane's background, painted once the row's height is known.
                            let background = [(); 2].map(|_| ui.painter().add(egui::Shape::Noop));
                            let table_width = (ui.available_width() - GAP_WIDTH - SIDE_WIDTH)
                                .max(TABLE_MIN_WIDTH);
                            let (mut pane, opened) =
                                self.files_pane(ui, state.mode, height, table_width);
                            let opened_any = !opened.is_empty();
                            diffs.extend(opened);
                            ui.add_space(GAP_WIDTH);
                            let side = Side {
                                width: SIDE_WIDTH,
                                room: height - natural,
                            };
                            let side = ui.vertical(|ui| self.side(ui, busy, &mut state, side));
                            // To the window's bottom edge, which the taller side decides.
                            pane.max.y = pane
                                .max
                                .y
                                .max(side.response.rect.bottom() + dialogs::MARGIN);
                            let c = table_style::colors(ui);
                            let line = egui::Stroke::new(1.0, c.line);
                            ui.painter()
                                .set(background[0], egui::Shape::rect_filled(pane, 0.0, c.pane));
                            ui.painter().set(
                                background[1],
                                egui::Shape::vline(pane.right(), pane.y_range(), line),
                            );
                            self.enter(side.inner, enter, opened_any, state.mode, busy)
                        })
                        .inner;
                    (answer, natural)
                })
            });
        self.fresh = false;
        self.mode = state.mode;
        if state.toggle_files {
            ctx.data_mut(|d| d.insert_persisted(files_shown_id(), !files_shown));
        }
        Asked {
            answer: shown.answer(),
            log: state.log,
            diffs,
        }
    }

    /// Enter runs a reset that loses nothing, unless it opened the files picked in the table.
    fn enter(
        &self,
        answer: dialogs::Answer,
        enter: bool,
        opened: bool,
        mode: Mode,
        busy: bool,
    ) -> dialogs::Answer {
        let preview = &self.preview;
        let runs = preview.refusal(mode).is_none() && !busy;
        if answer == dialogs::Answer::Open && enter && !opened && !preview.loses(mode) && runs {
            dialogs::Answer::Primary
        } else {
            answer
        }
    }

    /// The fields, the Git command, and the buttons' row with *Hide files*.
    fn side(
        &self,
        ui: &mut Ui,
        busy: bool,
        state: &mut SideState,
        layout: Side,
    ) -> dialogs::Answer {
        ui.set_width(layout.width);
        let preview = &self.preview;
        side_fields(ui, preview, &self.repo, state);
        ui.add_space(layout.room);
        let command = reset::command(state.mode, preview.target);
        dialogs::command_box(ui, &[command_text(&command)]);
        let loses = preview.loses(state.mode);
        let runs = preview.refusal(state.mode).is_none() && !busy;
        let label = if loses { "Reset anyway" } else { "Reset" };
        let files_shown = ui.data_mut(|d| *d.get_persisted_mut_or(files_shown_id(), true));
        ui.horizontal(|ui| {
            let fold = if files_shown {
                "Hide files"
            } else {
                "Show files"
            };
            // As tall as the row `actions` lays out, to line up with Cancel.
            state.toggle_files |= ui
                .allocate_ui_with_layout(
                    vec2(0.0, 34.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| widgets::text_button(ui, fold),
                )
                .inner
                .clicked();
            let primary = dialogs::Primary::new(label)
                .enabled(runs)
                .destructive(loses)
                .focus_cancel(self.fresh && loses);
            dialogs::actions(ui, primary)
        })
        .inner
    }

    /// The files, as a pane of the log window, `width` wide: to the window's edges, then a
    /// divider. Returns the pane, for its background, and the diffs a double-click (or Enter)
    /// asks for.
    fn files_pane(
        &mut self,
        ui: &mut Ui,
        mode: Mode,
        height: f32,
        width: f32,
    ) -> (egui::Rect, Vec<FileDiffSpec>) {
        let top = ui.cursor().top();
        let (slot, _) = ui.allocate_exact_size(vec2(width, height), egui::Sense::hover());
        let pane = egui::Rect::from_min_max(
            egui::pos2(slot.left() - dialogs::MARGIN, top - dialogs::MARGIN),
            egui::pos2(
                slot.right() + GAP_WIDTH / 2.0,
                top + height + dialogs::MARGIN,
            ),
        );
        let c = table_style::colors(ui);
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(pane)
                .id_salt("reset-files-pane")
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(pane);
        let preview = &self.preview;
        let outcomes = preview.files(mode);
        let files: Vec<_> = outcomes.iter().map(|o| preview.changed_file(o)).collect();
        let badges: Vec<_> = outcomes.iter().map(|o| badges(o, &c)).collect();
        let action = self.table.show_badged(
            &mut child,
            "reset-branch",
            Id::new("reset-branch-files"),
            &files,
            &badges,
            |_| {},
        );
        let diffs = action
            .open
            .into_iter()
            .filter_map(|f| outcomes.iter().find(|o| o.path == f.path))
            .map(|o| preview.diff(o))
            .collect();
        (pane, diffs)
    }
}

/// The side's fields: the target commit, what happens to the branch, the modes with their
/// help, git's refusal and the lost commits.
fn side_fields(ui: &mut Ui, preview: &Preview, repo: &Repo, state: &mut SideState) {
    let abbrev = repo.abbrev_len.max(7);
    dialogs::fields(ui, |ui| {
        if dialogs::commit_line(ui, repo, preview.target) {
            state.log = Some((vec![preview.target], false));
        }
        ui.label(preview.movement(abbrev));
        ui.add_space(4.0);
        for m in Mode::ALL {
            let help = preview.help(m, abbrev);
            if dialogs::radio_with_help(
                ui,
                state.mode == m,
                m.name(),
                MODE_NAME_WIDTH,
                m.flag(),
                help,
            ) {
                state.mode = m;
            }
        }
        let red = ui.visuals().error_fg_color;
        if let Some(refusal) = preview.blocked() {
            ui.add_space(4.0);
            ui.label(RichText::new(refusal).color(red));
        } else if let Some(refusal) = preview.refusal(state.mode) {
            ui.add_space(4.0);
            let refuses = format!("git reset {} refuses:", state.mode.flag());
            ui.label(RichText::new(refuses).color(red));
            ui.label(RichText::new(refusal).monospace().small().color(red));
        }
        if !preview.lost_commits.is_empty() {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let lost = plural(preview.lost_commits.len(), "commit");
                let text = format!("Lost: {lost}, on no branch, tag or worktree afterwards.");
                ui.label(RichText::new(text).color(red));
                if ui.link("Show in log").clicked() {
                    state.log = Some((preview.lost_commits.clone(), true));
                }
            });
        }
    });
}

/// A file's Status column: what it is afterwards, as `git status --short` letters (filled when
/// staged, outlined when on disk only, U untracked); a check mark for a file rewritten; a red
/// ! for lost work, or the file git refuses on.
fn badges(o: &FileOutcome, c: &Colors) -> StatusBadges {
    let color = |letter: char| match letter {
        'A' | 'U' | '✔' => c.added,
        'D' | '!' => c.removed,
        _ => c.modified,
    };
    let badge = |letter: char, filled: bool| StatusBadge {
        letter,
        filled,
        color: color(letter),
    };
    let mut badges = Vec::new();
    badges.extend(o.staged.map(|s| badge(s.letter(), true)));
    badges.extend(o.unstaged.map(|s| badge(s.letter(), false)));
    if o.untracked {
        badges.push(badge('U', false));
    }
    if badges.is_empty() && o.updated && o.lost.is_none() {
        badges.push(badge('✔', false));
    }
    if o.lost.is_some() || o.refused {
        badges.push(badge('!', true));
    }
    StatusBadges {
        badges,
        words: o.words(),
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui;
    use parterre_core::Oid;

    use super::super::branches::Request;
    use super::super::tool_harness::{Harness, git, menu, read, run_frame, write};

    /// main: base (a.txt, lib.txt) → tip (a.txt changed), the tip also on `pushed`.
    fn repository() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        write(p, "a.txt", "one\n");
        write(p, "lib.txt", "lib\n");
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", "base"]);
        write(p, "a.txt", "two\n");
        git(p, &["commit", "-q", "-am", "tip"]);
        git(p, &["branch", "pushed"]);
        dir
    }

    /// Asks for the dialog, and waits for it.
    fn open(h: &mut Harness, target: Oid) -> String {
        let title = format!("Reset main to {}", target.short(h.repo.abbrev_len.max(7)));
        h.ask(Request::Reset { target, mode: None }, &title);
        title
    }

    #[test]
    fn every_mode_and_hiding_the_files_shows_without_id_clashes() {
        let dir = repository();
        write(dir.path(), "lib.txt", "S\n");
        git(dir.path(), &["add", "lib.txt"]);
        write(dir.path(), "lib.txt", "W\n");
        let mut h = Harness::new(dir);
        let base = h.rev("HEAD~1");
        open(&mut h, base);
        // Mixed and Keep lose lib.txt's staged version: Soft is picked.
        assert!(h.shows("Reset"));
        for (mode, button) in [
            ("Mixed", "Reset anyway"),
            ("Keep", "Reset anyway"),
            ("Hard", "Reset anyway"),
            ("Soft", "Reset"),
        ] {
            h.click(mode);
            assert!(h.shows(button), "{mode}: {:?}", h.texts);
        }
        assert!(h.shows("2 files"));
        h.click("Hide files");
        assert!(!h.shows("2 files") && h.shows("Show files"));
        h.click("Show files");
        assert!(h.shows("2 files"));
        h.click("Git command");
        assert!(h.shows_part("reset --soft"));
    }

    #[test]
    fn reset_runs_git_and_says_so() {
        let mut h = Harness::new(repository());
        let base = h.rev("HEAD~1");
        let title = open(&mut h, base);
        h.click("Reset");
        h.until("the branch moves", |h| h.rev("HEAD") == base);
        h.until("the notification", |h| {
            h.shows(&title) && !h.shows("Cancel")
        });
        assert_eq!(read(h.path(), "a.txt"), "one\n");
    }

    /// Usage statistics (#264): the dialog counts as it opens, the reset as it is started, each
    /// once however many frames they take, and by kind only.
    #[test]
    fn the_dialog_and_the_reset_are_counted_once_each() {
        use crate::usage::{Action, Feature, Screen};
        let mut h = Harness::new(repository());
        let base = h.rev("HEAD~1");
        let ((), recorded) = parterre_telemetry::recording(|| {
            open(&mut h, base);
            h.click("Reset");
            h.until("the branch moves", |h| h.rev("HEAD") == base);
            for _ in 0..5 {
                h.frame();
            }
        });
        assert_eq!(
            recorded,
            [
                Feature::Screen(Screen::Reset),
                Feature::Action(Action::Reset)
            ]
        );
    }

    #[test]
    fn enter_runs_a_reset_that_loses_nothing() {
        let mut h = Harness::new(repository());
        let base = h.rev("HEAD~1");
        open(&mut h, base);
        h.key(egui::Key::Enter);
        h.until("the branch moves", |h| h.rev("HEAD") == base);
    }

    #[test]
    fn enter_cancels_a_reset_that_loses_work() {
        let dir = repository();
        git(dir.path(), &["branch", "-D", "pushed"]);
        let mut h = Harness::new(dir);
        let (tip, base) = (h.rev("HEAD"), h.rev("HEAD~1"));
        let title = open(&mut h, base);
        assert!(h.shows("Reset anyway") && h.shows("Show in log"));
        h.key(egui::Key::Enter);
        assert!(!h.shows(&title));
        for _ in 0..20 {
            h.frame();
        }
        assert_eq!(h.rev("HEAD"), tip);
    }

    #[test]
    fn a_mode_git_refuses_cannot_be_run() {
        let dir = repository();
        write(dir.path(), "a.txt", "mine\n");
        let mut h = Harness::new(dir);
        let (tip, base) = (h.rev("HEAD"), h.rev("HEAD~1"));
        open(&mut h, base);
        h.click("Keep");
        assert!(h.shows("git reset --keep refuses:"), "{:?}", h.texts);
        h.click("Reset");
        h.key(egui::Key::Enter);
        for _ in 0..20 {
            h.frame();
        }
        assert_eq!(h.rev("HEAD"), tip);
        assert_eq!(read(h.path(), "a.txt"), "mine\n");
    }

    #[test]
    fn the_dialog_follows_the_repository() {
        let mut h = Harness::new(repository());
        let base = h.rev("HEAD~1");
        open(&mut h, base);
        assert!(h.shows("1 file"));
        write(h.path(), "lib.txt", "edited\n");
        h.reload();
        h.until("the edit is listed", |h| h.shows("2 files"));
        // Moved there from outside: nothing left to reset, and the dialog goes.
        git(h.path(), &["reset", "-q", "--keep", &base.to_hex()]);
        h.reload();
        h.until("the dialog closes", |h| !h.shows("Cancel"));
    }

    #[test]
    fn asking_again_keeps_one_dialog() {
        let mut h = Harness::new(repository());
        let base = h.rev("HEAD~1");
        let title = open(&mut h, base);
        open(&mut h, base);
        assert_eq!(h.texts.iter().filter(|(t, _)| *t == title).count(), 1);
    }

    #[test]
    fn files_open_their_diff_and_lost_commits_their_log() {
        let dir = repository();
        git(dir.path(), &["branch", "-D", "pushed"]);
        let mut h = Harness::new(dir);
        let (tip, base) = (h.rev("HEAD"), h.rev("HEAD~1"));
        open(&mut h, base);
        let at = h.at("a.txt");
        h.click_at(at, 2);
        let diffs = std::mem::take(&mut h.tool.diff_requests);
        assert_eq!(diffs.len(), 1, "a double-click opens one diff");
        assert_eq!(diffs[0].1.path(), "a.txt");
        h.click("Show in log");
        let (_, commits, exact) = h.tool.log_request.take().expect("the log is asked for");
        assert_eq!((commits, exact), (vec![tip], true));
    }

    #[test]
    fn moving_to_another_line_lists_both_sides_without_id_clashes() {
        let dir = repository();
        let p = dir.path();
        git(p, &["switch", "-q", "-c", "other", "HEAD~1"]);
        write(p, "other.txt", "other\n");
        git(p, &["add", "other.txt"]);
        git(p, &["commit", "-q", "-m", "other"]);
        git(p, &["switch", "-q", "main"]);
        let mut h = Harness::new(dir);
        let other = h.rev("other");
        open(&mut h, other);
        assert!(h.shows_part("on another line"), "{:?}", h.texts);
        assert!(h.shows("a.txt") && h.shows("other.txt"));
        for mode in ["Soft", "Mixed", "Keep", "Hard"] {
            h.click(mode);
        }
    }

    /// The item, in the node menu the graph and the log's rows share: only for the open
    /// worktree's branch, and not where it is.
    #[test]
    fn the_menu_offers_a_reset_of_the_branch_checked_out_here() {
        let dir = repository();
        let catalog = parterre_core::branches::Catalog::load(dir.path()).unwrap();
        let rev = |r| Oid::from_hex(&git(dir.path(), &["rev-parse", r])).unwrap();
        let item = |commit, busy| {
            let reset = |ui: &mut egui::Ui| {
                super::super::branches::reset_item(ui, commit, Some(&catalog), busy)
            };
            menu(reset, None).0
        };
        assert_eq!(item(rev("HEAD~1"), false), ["Reset main to here…"]);
        assert_eq!(item(rev("HEAD~1"), true), ["Reset main to here…"]);
        assert!(item(rev("HEAD"), false).is_empty());
        let repo = parterre_core::git::load_repo(dir.path()).unwrap();
        let base = rev("HEAD~1");
        let node_menu = |ui: &mut egui::Ui| {
            let selection = [base];
            super::super::branches::row_node_menu(
                ui,
                &repo,
                base,
                &selection,
                Some(&catalog),
                false,
                false,
            )
        };
        let (texts, _) = menu(node_menu, None);
        assert!(
            texts.iter().any(|t| t == "Reset main to here…"),
            "the node menu has it: {texts:?}"
        );
        git(dir.path(), &["switch", "-q", "--detach"]);
        let catalog = parterre_core::branches::Catalog::load(dir.path()).unwrap();
        let ctx = egui::Context::default();
        let mut offered = true;
        run_frame(&ctx, egui::RawInput::default(), |ui| {
            offered = super::super::branches::reset_item(ui, rev("HEAD~1"), Some(&catalog), false)
                .is_some()
                || ui.min_rect().height() > 0.0;
        });
        assert!(!offered, "nothing for a detached HEAD");
    }
}
