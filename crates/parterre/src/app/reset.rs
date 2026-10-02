//! The dialog for resetting the open worktree's branch, from a log row's menu. On the left, the
//! files the reset concerns as the picked mode leaves them, in the log window's changed-files
//! table; on the right, what happens to the branch, git's modes, what's lost and the command.

use std::sync::Arc;

use eframe::egui::{self, Color32, Id, RichText, Ui, ViewportId, vec2};
use parterre_core::branches::command_text;
use parterre_core::file_diff::FileDiffSpec;
use parterre_core::reset::{self, FileOutcome, Mode, Preview};
use parterre_core::{Oid, Repo};

use super::file_table::{Badge, Badges, FileTable};
use super::log_window::Colors;
use crate::{dialogs, widgets};

/// The files table's width, the gap with the divider in it, and the side with the modes.
const TABLE: f32 = 520.0;
const GAP: f32 = 40.0;
const SIDE: f32 = 420.0;
/// The modes' names, so their help starts at the same edge.
const MODE_NAME: f32 = 70.0;

/// The files pane is shown, remembered as the Git command section is.
fn files_shown_id() -> Id {
    Id::new("reset-files-shown")
}

#[derive(Debug)]
pub struct ResetDialog {
    pub preview: Arc<Preview>,
    repo: Arc<Repo>,
    pub mode: Mode,
    /// The window it was asked from.
    pub opener: ViewportId,
    fresh: bool,
    /// Asked for again while open: left out for a frame, so its window opens anew in front.
    reopen: bool,
    table: FileTable,
}

/// What the dialog asks for in a frame.
#[derive(Debug, Default)]
pub struct Asked {
    pub answer: Option<dialogs::Answer>,
    /// The log of these commits: the lost ones (exactly them), or the target.
    pub log: Option<(Vec<Oid>, bool)>,
    pub diffs: Vec<FileDiffSpec>,
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

    /// `busy` while another Git operation runs.
    pub fn show(&mut self, ctx: &egui::Context, busy: bool) -> Asked {
        let mut asked = Asked::default();
        if std::mem::take(&mut self.reopen) {
            ctx.request_repaint();
            return asked;
        }
        let preview = self.preview.clone();
        let abbrev = self.repo.abbrev_len.max(7);
        let title = format!(
            "Reset {} to {}",
            preview.branch,
            preview.target.short(abbrev)
        );
        let files_shown = ctx.data_mut(|d| *d.get_persisted_mut_or(files_shown_id(), true));
        let width = if files_shown {
            TABLE + GAP + SIDE
        } else {
            SIDE
        };
        let mut mode = self.mode;
        let mut toggle = false;
        let shown = dialogs::Dialog::new("reset-branch", &title)
            .width(width)
            .opener(self.opener)
            .raise(self.fresh)
            .show(ctx, |ui| {
                // Before the table takes Enter to open the files picked in it.
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal_top(|ui| {
                    let mut opened = false;
                    // The files pane's background, painted once the row's height is known.
                    let background = [(); 2].map(|_| ui.painter().add(egui::Shape::Noop));
                    let mut pane = None;
                    if files_shown {
                        // The right side laid out unseen first, for the files pane's height.
                        let mut sizer = ui.new_child(
                            egui::UiBuilder::new()
                                .id_salt("reset-sizing")
                                .max_rect(ui.available_rect_before_wrap())
                                .layout(egui::Layout::top_down(egui::Align::Min))
                                .sizing_pass()
                                .invisible(),
                        );
                        self.right(
                            &mut sizer,
                            &preview,
                            busy,
                            &mut mode.clone(),
                            &mut Asked::default(),
                            &mut false,
                        );
                        let height = sizer.min_rect().height();
                        let (rect, diffs) = self.files_pane(ui, &preview, mode, height);
                        pane = Some(rect);
                        opened = !diffs.is_empty();
                        asked.diffs.extend(diffs);
                        ui.add_space(GAP);
                    }
                    let right = ui.vertical(|ui| {
                        self.right(ui, &preview, busy, &mut mode, &mut asked, &mut toggle)
                    });
                    if let Some(mut pane) = pane {
                        // To the window's bottom edge, which the taller side decides.
                        pane.max.y = pane
                            .max
                            .y
                            .max(right.response.rect.bottom() + dialogs::MARGIN);
                        let c = super::log_window::colors(ui);
                        let line = egui::Stroke::new(1.0, c.line);
                        ui.painter()
                            .set(background[0], egui::Shape::rect_filled(pane, 0.0, c.pane));
                        ui.painter().set(
                            background[1],
                            egui::Shape::vline(pane.right(), pane.y_range(), line),
                        );
                    }
                    let answer = right.inner;
                    // Enter runs a reset that loses nothing.
                    let runs = preview.refusal(mode).is_none() && !busy;
                    if answer == dialogs::Answer::Open
                        && enter
                        && !opened
                        && !preview.loses(mode)
                        && runs
                    {
                        dialogs::Answer::Primary
                    } else {
                        answer
                    }
                })
                .inner
            });
        self.fresh = false;
        self.mode = mode;
        if toggle {
            ctx.data_mut(|d| d.insert_persisted(files_shown_id(), !files_shown));
        }
        asked.answer = Some(if shown.should_close() {
            dialogs::Answer::Cancel
        } else {
            shown.inner
        });
        asked
    }

