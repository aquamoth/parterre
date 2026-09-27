//! Diff windows: one file diff each, in a window of its own (an immediate viewport, like the log
//! window). Several can be open at once; they outlive the log window they were opened from and
//! close with the repository. Decided in #45 and #46; the model is
//! [`parterre_core::file_diff`].
//!
//! A toolbar switches between side by side and unified, moves between changes, folds unchanged
//! stretches, and picks how changed words are found and what whitespace counts. The last
//! choices are kept in the settings for the next window. On the right an overview strip shows
//! where the changes are; long lines scroll sideways, both panes together.
//!
//! Deliberate deviations from TortoiseGitMerge: unchanged stretches fold by default; line
//! endings count unless whitespace changes are ignored, and a note says when they differ;
//! changed words pair similar lines rather than lines by position; the change marks are an
//! overview strip on the right instead of a locator bar on the left.

use std::sync::{Arc, mpsc};

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{
    self, Color32, CornerRadius, FontId, Key, Modifiers, Rect, RichText, ScrollArea, Sense, Stroke,
    StrokeKind, Ui, UiBuilder, Vec2, pos2, vec2,
};
use parterre_core::blame::BlameSpec;
use parterre_core::changed_files::FileStatus;
use parterre_core::file_diff::{
    Content, DiffLine, DiffOptions, FileDiff, FileDiffSpec, LineKind, LoadedDiff, Note, Rev, Row,
    Shown, Version, Whitespace, WordMode, fold, fold_lines,
};
use parterre_core::glyphs;
use parterre_core::{Oid, Repo};

use super::ParterreApp;
use crate::settings::{DiffForm, DiffWindowSettings};
use crate::text_size;
use crate::widgets;

/// Height of the toolbar.
const TOOLBAR: f32 = 44.0;
/// Height of the pane titles side by side.
const PANE_TITLE: f32 = 24.0;
const FONT_SIZE: f32 = 13.0;
/// Room for the `−`/`+` marker between the line numbers and the text.
const MARKER: f32 = 16.0;
/// Width of the overview strip.
pub(super) const OVERVIEW: f32 = 14.0;
/// Height of the horizontal scrollbar.
pub(super) const SCROLLBAR: f32 = 10.0;
/// Rows kept above a change scrolled to.
const LEAD: f32 = 3.0;

/// The open diff windows.
#[derive(Debug, Default)]
pub struct DiffWindows {
    windows: Vec<DiffWindow>,
    /// How many were opened, to give each its own viewport id.
    opened: u64,
}

impl DiffWindows {
    /// Opens a diff window for `spec`, or brings the one already showing it to the front.
    pub fn open(
        &mut self,
        repo: Arc<Repo>,
        spec: FileDiffSpec,
        settings: &DiffWindowSettings,
        ctx: &egui::Context,
    ) {
        self.open_at(repo, spec, None, settings, ctx);
    }

    /// As [`DiffWindows::open`], scrolled to the change at `line` (from 0) of the new version.
    pub fn open_at(
        &mut self,
        repo: Arc<Repo>,
        spec: FileDiffSpec,
        line: Option<usize>,
        settings: &DiffWindowSettings,
        ctx: &egui::Context,
    ) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.spec == spec) {
            // Files on disk may have changed since: load them again, in the same window.
            if spec.reads_working_tree() {
                *w = DiffWindow::new(w.id, repo, spec, settings, ctx);
            }
            w.goto = line.or(w.goto);
            w.focus = true;
            return;
        }
        self.opened += 1;
        let mut w = DiffWindow::new(self.opened, repo, spec, settings, ctx);
        w.goto = line;
        self.windows.push(w);
    }

    /// Closes every diff window (the repository they belong to is closing).
    pub fn close_all(&mut self) {
        self.windows.clear();
    }

    /// True while git or the diff is still working on any window.
    pub fn is_loading(&self) -> bool {
        self.windows
            .iter()
            .any(|w| matches!(w.load, Load::Loading(_)))
    }
}

/// A loaded diff and its model for the current options.
#[derive(Debug)]
struct Ready {
    loaded: LoadedDiff,
    diff: FileDiff,
    notes: Vec<Note>,
    options: DiffOptions,
}

impl Ready {
    fn new(loaded: LoadedDiff, options: DiffOptions) -> Ready {
        let diff = match &loaded.content {
            Content::Text { old, new, .. } => FileDiff::new(old, new, options),
            _ => FileDiff::default(),
        };
        let notes = diff.notes(&loaded, options.whitespace);
        Ready {
            loaded,
            diff,
            notes,
            options,
        }
    }
}

#[derive(Debug)]
enum Load {
    /// git and the diff run on a worker thread.
    Loading(mpsc::Receiver<Result<Ready, String>>),
    Ready(Box<Ready>),
    Failed(String),
}

/// The version text is chosen in: side by side, the pane; unified, the version of the line
/// the choosing started on (the other version's lines are left out).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
    Old,
    New,
}

impl Column {
    /// The side for [`FileDiff::line`].
    fn side(self) -> Option<bool> {
        match self {
            Column::Old => Some(true),
            Column::New => Some(false),
        }
    }
}

/// "To the end of the line", as a column.
const LINE_END: usize = usize::MAX;

/// Text chosen with the mouse, for copying: in one column, from `anchor` to `head`, each a
/// row of the form and a column (a character of the row's display text). Dragging over the
/// line numbers, or clicking them, chooses whole lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Selection {
    column: Column,
    anchor: (usize, usize),
    head: (usize, usize),
    lines: bool,
}

impl Selection {
    fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// The display columns chosen in `row` of `column` (the end may be [`LINE_END`]).
    fn columns(&self, column: Column, row: usize) -> Option<std::ops::Range<usize>> {
        let (a, b) = self.ordered();
        if column != self.column || row < a.0 || row > b.0 {
            return None;
        }
        if self.lines {
            return Some(0..LINE_END);
        }
        let start = if row == a.0 { a.1 } else { 0 };
        let end = if row == b.0 { b.1 } else { LINE_END };
        Some(start..end)
    }

    fn is_empty(&self) -> bool {
        !self.lines && self.anchor == self.head
    }
}

/// What the pointer did in the rows this frame, for the window to act on afterwards.
#[derive(Default)]
struct RowInput {
    /// A fold was clicked: the lines of the new version it hides.
    fold: Option<std::ops::Range<u32>>,
    /// The primary button went down on text or a line number: row, column, character, and
    /// whether on the line numbers.
    press: Option<(usize, Column, usize, bool)>,
    /// The pointer is over this row: row, and the character under it in each column.
    hover: Option<(usize, [(Column, usize); 2])>,
    /// Rows drawn this frame, first and last.
    first: Option<usize>,
    last: Option<usize>,
    /// Unified: the version a press where the pointer is would choose.
    version: Option<Column>,
}

#[derive(Debug)]
struct DiffWindow {
    id: u64,
    repo: Arc<Repo>,
    spec: FileDiffSpec,
    /// The size the window opened with (the viewport builder must not change while it is open).
    size: Vec2,
    load: Load,
    form: DiffForm,
    options: DiffOptions,
    fold: bool,
    /// Lines of the new version whose folds were opened by a click. Kept by line, so they stay
    /// open when the form, word mode or whitespace setting changes; the fold button folds them
    /// again.
    open: Vec<std::ops::Range<u32>>,
    /// The rows shown in the current form, folds included; rebuilt when `dirty`.
    shown: Vec<Shown>,
    /// Where each change's first row is in `shown`.
    positions: Vec<usize>,
    dirty: bool,
    /// The change in view (index into the form's changes), if the view is at or past one.
    current: Option<usize>,
    /// Scroll to this change in the next frame.
    jump: Option<usize>,
    /// Scroll to this offset in the next frame (a click in the overview).
    scroll_to: Option<f32>,
    /// The offset a jump to `current` left the view at: while the view stays there, that
    /// change stays current even if the view couldn't scroll to it (a short file).
    pinned: Option<f32>,
    /// Sideways scroll of the text, in points, shared by both panes.
    hoff: f32,
    selection: Option<Selection>,
    /// The mouse is choosing text.
    dragging: bool,
    /// Bring the window to the front in the next frame.
    focus: bool,
    closed: bool,
    /// The theme last given to the window's title bar.
    title_theme: Option<egui::SystemTheme>,
    /// A blame window asked for (the toolbar's Blame): the file, and the line to choose.
    blame: Option<(BlameSpec, Option<usize>)>,
    /// Once loaded, scroll to the change at this line (from 0) of the new version.
    goto: Option<usize>,
}

impl DiffWindow {
    fn new(
        id: u64,
        repo: Arc<Repo>,
        spec: FileDiffSpec,
        settings: &DiffWindowSettings,
        ctx: &egui::Context,
    ) -> DiffWindow {
        let options = DiffOptions {
            words: settings.words,
            whitespace: settings.whitespace,
        };
        let (tx, rx) = mpsc::channel();
        let git = parterre_core::git::Git::new(&repo.path);
        let job = spec.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = git
                .load_file_diff(&job)
                .map(|loaded| Ready::new(loaded, options))
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        DiffWindow::new_loading(id, repo, spec, settings, rx)
    }

