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