    /// The right side: the fields, the Git command, and the buttons' row with *Hide files*.
    fn right(
        &self,
        ui: &mut Ui,
        preview: &Preview,
        busy: bool,
        mode: &mut Mode,
        asked: &mut Asked,
        toggle: &mut bool,
    ) -> dialogs::Answer {
        ui.set_width(SIDE);
        let picked = side(ui, preview, &self.repo, mode, asked);
        dialogs::command_box(ui, &[command_text(&reset::command(picked, preview.target))]);
        let loses = preview.loses(picked);
        let runs = preview.refusal(picked).is_none() && !busy;
        let label = if loses { "Reset anyway" } else { "Reset" };
        let files_shown = ui.data_mut(|d| *d.get_persisted_mut_or(files_shown_id(), true));
        ui.horizontal(|ui| {
            let fold = if files_shown {
                "Hide files"
            } else {
                "Show files"
            };
            // As tall as the row `actions` lays out, to line up with Cancel.
            *toggle |= ui
                .allocate_ui_with_layout(
                    vec2(0.0, 34.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| widgets::text_button(ui, fold),
                )
                .inner
                .clicked();
            dialogs::actions(ui, label, runs, loses, self.fresh && loses)
        })
        .inner
    }

    /// The files, as a pane of the log window: to the window's edges, then a divider. Returns
    /// the pane, for its background, and the diffs a double-click (or Enter) asks for.
    fn files_pane(
        &mut self,
        ui: &mut Ui,
        preview: &Preview,
        mode: Mode,
        height: f32,
    ) -> (egui::Rect, Vec<FileDiffSpec>) {
        let top = ui.cursor().top();
        let (slot, _) = ui.allocate_exact_size(vec2(TABLE, height), egui::Sense::hover());
        let pane = egui::Rect::from_min_max(
            egui::pos2(slot.left() - dialogs::MARGIN, top - dialogs::MARGIN),
            egui::pos2(slot.right() + GAP / 2.0, top + height + dialogs::MARGIN),
        );
        let c = super::log_window::colors(ui);
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(pane)
                .id_salt("reset-files-pane")
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(pane);
        let outcomes = preview.files(mode);
        let files: Vec<_> = outcomes.iter().map(|o| preview.changed_file(o)).collect();
        let badges: Vec<_> = outcomes.iter().map(|o| badges(o, &c, &child)).collect();
        let action = self.table.show_badged(
            &mut child,
            &c,
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

/// The right side's fields: the target commit, what happens to the branch, the modes with
/// their help, git's refusal and the lost commits. Returns the mode picked.
fn side(ui: &mut Ui, preview: &Preview, repo: &Repo, mode: &mut Mode, asked: &mut Asked) -> Mode {
    let abbrev = repo.abbrev_len.max(7);
    dialogs::fields(ui, |ui| {
        if let Some(ix) = repo.lookup(&preview.target)
            && dialogs::commit_line(ui, repo.commit(ix), repo.abbrev_len)
        {
            asked.log = Some((vec![preview.target], false));
        }
        ui.label(preview.movement(abbrev));
        ui.add_space(4.0);
        for m in Mode::ALL {
            ui.horizontal_top(|ui| {
                let radio = ui
                    .allocate_ui_with_layout(
                        vec2(MODE_NAME, 18.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(MODE_NAME);
                            ui.radio(*mode == m, m.name())
                        },
                    )
                    .inner
                    .on_hover_text(m.flag());
                let help = ui.add(
                    egui::Label::new(RichText::new(preview.help(m, abbrev)).small().weak())
                        .wrap()
                        .sense(egui::Sense::click()),
                );
                if radio.clicked() || help.clicked() {
                    *mode = m;
                }
            });
        }
        let red = ui.visuals().error_fg_color;
        if let Some(refusal) = preview.blocked() {
            ui.add_space(4.0);
            ui.label(RichText::new(refusal).color(red));
        } else if let Some(refusal) = preview.refusal(*mode) {
            ui.add_space(4.0);
            ui.label(RichText::new(format!("git reset {} refuses:", mode.flag())).color(red));
            ui.label(RichText::new(refusal).monospace().small().color(red));
        }
        if !preview.commits.is_empty() {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let n = preview.commits.len();
                ui.label(
                    RichText::new(format!(
                        "Lost: {n} commit{}, on no branch, tag or worktree afterwards.",
                        if n == 1 { "" } else { "s" }
                    ))
                    .color(red),
                );
                if ui.link("Show in log").clicked() {
                    asked.log = Some((preview.commits.clone(), true));
                }
            });
        }
    });
    *mode
}

/// A file's Status column: what it is afterwards, as `git status --short` letters (filled when
/// staged, outlined when on disk only, U untracked); a check mark for a file rewritten; a red
/// ! for lost work, or the file git refuses on.
fn badges(o: &FileOutcome, c: &Colors, ui: &Ui) -> Badges {
    let amber = if ui.visuals().dark_mode {
        Color32::from_rgb(0xe0, 0xa8, 0x40)
    } else {
        Color32::from_rgb(0xa8, 0x6a, 0x00)
    };
    let color = |letter: char| match letter {
        'A' | 'U' | '✔' => c.added,
        'D' | '!' => c.removed,
        _ => amber,
    };
    let badge = |letter: char, filled: bool| Badge {
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
    Badges {
        badges,
        words: o.words(),
    }
}

/// The dialog and the branch tool around it, driven frame by frame in a headless context
/// against real, disposable repositories: clicked by the text on screen, as a person would.
#[cfg(test)]
mod tests {
    use eframe::egui;
    use parterre_core::Oid;

    use super::super::branches::Request;
    use super::super::tool_harness::{Harness, collect, git, read, write};

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
            let ctx = egui::Context::default();
            let mut texts = Vec::new();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                super::super::branches::reset_item(ui, commit, Some(&catalog), busy);
            });
            output.textures_delta.clear();
            for clipped in &output.shapes {
                collect(&clipped.shape, &mut texts);
            }
            texts.into_iter().map(|(t, _)| t).collect::<Vec<_>>()
        };
        assert_eq!(item(rev("HEAD~1"), false), ["Reset main to here…"]);
        assert_eq!(item(rev("HEAD~1"), true), ["Reset main to here…"]);
        assert!(item(rev("HEAD"), false).is_empty());
        let repo = parterre_core::git::load_repo(dir.path()).unwrap();
        let ctx = egui::Context::default();
        let mut texts = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let base = rev("HEAD~1");
            super::super::branches::node_menu(ui, &repo, base, Some(&catalog), false, false);
        });
        output.textures_delta.clear();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut texts);
        }
        assert!(
            texts.iter().any(|(t, _)| t == "Reset main to here…"),
            "the node menu has it: {texts:?}"
        );
        git(dir.path(), &["switch", "-q", "--detach"]);
        let catalog = parterre_core::branches::Catalog::load(dir.path()).unwrap();
        let ctx = egui::Context::default();
        let mut offered = true;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            offered = super::super::branches::reset_item(ui, rev("HEAD~1"), Some(&catalog), false)
                .is_some()
                || ui.min_rect().height() > 0.0;
        });
        output.textures_delta.clear();
        assert!(!offered, "nothing for a detached HEAD");
    }
}