    /// A window waiting for `rx` to bring its diff.
    fn new_loading(
        id: u64,
        repo: Arc<Repo>,
        spec: FileDiffSpec,
        settings: &DiffWindowSettings,
        rx: mpsc::Receiver<Result<Ready, String>>,
    ) -> DiffWindow {
        let [w, h] = settings.size;
        DiffWindow {
            id,
            repo,
            spec,
            size: vec2(w, h),
            load: Load::Loading(rx),
            form: settings.form,
            options: DiffOptions {
                words: settings.words,
                whitespace: settings.whitespace,
            },
            fold: settings.fold,
            open: Vec::new(),
            shown: Vec::new(),
            positions: Vec::new(),
            dirty: true,
            current: None,
            jump: Some(0),
            scroll_to: None,
            pinned: None,
            hoff: 0.0,
            selection: None,
            dragging: false,
            focus: false,
            closed: false,
            title_theme: None,
            blame: None,
            goto: None,
        }
    }

    fn viewport_id(&self) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(("diff", self.id))
    }

    /// `<file> (<short hash>) – <repo> – Diff`
    fn title(&self) -> String {
        let path = self.spec.path();
        let file = path.rsplit('/').next().unwrap_or(path);
        let rev = self
            .spec
            .new
            .as_ref()
            .or(self.spec.old.as_ref())
            .map(|v| match v.rev {
                Rev::Commit(oid) => oid.short(self.repo.abbrev_len),
                Rev::WorkingTree => "working tree".to_owned(),
            })
            .unwrap_or_default();
        format!("{file} ({rev}) – {} – Diff", self.repo.display_name())
    }

    /// Takes the worker's result when it is there.
    fn poll(&mut self) {
        if let Load::Loading(rx) = &self.load
            && let Ok(result) = rx.try_recv()
        {
            self.load = match result {
                Ok(ready) => Load::Ready(Box::new(ready)),
                Err(e) => Load::Failed(e),
            };
            self.dirty = true;
        }
        // Options changed while the worker ran, or since.
        if let Load::Ready(ready) = &mut self.load
            && ready.options != self.options
        {
            let loaded = ready.loaded.clone();
            **ready = Ready::new(loaded, self.options);
            // The rows change with the options; the selection was made on the old ones.
            self.selection = None;
            self.dirty = true;
            self.jump = Some(self.current.unwrap_or(0));
        }
    }

    fn ready(&self) -> Option<&Ready> {
        match &self.load {
            Load::Ready(r) => Some(r),
            _ => None,
        }
    }

    fn form_ix(&self) -> usize {
        match self.form {
            DiffForm::SideBySide => 0,
            DiffForm::Unified => 1,
        }
    }

    /// The rows of the current form, and where its changes start.
    fn rows_and_changes(diff: &FileDiff, form: DiffForm) -> (&[Row], &[usize]) {
        match form {
            DiffForm::SideBySide => (&diff.side, &diff.side_changes),
            DiffForm::Unified => (&diff.unified, &diff.unified_changes),
        }
    }

    /// Rebuilds the shown rows after a change of form, options or folds.
    fn refresh(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let Some(ready) = self.ready() else {
            self.shown.clear();
            self.positions.clear();
            return;
        };
        let (rows, _) = Self::rows_and_changes(&ready.diff, self.form);
        let shown = if self.fold {
            fold(rows, &ready.diff, &self.open)
        } else {
            (0..rows.len()).map(Shown::Row).collect()
        };
        self.shown = shown;
        self.positions = self.change_positions();
    }

    /// Where each change's first row is among the shown rows.
    fn change_positions(&self) -> Vec<usize> {
        let Some(ready) = self.ready() else {
            return Vec::new();
        };
        let (_, changes) = Self::rows_and_changes(&ready.diff, self.form);
        let mut positions = Vec::with_capacity(changes.len());
        let mut next = changes.iter().peekable();
        for (i, s) in self.shown.iter().enumerate() {
            let Shown::Row(r) = s else { continue };
            while let Some(&&c) = next.peek() {
                if c < *r {
                    next.next();
                } else {
                    break;
                }
            }
            if next.peek() == Some(&r) {
                positions.push(i);
                next.next();
            }
        }
        positions
    }

    /// The change that line `line` (from 0) of the new version is in, or the last one before
    /// it.
    fn change_at(&self, line: usize) -> Option<usize> {
        let ready = self.ready()?;
        let (rows, _) = Self::rows_and_changes(&ready.diff, self.form);
        let at = self.shown.iter().position(
            |s| matches!(s, Shown::Row(r) if rows[*r].new.is_some_and(|n| n as usize >= line)),
        )?;
        self.positions.iter().rposition(|&p| p <= at)
    }

    fn set_form(&mut self, form: DiffForm) {
        if self.form != form {
            self.form = form;
            self.dirty = true;
            self.selection = None;
            self.jump = Some(self.current.unwrap_or(0));
        }
    }

    /// The fold button: folded → whole file → folded, and folded with folds opened by hand →
    /// all folded again.
    fn toggle_fold(&mut self, settings: &mut DiffWindowSettings) {
        if !(self.fold && !self.open.is_empty()) {
            self.fold = !self.fold;
            settings.fold = self.fold;
        }
        self.open.clear();
        self.dirty = true;
        self.jump = Some(self.current.unwrap_or(0));
    }

    fn step(&mut self, forward: bool, changes: usize) {
        if changes == 0 {
            return;
        }
        let target = match (self.current, forward) {
            (None, true) => 0,
            (None, false) => return,
            (Some(c), true) => (c + 1).min(changes - 1),
            (Some(c), false) => c.saturating_sub(1),
        };
        self.jump = Some(target);
    }

    /// Esc closes; Ctrl+D switches the form; Ctrl+Down/Up and F7/Shift+F7 move between changes.
    fn handle_keys(&mut self, ui: &Ui, settings: &mut DiffWindowSettings) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let changes = self.positions.len();
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
            self.select_all();
        }
        let (form, next, prev, close) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::D),
                i.consume_key(Modifiers::COMMAND, Key::ArrowDown)
                    || i.consume_key(Modifiers::NONE, Key::F7),
                i.consume_key(Modifiers::COMMAND, Key::ArrowUp)
                    || i.consume_key(Modifiers::SHIFT, Key::F7),
                i.key_pressed(Key::Escape),
            )
        });
        if form {
            let other = match self.form {
                DiffForm::SideBySide => DiffForm::Unified,
                DiffForm::Unified => DiffForm::SideBySide,
            };
            self.set_form(other);
            settings.form = other;
        }
        if next {
            self.step(true, changes);
        }
        if prev {
            self.step(false, changes);
        }
        if close {
            self.closed = true;
        }
    }

    fn contents(&mut self, ui: &mut Ui, settings: &mut DiffWindowSettings) {
        self.refresh();
        let c = colors(ui);
        self.toolbar(ui, settings, &c);
        self.header(ui, &c);
        let body = ui.available_rect_before_wrap();
        ui.painter().rect_filled(body, 0.0, c.pane);
        match &self.load {
            Load::Loading(_) => message(ui, body, "Loading…", ui.visuals().weak_text_color()),
            Load::Failed(e) => message(
                ui,
                body,
                &format!("Could not read the file: {e}"),
                c.removed,
            ),
            Load::Ready(ready) => match &ready.loaded.content {
                Content::Binary { old_size, new_size } => {
                    let text = format!(
                        "Binary file, not shown.   Old: {}   New: {}",
                        size_text(*old_size),
                        size_text(*new_size)
                    );
                    message(ui, body, &text, ui.visuals().text_color());
                }
                Content::Submodule { old, new } => {
                    let short = |o: &Option<Oid>| {
                        o.map_or("(none)".to_owned(), |o| o.short(self.repo.abbrev_len))
                    };
                    let text = format!("Submodule: {} → {}", short(old), short(new));
                    message(ui, body, &text, ui.visuals().text_color());
                }
                Content::Text { .. } => {
                    let mut child = ui.new_child(UiBuilder::new().max_rect(body));
                    child.set_clip_rect(body.intersect(ui.clip_rect()));
                    self.body(&mut child, body, &c);
                }
            },
        }
        ui.allocate_rect(body, Sense::hover());
    }

    /// Form, changes, folding, word mode and whitespace, left to right.
    fn toolbar(&mut self, ui: &mut Ui, settings: &mut DiffWindowSettings, c: &Colors) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), TOOLBAR), Sense::hover());
        ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
        let mut bar = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(10.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let ui = &mut bar;
        ui.spacing_mut().item_spacing.x = 4.0;

        let forms = [
            (DiffForm::SideBySide, glyphs::DIFF_SIDE_BY_SIDE),
            (DiffForm::Unified, glyphs::DIFF_UNIFIED),
        ];
        let picked = widgets::segmented(ui, self.form, &forms, |form, r| {
            let text = match form {
                DiffForm::SideBySide => "Side by side",
                DiffForm::Unified => "Unified",
            };
            widgets::tip(r, text, "Ctrl+D")
        });
        if let Some(form) = picked {
            self.set_form(form);
            settings.form = form;
        }
        ui.add_space(14.0);

        let changes = self.positions.len();
        let at = self.current;
        let prev = ui
            .add_enabled_ui(at.is_some_and(|c| c > 0), |ui| {
                widgets::tip(
                    widgets::icon_button(ui, glyphs::CHEVRON_UP, false),
                    "Previous change",
                    "Ctrl+Up",
                )
            })
            .inner;
        let next = ui
            .add_enabled_ui(changes > 0 && at.is_none_or(|c| c + 1 < changes), |ui| {
                widgets::tip(
                    widgets::icon_button(ui, glyphs::CHEVRON_DOWN, false),
                    "Next change",
                    "Ctrl+Down",
                )
            })
            .inner;
        if prev.clicked() {
            self.step(false, changes);
        }
        if next.clicked() {
            self.step(true, changes);
        }
        let label = match (changes, at) {
            (0, _) => "No changes".to_owned(),
            (n, Some(c)) => format!("Change {} of {n}", c + 1),
            (n, None) => format!("{n} change{}", if n == 1 { "" } else { "s" }),
        };
        ui.add_space(4.0);
        ui.label(
            RichText::new(label)
                .size(12.5)
                .color(ui.visuals().weak_text_color()),
        );
        ui.add_space(14.0);

        // Three states: folded, folded with some folds opened by hand, and the whole file.
        let opened = self.fold && !self.open.is_empty();
        let (title, body) = match (self.fold, opened) {
            (true, false) => (
                "Unchanged lines folded",
                "Click a fold to open it, or here to show the whole file.",
            ),
            (true, true) => ("Some folds are open", "Click to fold them all again."),
            (false, _) => (
                "Whole file",
                "Click to fold the unchanged stretches between changes.",
            ),
        };
        let r = widgets::tip_explained(fold_button(ui, self.fold, opened), title, "", body);
        if r.clicked() {
            self.toggle_fold(settings);
        }
        ui.add_space(14.0);

        ui.label(
            RichText::new("Words")
                .size(12.5)
                .color(ui.visuals().weak_text_color()),
        );
        let mut words = self.options.words;
        let items = WordMode::ALL.map(|m| (m, m.label()));
        widgets::text_segmented(ui, &mut words, &items);
        if words != self.options.words {
            self.options.words = words;
            settings.words = words;
        }
        ui.add_space(14.0);

        let spaces = [
            (Whitespace::Compare, glyphs::WHITESPACE_COMPARE),
            (Whitespace::IgnoreChanges, glyphs::WHITESPACE_IGNORE_CHANGES),
            (Whitespace::IgnoreAll, glyphs::WHITESPACE_IGNORE_ALL),
        ];
        let picked = widgets::segmented(ui, self.options.whitespace, &spaces, |ws, r| {
            let (title, body) = match ws {
                Whitespace::Compare => (
                    "Compare whitespace",
                    "Every space, tab and line ending counts.",
                ),
                Whitespace::IgnoreChanges => (
                    "Ignore whitespace changes",
                    "More or fewer blanks, and blanks at line ends (CRLF too), don't count. As git diff -b.",
                ),
                Whitespace::IgnoreAll => {
                    ("Ignore all whitespace", "No blank counts. As git diff -w.")
                }
            };
            widgets::tip_explained(r, title, "", body)
        });
        if let Some(ws) = picked {
            self.options.whitespace = ws;
            settings.whitespace = ws;
        }
        let _ = c;

        let why = if self.spec.is_submodule() {
            Some("A submodule has no lines to blame")
        } else if self.spec.binary {
            Some("A binary file has no lines to blame")
        } else {
            None
        };
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add_enabled_ui(why.is_none(), |ui| widgets::text_button(ui, "Blame"));
            let r = match why {
                Some(why) => r.inner.on_disabled_hover_text(why),
                None => widgets::tip_explained(
                    r.inner,
                    "Blame",
                    "",
                    "Which commit last changed each line of the new version (of the old one, \
                     for a deleted file), at the change in view.",
                ),
            };
            if r.clicked() {
                self.blame = self.blame_spec();
            }
        });
    }

    /// The version to blame (the new one, or the old one of a deleted file), and the line of
    /// it at the change in view.
    fn blame_spec(&self) -> Option<(BlameSpec, Option<usize>)> {
        let (version, old) = match (&self.spec.new, &self.spec.old) {
            (Some(v), _) => (v, false),
            (None, Some(v)) => (v, true),
            (None, None) => return None,
        };
        let line = self.ready().and_then(|ready| {
            let (rows, _) = Self::rows_and_changes(&ready.diff, self.form);
            let at = *self.positions.get(self.current?)?;
            let Shown::Row(r) = self.shown.get(at)? else {
                return None;
            };
            // The change's first line of that version, or the one before where it has none.
            rows[*r..]
                .iter()
                .find_map(|row| if old { row.old } else { row.new })
                .map(|l| l as usize)
        });
        let spec = BlameSpec {
            rev: version.rev,
            path: version.path.clone(),
        };
        Some((spec, line))
    }

    /// Status and path, the counts on the right, and the notes below.
    fn header(&self, ui: &mut Ui, c: &Colors) {
        let notes: &[Note] = self.ready().map_or(&[], |r| r.notes.as_slice());
        let height = if notes.is_empty() { 34.0 } else { 54.0 };
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, ui.visuals().panel_fill);
        painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, c.line));
        let weak = ui.visuals().weak_text_color();
        let text = ui.visuals().text_color();
        let (status, color) = match self.spec.status {
            FileStatus::Added => ("Added", c.added),
            FileStatus::Deleted => ("Deleted", c.removed),
            FileStatus::Renamed => ("Renamed", c.renamed),
            FileStatus::Copied => ("Copied", c.renamed),
            other => (other.name(), weak),
        };
        let mut job = LayoutJob::default();
        let small = FontId::proportional(12.5);
        let big = FontId::proportional(14.5);
        job.append(status, 0.0, TextFormat::simple(small.clone(), color));
        let old_path = self.spec.old.as_ref().map(|v| v.path.as_str());
        let new_path = self.spec.new.as_ref().map(|v| v.path.as_str());
        match (old_path, new_path) {
            (Some(a), Some(b)) if a != b => {
                job.append(a, 10.0, TextFormat::simple(big.clone(), text));
                job.append(
                    " → ",
                    0.0,
                    TextFormat::simple(FontId::monospace(14.0), weak),
                );
                job.append(b, 0.0, TextFormat::simple(big.clone(), text));
            }
            _ => job.append(
                self.spec.path(),
                10.0,
                TextFormat::simple(big.clone(), text),
            ),
        }
        let line1 = rect.top() + 17.0;
        let g = painter.layout_job(job);
        painter.galley(pos2(rect.left() + 12.0, line1 - g.size().y / 2.0), g, text);
        if let Some(ready) = self.ready()
            && matches!(ready.loaded.content, Content::Text { .. })
        {
            let mut counts = LayoutJob::default();
            counts.append(
                &format!("+{}", ready.diff.added),
                0.0,
                TextFormat::simple(small.clone(), c.added),
            );
            counts.append(
                &format!("−{}", ready.diff.removed),
                8.0,
                TextFormat::simple(small.clone(), c.removed),
            );
            let g = painter.layout_job(counts);
            painter.galley(
                pos2(rect.right() - 12.0 - g.size().x, line1 - g.size().y / 2.0),
                g,
                text,
            );
        }
        if !notes.is_empty() {
            let line = notes
                .iter()
                .map(Note::to_string)
                .collect::<Vec<_>>()
                .join("   ·   ");
            let g = painter.layout_job(with_arrows(&line, &small, c.note));
            painter.galley(pos2(rect.left() + 12.0, rect.top() + 34.0), g, c.note);
        }
    }

    /// The rows, the overview strip and the horizontal scrollbar.
    fn body(&mut self, ui: &mut Ui, full: Rect, c: &Colors) {
        // The toolbar, drawn just before, may have changed the form or the folding.
        self.refresh();
        if let Some(line) = self.goto.take()
            && let Some(k) = self.change_at(line)
        {
            self.jump = Some(k);
        }
        let positions = self.positions.clone();
        let Load::Ready(ready) = &self.load else {
            return;
        };
        let diff = &ready.diff;
        let font = FontId::monospace(FONT_SIZE);
        let row_h = ui.fonts_mut(|f| f.row_height(&font)).ceil() + 3.0;
        let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
        let digits = (diff.old.len().max(diff.new.len()).max(1) as f32)
            .log10()
            .floor()
            + 1.0;
        let gutter = digits * char_w + 18.0;
        let form = self.form;
        let side = form == DiffForm::SideBySide;

        let mut top = full.top();
        if side {
            let bar = Rect::from_min_max(full.min, pos2(full.right() - OVERVIEW, top + PANE_TITLE));
            self.pane_titles(ui, bar, c);
            top = bar.bottom();
        }

        // Sideways: how much text fits, and how far it can scroll.
        let body_w = full.width() - OVERVIEW;
        let text_w = if side {
            body_w / 2.0 - gutter - MARKER
        } else {
            body_w - 2.0 * gutter - MARKER
        };
        let content_w = diff.widest as f32 * char_w + 24.0;
        let hmax = (content_w - text_w).max(0.0);
        let bottom = full.bottom() - if hmax > 0.0 { SCROLLBAR } else { 0.0 };
        let area = Rect::from_min_max(
            pos2(full.left(), top),
            pos2(full.right() - OVERVIEW, bottom),
        );

        let (rows, _) = Self::rows_and_changes(diff, form);
        let mut scroll = ScrollArea::vertical()
            .auto_shrink(false)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .id_salt(("diff", self.id, self.form_ix(), self.fold));
        if let Some(offset) = self.scroll_to.take() {
            scroll = scroll.vertical_scroll_offset(offset.max(0.0));
            self.pinned = None;
        }
        let jumped = self.jump.take().filter(|&k| k < positions.len());
        if let Some(&at) = jumped.and_then(|k| positions.get(k)) {
            scroll = scroll.vertical_scroll_offset((at as f32 - LEAD).max(0.0) * row_h);
            // Bring the change's first changed word into view sideways.
            self.hoff = match first_word_column(diff, rows, &self.shown, at) {
                Some(col) if col as f32 * char_w > text_w - 40.0 => {
                    (col as f32 * char_w - 80.0).clamp(0.0, hmax)
                }
                _ => 0.0,
            };
        }
        if ui.rect_contains_pointer(area) {
            let dx = ui.input(|i| i.smooth_scroll_delta.x);
            self.hoff -= dx;
        }
        self.hoff = self.hoff.clamp(0.0, hmax);

        let geometry = Geometry {
            row_h,
            gutter,
            hoff: self.hoff,
            font: font.clone(),
        };
        let shown = &self.shown;
        let selection = self.selection;
        let dragging = self.dragging;
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let (pressed, shift, ctrl) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.modifiers.shift,
                i.modifiers.command,
            )
        });
        let mut input = RowInput::default();
        let mut child = ui.new_child(UiBuilder::new().max_rect(area));
        child.set_clip_rect(area.intersect(ui.clip_rect()));
        child.spacing_mut().item_spacing = Vec2::ZERO;
        let out = scroll.show_rows(&mut child, row_h, shown.len(), |ui, range| {
            for i in range {
                let (rect, response) = ui.allocate_exact_size(
                    vec2(ui.available_width(), row_h),
                    Sense::click_and_drag(),
                );
                let r = match &shown[i] {
                    Shown::Fold(hidden) => {
                        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                        fold_row(ui, rect, hidden.len(), response.hovered(), c);
                        if response.clicked() && !dragging {
                            input.fold = fold_lines(rows, hidden);
                        }
                        continue;
                    }
                    Shown::Row(r) => *r,
                };
                input.first.get_or_insert(r);
                input.last = Some(r);
                let row = rows[r];
                // Each column: where it is, its numbers, its line, and where its text went.
                let columns: Vec<(Column, Rect, f32, Option<TextAt>)> = if side {
                    let mid = rect.center().x;
                    let halves = [
                        (Column::Old, rect.left()..=mid, Numbers::Own),
                        (Column::New, mid..=rect.right(), Numbers::Own),
                    ];
                    let drawn = halves
                        .into_iter()
                        .map(|(column, xs, numbers)| {
                            let half = Rect::from_x_y_ranges(xs, rect.y_range());
                            let line = diff.line(row, column.side());
                            let sel =
                                selection.and_then(|s| Some((s.columns(column, r)?, s.lines)));
                            let at = paint_line(ui, half, line, numbers, &geometry, sel, c);
                            (column, half, gutter, at)
                        })
                        .collect();
                    ui.painter()
                        .vline(mid, rect.y_range(), Stroke::new(1.0, c.line));
                    drawn
                } else {
                    let line = diff.line(row, None);
                    // Only lines of the chosen version are chosen.
                    let sel = selection
                        .filter(|s| diff.line(row, s.column.side()).is_some())
                        .and_then(|s| Some((s.columns(s.column, r)?, s.lines)));
                    let numbers = Numbers::Both(row.old, row.new);
                    let at = paint_line(ui, rect, line, numbers, &geometry, sel, c);
                    // The version is settled below, when a press starts choosing.
                    vec![(Column::New, rect, 2.0 * gutter, at)]
                };
                let Some(p) = pointer.filter(|p| rect.y_range().contains(p.y)) else {
                    continue;
                };
                let char_at = |at: &Option<TextAt>| {
                    at.as_ref().map_or(0, |t| {
                        if p.x <= t.origin.x {
                            0
                        } else {
                            t.galley.cursor_from_pos(p - t.origin).index.0
                        }
                    })
                };
                let under: Vec<(Column, usize)> = if side {
                    columns
                        .iter()
                        .map(|(col, _, _, at)| (*col, char_at(at)))
                        .collect()
                } else {
                    let col = char_at(&columns[0].3);
                    vec![(Column::Old, col), (Column::New, col)]
                };
                input.hover = Some((r, [under[0], *under.last().unwrap_or(&under[0])]));
                if let Some((column, half, numbers_w, at)) = columns
                    .iter()
                    .find(|(_, half, _, _)| half.x_range().contains(p.x))
                {
                    let on_numbers = p.x < half.left() + numbers_w;
                    if !on_numbers {
                        let _ = response.clone().on_hover_cursor(egui::CursorIcon::Text);
                    }
                    // The version a press here chooses: side by side, the pane; unified,
                    // the selection's with Shift, else the line's (Ctrl takes the old
                    // version of an unchanged line), or on the numbers, their version.
                    let column = if side {
                        *column
                    } else if let Some(s) = selection.filter(|_| shift) {
                        s.column
                    } else if on_numbers {
                        // The old numbers come first, then the new.
                        if p.x < half.left() + gutter {
                            Column::Old
                        } else {
                            Column::New
                        }
                    } else {
                        match diff.line(row, None).map(|l| l.kind) {
                            Some(LineKind::Removed) => Column::Old,
                            Some(LineKind::Same) if ctrl => Column::Old,
                            _ => Column::New,
                        }
                    };
                    if !side {
                        input.version = Some(column);
                    }
                    if pressed && response.hovered() {
                        input.press = Some((r, column, char_at(at), on_numbers));
                    }
                }
            }
        });
        // The change in view: the one jumped to while the view stays put, else the last one
        // at or above the reading line (as far down as a jump puts a change).
        let offset = out.state.offset.y;
        if let Some(k) = jumped {
            self.current = Some(k);
            self.pinned = Some(offset);
        } else if self.pinned.is_none_or(|p| (p - offset).abs() > 0.5) {
            self.pinned = None;
            let first = (offset / row_h).round() as usize;
            self.current = positions
                .iter()
                .rposition(|&p| p <= first + LEAD as usize)
                .or((!positions.is_empty() && first == 0).then_some(0));
        }

        let strip = Rect::from_min_max(pos2(full.right() - OVERVIEW, top), full.max);
        let view = (
            out.state.offset.y,
            out.inner_rect.height(),
            out.content_size.y,
        );
        overview(ui, strip, diff, rows, &self.shown, side, view, row_h, c);
        self.select(ui, &input, area, out.state.offset.y, row_h);
        // Unified: a sign by the pointer says which version choosing takes.
        let badge = if self.dragging {
            self.selection
                .map(|s| s.column)
                .filter(|_| form == DiffForm::Unified)
        } else {
            input.version
        };
        if let (Some(version), Some(p)) = (badge, ui.input(|i| i.pointer.hover_pos())) {
            version_badge(ui, p, version, c);
        }
        if let Some(lines) = input.fold {
            self.open.push(lines);
            self.dirty = true;
        }
        let id = egui::Id::new(("diff-overview", self.id));
        if let Some(offset) = overview_scroll(ui, id, strip, self.shown.len(), row_h, view) {
            self.scroll_to = Some(offset);
        }
        if hmax > 0.0 {
            let track = Rect::from_min_max(
                pos2(full.left(), bottom),
                pos2(full.right() - OVERVIEW, full.bottom()),
            );
            let id = egui::Id::new(("diff-hbar", self.id));
            hscrollbar(ui, id, track, &mut self.hoff, text_w / content_w, hmax, c);
        }
    }

    fn pane_titles(&self, ui: &Ui, bar: Rect, c: &Colors) {
        let painter = ui.painter();
        painter.rect_filled(bar, 0.0, ui.visuals().panel_fill);
        painter.hline(bar.x_range(), bar.top() + 0.5, Stroke::new(1.0, c.line));
        painter.hline(bar.x_range(), bar.bottom() - 0.5, Stroke::new(1.0, c.line));
        let weak = ui.visuals().weak_text_color();
        let mid = bar.center().x;
        for (version, x) in [(&self.spec.old, bar.left()), (&self.spec.new, mid)] {
            let text = match version {
                Some(v) => self.version_title(v),
                None => "(no file)".to_owned(),
            };
            let g = painter.layout_no_wrap(text, FontId::proportional(12.0), weak);
            let clip = Rect::from_x_y_ranges(x..=x + bar.width() / 2.0 - 8.0, bar.y_range());
            painter.with_clip_rect(clip).galley(
                pos2(x + 10.0, bar.center().y - g.size().y / 2.0),
                g,
                weak,
            );
        }
    }

    /// `<short hash>  <subject>`, or "Working tree".
    fn version_title(&self, v: &Version) -> String {
        let Rev::Commit(oid) = v.rev else {
            return "Working tree".to_owned();
        };
        let short = oid.short(self.repo.abbrev_len);
        let subject = self
            .repo
            .lookup(&oid)
            .map(|c| self.repo.commit(c).subject.clone())
            .unwrap_or_default();
        format!("{short}   {subject}")
    }

    /// Chooses text with the mouse: a press starts (Shift extends), a drag moves the end
    /// (scrolling past the edges), a double-click takes a word, and a click without a drag
    /// clears. Presses on the line numbers choose whole lines.
    fn select(&mut self, ui: &Ui, input: &RowInput, area: Rect, offset: f32, row_h: f32) {
        let (down, shift, double) = ui.input(|i| {
            (
                i.pointer.primary_down(),
                i.modifiers.shift,
                i.pointer
                    .button_double_clicked(egui::PointerButton::Primary),
            )
        });
        if let Some((row, column, col, on_numbers)) = input.press {
            let here = (row, if on_numbers { LINE_END } else { col });
            self.selection = match self.selection {
                Some(s) if shift && s.column == column => Some(Selection { head: here, ..s }),
                _ => Some(Selection {
                    column,
                    anchor: here,
                    head: here,
                    lines: on_numbers,
                }),
            };
            self.dragging = true;
        } else if double
            && let Some((row, under)) = input.hover
            && let Some(s) = self.selection
            && !s.lines
            && let Some(&(_, col)) = under.iter().find(|(c, _)| *c == s.column)
            && let Some(words) = self.word_at(s.column, row, col)
        {
            self.selection = Some(Selection {
                anchor: (row, words.start),
                head: (row, words.end),
                ..s
            });
            self.dragging = false;
        }
        if !self.dragging {
            return;
        }
        if !down {
            self.dragging = false;
            if self.selection.is_some_and(|s| s.is_empty()) {
                self.selection = None;
            }
            return;
        }
        let Some(mut s) = self.selection else { return };
        let pointer = ui.input(|i| i.pointer.interact_pos());
        if let Some((row, under)) = input.hover
            && let Some(&(_, col)) = under.iter().find(|(c, _)| *c == s.column)
        {
            s.head = (row, if s.lines { LINE_END } else { col });
        } else if let Some(p) = pointer {
            // Past the top or bottom: take the first or last row drawn, and scroll on.
            if p.y < area.top() {
                if let Some(first) = input.first {
                    s.head = (first, 0);
                }
                self.scroll_to = Some(offset - row_h);
            } else if p.y > area.bottom() {
                if let Some(last) = input.last {
                    s.head = (last, LINE_END);
                }
                self.scroll_to = Some(offset + row_h);
            }
            ui.ctx().request_repaint();
        }
        self.selection = Some(s);
    }

    /// The word (or run of blanks, or other character) at display column `col` of a row.
    fn word_at(&self, column: Column, row: usize, col: usize) -> Option<std::ops::Range<usize>> {
        let ready = self.ready()?;
        let (rows, _) = Self::rows_and_changes(&ready.diff, self.form);
        let text: Vec<char> = ready
            .diff
            .line(*rows.get(row)?, column.side())?
            .text
            .chars()
            .collect();
        let col = col.min(text.len().checked_sub(1)?);
        let class = |c: char| {
            if c.is_alphanumeric() || c == '_' {
                0
            } else if c.is_whitespace() {
                1
            } else {
                2
            }
        };
        let k = class(text[col]);
        if k == 2 {
            return Some(col..col + 1);
        }
        let start = (0..col)
            .rev()
            .take_while(|&i| class(text[i]) == k)
            .last()
            .unwrap_or(col);
        let end = (col..text.len())
            .take_while(|&i| class(text[i]) == k)
            .last()
            .unwrap_or(col)
            + 1;
        Some(start..end)
    }

    /// The chosen text as it is in the file (tabs kept), a line per row; whole lines end
    /// with a newline. Fillers have no text and add no line.
    fn selected_text(&self) -> Option<String> {
        let s = self.selection.filter(|s| !s.is_empty())?;
        let ready = self.ready()?;
        let (rows, _) = Self::rows_and_changes(&ready.diff, self.form);
        let (a, b) = s.ordered();
        let mut lines = Vec::new();
        let chosen = rows.iter().enumerate().take(b.0 + 1).skip(a.0);
        for (r, &row) in chosen {
            let Some(line) = ready.diff.line(row, s.column.side()) else {
                continue;
            };
            let cols = s.columns(s.column, r)?;
            let start = line.raw_offset(cols.start);
            let end = if cols.end == LINE_END {
                line.raw.len()
            } else {
                line.raw_offset(cols.end)
            };
            lines.push(&line.raw[start..end.max(start)]);
        }
        let mut text = lines.join("\n");
        if s.lines || b.1 == LINE_END {
            text.push('\n');
        }
        Some(text)
    }

    /// Chooses every line of the column in use (the new side, or the unified column).
    fn select_all(&mut self) {
        let Some(ready) = self.ready() else { return };
        let (rows, _) = Self::rows_and_changes(&ready.diff, self.form);
        let Some(last) = rows.len().checked_sub(1) else {
            return;
        };
        let column = self.selection.map_or(Column::New, |s| s.column);
        self.selection = Some(Selection {
            column,
            anchor: (0, 0),
            head: (last, LINE_END),
            lines: true,
        });
    }

    /// Shows the window; returns nothing, but sets `closed` when it was closed. Ctrl+wheel and
    /// Ctrl+plus, minus and 0 change `text_size`.
    fn show(
        &mut self,
        ctx: &egui::Context,
        settings: &mut DiffWindowSettings,
        text_size: &mut f32,
        window_theme: Option<egui::SystemTheme>,
        icon: &Arc<egui::IconData>,
    ) {
        let builder = egui::ViewportBuilder::default()
            .with_title(self.title())
            .with_app_id(crate::settings::APP_ID)
            .with_icon(icon.clone())
            .with_inner_size(self.size)
            .with_min_inner_size([520.0, 320.0]);
        let id = self.viewport_id();
        if self.title_theme.is_none() && !ctx.embed_viewports() {
            ctx.request_repaint();
        }
        if std::mem::take(&mut self.focus) && !ctx.embed_viewports() {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        }
        ctx.show_viewport_immediate(id, builder, |ui, class| {
            self.poll();
            if class != egui::ViewportClass::EmbeddedWindow {
                if self.title_theme != window_theme {
                    self.title_theme = window_theme;
                    if let Some(theme) = window_theme {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::SetTheme(theme));
                    }
                }
                let (close, size) = ui.input(|i| {
                    (
                        i.viewport().close_requested(),
                        i.viewport().inner_rect.map(|r| r.size()),
                    )
                });
                if let Some(size) = size
                    && size.x > 0.0
                    && size.y > 0.0
                {
                    settings.size = [size.x, size.y];
                }
                // Keys go to the main window too when the diff is embedded in it.
                self.handle_keys(ui, settings);
                text_size::read_input(ui, text_size, true);
                if close {
                    self.closed = true;
                }
            }
            if ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)))
                && let Some(text) = self.selected_text()
            {
                ui.ctx().copy_text(text);
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::central_panel(&ui.ctx().global_style()).inner_margin(0))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    self.contents(ui, settings);
                });
        });
    }
}

/// Sizes shared by the rows of a frame.
struct Geometry {
    row_h: f32,
    gutter: f32,
    hoff: f32,
    font: FontId,
}

/// The line numbers a row shows: its own line's, or both sides' (unified).
enum Numbers {
    Own,
    Both(Option<u32>, Option<u32>),
}

/// Where a line's text was drawn, for finding the character under the pointer.
struct TextAt {
    galley: Arc<egui::Galley>,
    origin: egui::Pos2,
}

/// Paints one line (or a filler, for `None`) into `rect`: line numbers, marker, text, and
/// the chosen characters `sel` (display columns, the end may be [`LINE_END`]; and whether
/// whole lines were chosen, on the numbers). Returns where the text went.
fn paint_line(
    ui: &Ui,
    rect: Rect,
    line: Option<&DiffLine>,
    numbers: Numbers,
    g: &Geometry,
    sel: Option<(std::ops::Range<usize>, bool)>,
    c: &Colors,
) -> Option<TextAt> {
    let (sel, whole) = match sel {
        Some((range, lines)) => (Some(range), lines),
        None => (None, false),
    };
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let Some(line) = line else {
        painter.rect_filled(rect, 0.0, c.filler);
        return None;
    };
    let (bg, word, marker, marker_color) = match line.kind {
        LineKind::Same => (Color32::TRANSPARENT, Color32::TRANSPARENT, "", c.weak),
        LineKind::Removed => (c.removed_line, c.removed_word, "−", c.removed),
        LineKind::Added => (c.added_line, c.added_word, "+", c.added),
    };
    painter.rect_filled(rect, 0.0, bg);
    let y = rect.top() + 1.5;
    let number = |x: f32, no: Option<u32>| {
        let Some(ix) = no else { return };
        let cell = Rect::from_min_size(pos2(x, rect.top()), vec2(g.gutter, g.row_h));
        if whole {
            painter.rect_filled(cell, 0.0, c.selected_bg);
        }
        let color = if whole { c.selected_fg } else { c.weak };
        let text = (ix + 1).to_string();
        let galley = painter.layout_no_wrap(text, g.font.clone(), color);
        painter.galley(pos2(x + g.gutter - 8.0 - galley.size().x, y), galley, color);
    };
    let mut x = rect.left();
    match numbers {
        Numbers::Own => {
            number(x, Some(line.no - 1));
            x += g.gutter;
        }
        Numbers::Both(old, new) => {
            number(x, old);
            x += g.gutter;
            number(x, new);
            x += g.gutter;
        }
    }
    if !marker.is_empty() {
        let m = painter.layout_no_wrap(marker.into(), g.font.clone(), marker_color);
        painter.galley(pos2(x + 2.0, y), m, marker_color);
    }
    x += MARKER;
    let mut job = LayoutJob::default();
    let format = |background: Color32| TextFormat {
        font_id: g.font.clone(),
        color: c.text,
        background,
        ..Default::default()
    };
    let mut at = 0;
    for span in &line.spans {
        if span.start > at {
            job.append(
                &line.text[at..span.start],
                0.0,
                format(Color32::TRANSPARENT),
            );
        }
        job.append(&line.text[span.clone()], 0.0, format(word));
        at = span.end;
    }
    if at < line.text.len() {
        job.append(&line.text[at..], 0.0, format(Color32::TRANSPARENT));
    }
    let galley = painter.layout_job(job);
    let clip = Rect::from_min_max(pos2(x, rect.top()), rect.max).intersect(ui.clip_rect());
    let text = painter.with_clip_rect(clip);
    let origin = pos2(x - g.hoff, y);
    if let Some(sel) = sel {
        let at =
            |col: usize| origin.x + galley.pos_from_cursor(egui::text::CCursor::new(col)).min.x;
        let x0 = at(sel.start);
        let x1 = if sel.end == LINE_END {
            // Past the end, a little, to show the line break is included.
            origin.x + galley.size().x + g.row_h / 2.0
        } else {
            at(sel.end)
        };
        text.rect_filled(
            Rect::from_x_y_ranges(x0..=x1, rect.y_range()),
            0.0,
            c.selection,
        );
    }
    text.galley(origin, galley.clone(), c.text);
    Some(TextAt { galley, origin })
}

/// The fold button, whose icon says the state: arrows closing, tinted, when unchanged
/// stretches are folded; the same untinted when some folds were opened by hand; arrows
/// opening when the whole file shows.
fn fold_button(ui: &mut Ui, on: bool, opened: bool) -> egui::Response {
    let (glyph, tinted) = match (on, opened) {
        (true, false) => (glyphs::FOLD, true),
        (true, true) => (glyphs::FOLD, false),
        (false, _) => (glyphs::UNFOLD, false),
    };
    widgets::icon_button(ui, glyph, tinted)
}

/// A small green `+` (new version) or red `−` (old version) at the lower right of the
/// pointer, a quiet reminder of which version choosing text takes in the unified form. egui
/// can't change the cursor's image, so the sign is drawn beside it, above everything else.
fn version_badge(ui: &Ui, pointer: egui::Pos2, version: Column, c: &Colors) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("diff-version-badge"),
    ));
    let center = pointer + vec2(10.0, 12.0);
    let (color, plus) = match version {
        Column::Old => (c.removed, false),
        Column::New => (c.added, true),
    };
    let stroke = Stroke::new(1.5, color);
    painter.hline(center.x - 3.0..=center.x + 3.0, center.y, stroke);
    if plus {
        painter.vline(center.x, center.y - 3.0..=center.y + 3.0, stroke);
    }
}

/// A fold: `n unchanged lines`, across the row.
fn fold_row(ui: &Ui, rect: Rect, lines: usize, hovered: bool, c: &Colors) {
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, if hovered { c.fold_hover } else { c.fold });
    painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, c.line));
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, c.line),
    );
    let text = if hovered {
        format!("Show {lines} unchanged lines")
    } else {
        format!("{lines} unchanged lines")
    };
    let color = if hovered {
        ui.visuals().text_color()
    } else {
        c.weak
    };
    let g = painter.layout_no_wrap(text, FontId::proportional(12.0), color);
    painter.galley(rect.center() - g.size() / 2.0, g, color);
}

/// The overview strip: every change's place in the whole diff, and the part in view.
#[allow(clippy::too_many_arguments)]
fn overview(
    ui: &Ui,
    strip: Rect,
    diff: &FileDiff,
    rows: &[Row],
    shown: &[Shown],
    side: bool,
    view: (f32, f32, f32),
    row_h: f32,
    c: &Colors,
) {
    overview_background(ui, strip, c);
    let painter = ui.painter();
    let scale = overview_scale(strip, shown.len(), row_h);
    let mark = |i: usize, kind: LineKind| {
        let y = strip.top() + i as f32 * scale;
        let color = if kind == LineKind::Removed {
            c.removed
        } else {
            c.added
        };
        let (x0, x1) = match (side, kind) {
            (true, LineKind::Removed) => (strip.left() + 3.0, strip.center().x),
            (true, _) => (strip.center().x, strip.right() - 2.0),
            _ => (strip.left() + 3.0, strip.right() - 2.0),
        };
        painter.rect_filled(
            Rect::from_min_max(pos2(x0, y), pos2(x1, y + scale.max(2.0))),
            0.0,
            color,
        );
    };
    for (i, s) in shown.iter().enumerate() {
        let Shown::Row(r) = s else { continue };
        let row = rows[*r];
        let kind = |lines: &[DiffLine], ix: Option<u32>| ix.map(|ix| lines[ix as usize].kind);
        if kind(&diff.old, row.old) == Some(LineKind::Removed) {
            mark(i, LineKind::Removed);
        }
        if kind(&diff.new, row.new) == Some(LineKind::Added) {
            mark(i, LineKind::Added);
        }
    }
    overview_view(ui, strip, view, c);
}

/// The height of a row in an overview strip of `rows` rows: a text shorter than the window is
/// drawn at its own scale, level with its rows.
pub(super) fn overview_scale(strip: Rect, rows: usize, row_h: f32) -> f32 {
    (strip.height() / rows.max(1) as f32).min(row_h)
}

/// An overview strip's background and its line on the left, under the marks.
pub(super) fn overview_background(ui: &Ui, strip: Rect, c: &Colors) {
    let painter = ui.painter();
    painter.rect_filled(strip, 0.0, ui.visuals().panel_fill);
    painter.vline(
        strip.left() + 0.5,
        strip.y_range(),
        Stroke::new(1.0, c.line),
    );
}

/// The part in view, over an overview strip's marks. `view` is the scroll offset, the height in
/// view and the height of the whole text.
pub(super) fn overview_view(
    ui: &Ui,
    strip: Rect,
    (offset, height, content): (f32, f32, f32),
    c: &Colors,
) {
    if content > height {
        let y0 = strip.top() + offset / content * strip.height();
        let y1 = strip.top() + (offset + height) / content * strip.height();
        ui.painter().rect_stroke(
            Rect::from_min_max(
                pos2(strip.left() + 1.5, y0),
                pos2(strip.right() - 0.5, y1.min(strip.bottom())),
            ),
            1.0,
            Stroke::new(1.0, c.weak),
            StrokeKind::Inside,
        );
    }
}

/// An overview strip scrolls too: a click or drag on it gives the scroll offset that puts that
/// place in the middle. `view` as for [`overview_view`].
pub(super) fn overview_scroll(
    ui: &Ui,
    id: egui::Id,
    strip: Rect,
    rows: usize,
    row_h: f32,
    (_, height, content): (f32, f32, f32),
) -> Option<f32> {
    let response = ui.interact(strip, id, Sense::click_and_drag());
    let p = response.interact_pointer_pos()?;
    if !(response.clicked() || response.dragged()) || content <= height {
        return None;
    }
    ui.ctx().request_repaint();
    let row = (p.y - strip.top()) / overview_scale(strip, rows, row_h);
    Some(row * row_h - height / 2.0)
}

/// A horizontal scrollbar in `track` for an offset `hoff` of at most `hmax`, `visible` being
/// the share of the width in view: drag the thumb, or click to put it there.
pub(super) fn hscrollbar(
    ui: &mut Ui,
    id: egui::Id,
    track: Rect,
    hoff: &mut f32,
    visible: f32,
    hmax: f32,
    c: &Colors,
) {
    ui.painter()
        .rect_filled(track, 0.0, ui.visuals().panel_fill);
    let thumb_w = (track.width() * visible).clamp(30.0, track.width());
    let room = (track.width() - thumb_w).max(1.0);
    let x = track.left() + room * (*hoff / hmax);
    let thumb = Rect::from_min_size(pos2(x, track.top() + 2.0), vec2(thumb_w, SCROLLBAR - 4.0));
    let response = ui.interact(track, id, Sense::click_and_drag());
    if response.dragged() {
        *hoff = (*hoff + response.drag_delta().x * hmax / room).clamp(0.0, hmax);
    } else if response.clicked()
        && let Some(p) = response.interact_pointer_pos()
    {
        *hoff = ((p.x - track.left() - thumb_w / 2.0) / room * hmax).clamp(0.0, hmax);
    }
    let fill = if response.hovered() || response.dragged() {
        c.thumb_hover
    } else {
        c.thumb
    };
    ui.painter().rect_filled(thumb, CornerRadius::same(3), fill);
}

/// The column of the first changed word in the shown row `at`, if any.
fn first_word_column(diff: &FileDiff, rows: &[Row], shown: &[Shown], at: usize) -> Option<usize> {
    let Some(Shown::Row(r)) = shown.get(at) else {
        return None;
    };
    let row = rows[*r];
    [
        row.old.map(|i| &diff.old[i as usize]),
        row.new.map(|i| &diff.new[i as usize]),
    ]
    .into_iter()
    .flatten()
    .filter_map(|l| l.spans.first().map(|s| l.text[..s.start].chars().count()))
    .min()
}

/// `text` in `font`, but its arrows in the monospace font: the proportional one has none.
fn with_arrows(text: &str, font: &FontId, color: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (i, part) in text.split('→').enumerate() {
        if i > 0 {
            job.append(
                "→",
                0.0,
                TextFormat::simple(FontId::monospace(font.size), color),
            );
        }
        job.append(part, 0.0, TextFormat::simple(font.clone(), color));
    }
    job
}

pub(super) fn message(ui: &Ui, body: Rect, text: &str, color: Color32) {
    let g = ui
        .painter()
        .layout_job(with_arrows(text, &FontId::proportional(14.0), color));
    ui.painter().galley(body.min + vec2(20.0, 20.0), g, color);
}

fn size_text(size: Option<u64>) -> String {
    let Some(n) = size else {
        return "(none)".to_owned();
    };
    if n < 1024 {
        format!("{n} bytes")
    } else if n < 1024 * 1024 {
        format!("{:.1} KiB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", n as f64 / 1024.0 / 1024.0)
    }
}

pub(super) struct Colors {
    pub(super) pane: Color32,
    pub(super) line: Color32,
    pub(super) weak: Color32,
    pub(super) text: Color32,
    pub(super) filler: Color32,
    pub(super) fold: Color32,
    pub(super) fold_hover: Color32,
    pub(super) removed_line: Color32,
    pub(super) removed_word: Color32,
    pub(super) added_line: Color32,
    pub(super) added_word: Color32,
    pub(super) removed: Color32,
    pub(super) added: Color32,
    pub(super) renamed: Color32,
    pub(super) note: Color32,
    pub(super) selected_bg: Color32,
    pub(super) selected_fg: Color32,
    /// Behind chosen text.
    pub(super) selection: Color32,
    pub(super) thumb: Color32,
    pub(super) thumb_hover: Color32,
}

/// The log window's colours for panes and statuses, and tints for the lines.
pub(super) fn colors(ui: &Ui) -> Colors {
    let t = widgets::tones(ui);
    let weak = ui.visuals().weak_text_color();
    let text = ui.visuals().text_color();
    if ui.visuals().dark_mode {
        Colors {
            pane: Color32::from_gray(22),
            line: Color32::from_white_alpha(23),
            weak,
            text,
            filler: Color32::from_gray(30),
            fold: Color32::from_gray(28),
            fold_hover: Color32::from_gray(36),
            removed_line: Color32::from_rgb(0x3d, 0x1c, 0x20),
            removed_word: Color32::from_rgb(0x80, 0x2c, 0x35),
            added_line: Color32::from_rgb(0x17, 0x33, 0x21),
            added_word: Color32::from_rgb(0x26, 0x62, 0x37),
            removed: Color32::from_rgb(0xff, 0x8a, 0x80),
            added: Color32::from_rgb(0x7b, 0xd8, 0x8f),
            renamed: Color32::from_rgb(0xd1, 0xa5, 0xff),
            note: Color32::from_rgb(0xe8, 0xc0, 0x6a),
            selected_bg: t.on_bg,
            selected_fg: Color32::from_rgb(0xcf, 0xe5, 0xff),
            selection: Color32::from_rgba_unmultiplied(0x35, 0x84, 0xe4, 110),
            thumb: Color32::from_white_alpha(50),
            thumb_hover: Color32::from_white_alpha(90),
        }
    } else {
        Colors {
            pane: Color32::WHITE,
            line: Color32::from_black_alpha(26),
            weak,
            text,
            filler: Color32::from_gray(243),
            fold: Color32::from_rgb(0xf3, 0xf6, 0xfa),
            fold_hover: Color32::from_rgb(0xe6, 0xee, 0xf8),
            removed_line: Color32::from_rgb(0xff, 0xeb, 0xe9),
            removed_word: Color32::from_rgb(0xff, 0xc0, 0xc0),
            added_line: Color32::from_rgb(0xe6, 0xff, 0xec),
            added_word: Color32::from_rgb(0xab, 0xf2, 0xbc),
            removed: Color32::from_rgb(0xc6, 0x28, 0x28),
            added: Color32::from_rgb(0x2e, 0x7d, 0x32),
            renamed: Color32::from_rgb(0x7b, 0x3f, 0xc4),
            note: Color32::from_rgb(0x9a, 0x5b, 0x00),
            selected_bg: t.on_bg,
            selected_fg: Color32::from_rgb(0x0b, 0x3d, 0x7a),
            selection: Color32::from_rgba_unmultiplied(0x35, 0x84, 0xe4, 80),
            thumb: Color32::from_black_alpha(45),
            thumb_hover: Color32::from_black_alpha(90),
        }
    }
}

impl ParterreApp {
    /// Every open diff window.
    pub(super) fn diff_windows(&mut self, ctx: &egui::Context) {
        let settings = &mut self.settings;
        let mut blames = Vec::new();
        for window in &mut self.diffs.windows {
            window.show(
                ctx,
                &mut settings.diff_window,
                &mut settings.text_size,
                self.window_theme,
                &self.window_icon,
            );
            if let Some(blame) = window.blame.take() {
                blames.push((window.repo.clone(), blame));
            }
        }
        self.diffs.windows.retain(|w| !w.closed);
        for (repo, (spec, line)) in blames {
            self.open_blame(repo, spec, line, ctx);
        }
    }

    /// Opens the diff of `path` in the commit `rev` (a ref or hash prefix) against its first
    /// parent, for `--demo-diff`.
    pub(super) fn open_demo_diff(&mut self, spec: &str, ctx: &egui::Context) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let Some((rev, path)) = spec.split_once(':') else {
            eprintln!("--demo-diff: expected <commit>:<path>");
            return;
        };
        let Some(ix) = repo.resolve(rev) else {
            eprintln!("--demo-diff: no commit named {rev}");
            return;
        };
        let commit = repo.commit(ix);
        let git = parterre_core::git::Git::new(&repo.path);
        let files = match git.changed_files(&commit.oid) {
            Ok(files) => files,
            Err(e) => {
                eprintln!("--demo-diff: {e}");
                return;
            }
        };
        let Some(file) = files.iter().find(|f| f.path == path) else {
            eprintln!("--demo-diff: {path} is not among the files {rev} changed");
            return;
        };
        let parent = commit.parents.first().map(|&p| repo.commit(p).oid);
        let spec = FileDiffSpec::of_commit(commit.oid, parent, file);
        self.diffs
            .open(repo.clone(), spec, &self.settings.diff_window, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parterre_core::file_diff::Version;
    use parterre_core::repo::Head;

    /// A window with a loaded diff of `old` against `new`, without git.
    fn window(old: &str, new: &str, settings: &DiffWindowSettings) -> DiffWindow {
        let rev = Rev::Commit(Oid::from_hex("0123456789012345678901234567890123456789").unwrap());
        let v = || Version {
            rev,
            path: "a.txt".into(),
        };
        let spec = FileDiffSpec {
            old: Some(v()),
            new: Some(v()),
            status: FileStatus::Modified,
            modes: [0o100644; 2],
            binary: false,
        };
        let loaded = LoadedDiff {
            spec: spec.clone(),
            content: Content::Text {
                old: old.into(),
                new: new.into(),
                invalid_bytes: 0,
            },
            textconv: None,
        };
        let repo = Arc::new(Repo::new(
            "/nowhere".into(),
            Vec::new(),
            Vec::new(),
            Head::Branch {
                name: "main".into(),
                target: None,
            },
        ));
        let (_, rx) = mpsc::channel();
        let mut w = DiffWindow::new_loading(1, repo, spec, settings, rx);
        let options = w.options;
        w.load = Load::Ready(Box::new(Ready::new(loaded, options)));
        w
    }

    /// Runs one frame of the window's contents with `events`.
    fn frame(
        ctx: &egui::Context,
        w: &mut DiffWindow,
        settings: &mut DiffWindowSettings,
        events: Vec<egui::Event>,
    ) {
        frame_with(ctx, w, settings, events, Modifiers::NONE);
    }

    /// As [`frame`], with `modifiers` held.
    fn frame_with(
        ctx: &egui::Context,
        w: &mut DiffWindow,
        settings: &mut DiffWindowSettings,
        events: Vec<egui::Event>,
        modifiers: Modifiers,
    ) {
        let mut all = vec![egui::Event::ModifiersChanged(modifiers)];
        all.extend(events);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))),
            events: all,
            ..Default::default()
        };
        // As `show` does, less the viewport. Nothing is rendered, so the texture updates are
        // discarded.
        w.poll();
        ctx.run_ui(input, |ui| w.contents(ui, settings))
            .textures_delta
            .clear();
    }

    fn click(at: egui::Pos2) -> [Vec<egui::Event>; 2] {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        [
            vec![egui::Event::PointerMoved(at), button(true)],
            vec![button(false)],
        ]
    }

    fn numbered(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}\n")).collect()
    }

    #[test]
    fn switching_the_form_from_the_toolbar_draws_the_new_form() {
        // Unified has a row more than side by side: the edited line's two versions.
        let old = numbered(40);
        let new = old.replace("line 20\n", "line twenty\n");
        let mut settings = DiffWindowSettings {
            form: DiffForm::Unified,
            fold: false,
            ..DiffWindowSettings::default()
        };
        let mut w = window(&old, &new, &settings);
        let ctx = egui::Context::default();
        frame(&ctx, &mut w, &mut settings, Vec::new());
        // The side-by-side segment is the first in the toolbar.
        for events in click(pos2(27.0, TOOLBAR / 2.0)) {
            frame(&ctx, &mut w, &mut settings, events);
        }
        frame(&ctx, &mut w, &mut settings, Vec::new());
        assert_eq!(w.form, DiffForm::SideBySide);
        assert_eq!(settings.form, DiffForm::SideBySide);
    }

    #[test]
    fn a_window_opened_at_a_line_starts_at_its_change() {
        let old = numbered(80);
        let new = old
            .replace("line 5\n", "line five\n")
            .replace("line 60\n", "line sixty\n");
        let mut settings = DiffWindowSettings::default();
        let mut w = window(&old, &new, &settings);
        w.jump = None;
        w.goto = Some(59);
        let ctx = egui::Context::default();
        frame(&ctx, &mut w, &mut settings, Vec::new());
        frame(&ctx, &mut w, &mut settings, Vec::new());
        assert_eq!(w.current, Some(1));
    }

    /// Where display column `col` of row `row` starts in the new (right) pane, side by side and
    /// unfolded, with no notes in the header.
    fn at(ctx: &egui::Context, w: &DiffWindow, row: usize, col: usize) -> egui::Pos2 {
        let font = FontId::monospace(FONT_SIZE);
        let (row_h, char_w) =
            ctx.fonts_mut(|f| (f.row_height(&font).ceil() + 3.0, f.glyph_width(&font, '0')));
        let ready = w.ready().unwrap();
        let lines = ready.diff.old.len().max(ready.diff.new.len()).max(1);
        let gutter = ((lines as f32).log10().floor() + 1.0) * char_w + 18.0;
        let mid = (1200.0 - OVERVIEW) / 2.0;
        let top = TOOLBAR + 34.0 + PANE_TITLE;
        pos2(
            mid + gutter + MARKER + col as f32 * char_w + 1.0,
            top + (row as f32 + 0.5) * row_h,
        )
    }

    fn button(at: egui::Pos2, pressed: bool, modifiers: Modifiers) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        }
    }

    fn text_window() -> (egui::Context, DiffWindow, DiffWindowSettings) {
        // An added last line keeps the header free of notes ("Content unchanged").
        let text = "one\n\ttwo three\nfour five\n";
        let mut settings = DiffWindowSettings {
            fold: false,
            ..DiffWindowSettings::default()
        };
        let mut w = window(text, &format!("{text}six\n"), &settings);
        let ctx = egui::Context::default();
        frame(&ctx, &mut w, &mut settings, Vec::new());
        (ctx, w, settings)
    }

    #[test]
    fn dragging_over_text_copies_it_as_in_the_file() {
        let (ctx, mut w, mut settings) = text_window();
        // From "two" (column 4, after the tab's spaces) to "four| five".
        let (a, b) = (at(&ctx, &w, 1, 4), at(&ctx, &w, 2, 4));
        let none = Modifiers::NONE;
        let steps = [
            vec![egui::Event::PointerMoved(a), button(a, true, none)],
            vec![egui::Event::PointerMoved(b)],
            vec![button(b, false, none)],
        ];
        for events in steps {
            frame(&ctx, &mut w, &mut settings, events);
        }
        assert!(!w.dragging);
        assert_eq!(w.selected_text().as_deref(), Some("two three\nfour"));
    }

    #[test]
    fn line_numbers_choose_whole_lines_with_their_tabs() {
        let (ctx, mut w, mut settings) = text_window();
        let (a, b) = (at(&ctx, &w, 0, 0), at(&ctx, &w, 1, 0));
        // On the numbers: well left of the text.
        let (a, b) = (a - vec2(MARKER + 12.0, 0.0), b - vec2(MARKER + 12.0, 0.0));
        let (none, shift) = (Modifiers::NONE, Modifiers::SHIFT);
        let steps = [
            (
                vec![egui::Event::PointerMoved(a), button(a, true, none)],
                none,
            ),
            (vec![button(a, false, none)], none),
            (
                vec![egui::Event::PointerMoved(b), button(b, true, shift)],
                shift,
            ),
            (vec![button(b, false, shift)], shift),
        ];
        for (events, held) in steps {
            frame_with(&ctx, &mut w, &mut settings, events, held);
        }
        assert_eq!(w.selected_text().as_deref(), Some("one\n\ttwo three\n"));
    }

    #[test]
    fn a_double_click_takes_a_word_and_ctrl_a_everything() {
        let (ctx, mut w, mut settings) = text_window();
        let p = at(&ctx, &w, 2, 6);
        let none = Modifiers::NONE;
        for _ in 0..2 {
            frame(
                &ctx,
                &mut w,
                &mut settings,
                vec![egui::Event::PointerMoved(p), button(p, true, none)],
            );
            frame(&ctx, &mut w, &mut settings, vec![button(p, false, none)]);
        }
        assert_eq!(w.selected_text().as_deref(), Some("five"));

        w.select_all();
        assert_eq!(
            w.selected_text().as_deref(),
            Some("one\n\ttwo three\nfour five\nsix\n")
        );
    }

    /// Where display column `col` of row `row` starts in the unified form, unfolded.
    fn at_unified(ctx: &egui::Context, w: &DiffWindow, row: usize, col: usize) -> egui::Pos2 {
        let right_pane = at(ctx, w, row, col);
        let font = FontId::monospace(FONT_SIZE);
        let char_w = ctx.fonts_mut(|f| f.glyph_width(&font, '0'));
        let ready = w.ready().unwrap();
        let lines = ready.diff.old.len().max(ready.diff.new.len()).max(1);
        let gutter = ((lines as f32).log10().floor() + 1.0) * char_w + 18.0;
        let mid = (1200.0 - OVERVIEW) / 2.0;
        // No pane titles, two number columns, from the left edge.
        pos2(right_pane.x - mid + gutter, right_pane.y - PANE_TITLE)
    }

    fn drag(
        ctx: &egui::Context,
        w: &mut DiffWindow,
        settings: &mut DiffWindowSettings,
        a: egui::Pos2,
        b: egui::Pos2,
    ) {
        let none = Modifiers::NONE;
        let steps = [
            vec![egui::Event::PointerMoved(a), button(a, true, none)],
            vec![egui::Event::PointerMoved(b)],
            vec![button(b, false, none)],
        ];
        for events in steps {
            frame(ctx, w, settings, events);
        }
    }

    #[test]
    fn unified_text_is_chosen_in_the_version_it_starts_on() {
        let mut settings = DiffWindowSettings {
            form: DiffForm::Unified,
            fold: false,
            ..DiffWindowSettings::default()
        };
        let mut w = window("keep\nold line\nend\n", "keep\nnew line\nend\n", &settings);
        let ctx = egui::Context::default();
        frame(&ctx, &mut w, &mut settings, Vec::new());
        // Rows: keep, −old line, +new line, end.

        // From the removed line: the old version; the added line is left out.
        let (a, b) = (at_unified(&ctx, &w, 1, 0), at_unified(&ctx, &w, 3, 3));
        drag(&ctx, &mut w, &mut settings, a, b);
        assert_eq!(w.selected_text().as_deref(), Some("old line\nend"));

        // From an unchanged line: the new version.
        let (a, b) = (at_unified(&ctx, &w, 0, 0), at_unified(&ctx, &w, 3, 3));
        drag(&ctx, &mut w, &mut settings, a, b);
        assert_eq!(w.selected_text().as_deref(), Some("keep\nnew line\nend"));

        // From an unchanged line with Ctrl held: the old version.
        let (a, b) = (at_unified(&ctx, &w, 0, 0), at_unified(&ctx, &w, 3, 3));
        let ctrl = Modifiers::COMMAND;
        let steps = [
            vec![egui::Event::PointerMoved(a), button(a, true, ctrl)],
            vec![egui::Event::PointerMoved(b)],
            vec![button(b, false, ctrl)],
        ];
        for events in steps {
            frame_with(&ctx, &mut w, &mut settings, events, ctrl);
        }
        assert_eq!(w.selected_text().as_deref(), Some("keep\nold line\nend"));

        // On the old line numbers (the first column): whole lines of the old version.
        let gutter_old = |row| at_unified(&ctx, &w, row, 0) - vec2(MARKER + 40.0, 0.0);
        let (a, b) = (gutter_old(0), gutter_old(3));
        drag(&ctx, &mut w, &mut settings, a, b);
        assert_eq!(w.selected_text().as_deref(), Some("keep\nold line\nend\n"));
    }

    fn folds(w: &DiffWindow) -> usize {
        w.shown
            .iter()
            .filter(|s| matches!(s, Shown::Fold(_)))
            .count()
    }

    #[test]
    fn opened_folds_survive_options_until_the_fold_button_folds_them_again() {
        let old = numbered(40);
        let new = old.replace("line 20\n", "line twenty\n");
        let mut settings = DiffWindowSettings::default();
        let mut w = window(&old, &new, &settings);
        let ctx = egui::Context::default();
        frame(&ctx, &mut w, &mut settings, Vec::new());
        assert_eq!(folds(&w), 2);

        // Open the first fold, as a click on it does.
        let Some(Shown::Fold(hidden)) = w.shown.first().cloned() else {
            panic!("expected a fold first: {:?}", w.shown);
        };
        let ready = w.ready().unwrap();
        let lines = fold_lines(&ready.diff.side, &hidden).unwrap();
        w.open.push(lines);
        w.dirty = true;
        frame(&ctx, &mut w, &mut settings, Vec::new());
        assert_eq!(folds(&w), 1);

        // Another whitespace setting and the other form keep it open.
        w.options.whitespace = Whitespace::IgnoreAll;
        w.set_form(DiffForm::Unified);
        frame(&ctx, &mut w, &mut settings, Vec::new());
        assert_eq!(w.ready().unwrap().options.whitespace, Whitespace::IgnoreAll);
        assert_eq!(folds(&w), 1);

        // The button, now in its third state, folds everything again and stays on.
        w.toggle_fold(&mut settings);
        frame(&ctx, &mut w, &mut settings, Vec::new());
        assert!(w.fold && w.open.is_empty());
        assert_eq!(folds(&w), 2);

        // Then it turns folding off.
        w.toggle_fold(&mut settings);
        frame(&ctx, &mut w, &mut settings, Vec::new());
        assert!(!w.fold && !settings.fold);
        assert_eq!(folds(&w), 0);
    }
}
