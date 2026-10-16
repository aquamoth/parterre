use std::ops::Range;
use std::sync::Arc;

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{
    self, Color32, FontId, Key, Modifiers, Pos2, Rect, Response, RichText, ScrollArea, Sense,
    Stroke, Ui, UiBuilder, Vec2, pos2, vec2,
};
use parterre_core::blame::BlameSpec;
use parterre_core::changed_files::FileStatus;
use parterre_core::file_diff::{
    self, CONTEXT_LINES, Content, DiffLine, DiffOptions, FileDiff, FileDiffSpec, Found, LineKind,
    LoadedDiff, Note, Rev, Row, Shown, Side, Version, Whitespace, WordMode, display_column, fold,
    fold_lines,
};
use parterre_core::find::{self, Step};
use parterre_core::glyphs;
use parterre_core::text::word_at;
use parterre_core::{Oid, Repo};
use parterre_highlight::{self as highlight, Engine, Spans};
use parterre_util::Cancel;

use super::text_view::{
    Colors, Find, FindInText, HEADER_HEIGHT, LINE_END, Load, OVERVIEW_WIDTH, SCROLLBAR_HEIGHT,
    SIDEWAYS_MARGIN, ScrollView, TOOL_GAP, TextAt, TextMetrics, TextRow, chosen_columns, colors,
    file_window_title, hscrollbar, line_numbers_width, message, overview_background,
    overview_found, overview_scale, overview_scroll, overview_view, paint_text, raw_slice, reveal,
    reveal_columns, scroll_sideways, text_font, toolbar_row, with_arrows,
};
use super::viewport::{self, Viewport};
use super::{ParterreApp, syntax};
use crate::keys;
use crate::settings::{DiffForm, DiffWindowSettings};
use crate::text_size;
use crate::usage::Screen;
use crate::widgets;
use crate::worker;

const NOTES_HEIGHT: f32 = 20.0;
const PANE_TITLE_HEIGHT: f32 = 24.0;
const MARKER_WIDTH: f32 = 16.0;
const ROWS_ABOVE_CHANGE: usize = 3;

#[derive(Debug, Default)]
pub struct DiffWindows {
    windows: Vec<DiffWindow>,
    last_id: u64,
    engine: Engine,
}

impl DiffWindows {
    pub fn new(engine: Engine) -> DiffWindows {
        DiffWindows {
            engine,
            ..DiffWindows::default()
        }
    }

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

    /// [`DiffWindows::open`], at the change at `line` (from 0) of the new version.
    pub fn open_at(
        &mut self,
        repo: Arc<Repo>,
        spec: FileDiffSpec,
        line: Option<usize>,
        settings: &DiffWindowSettings,
        ctx: &egui::Context,
    ) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.spec == spec) {
            // The files on disk may have changed since.
            if spec.reads_working_tree() {
                *w = DiffWindow::new(w.id, repo, spec, settings, &self.engine, ctx);
            }
            w.goto_line = line.or(w.goto_line);
            w.viewport.bring_to_front = true;
            return;
        }
        self.last_id += 1;
        let mut w = DiffWindow::new(self.last_id, repo, spec, settings, &self.engine, ctx);
        w.goto_line = line;
        self.windows.push(w);
    }

    pub fn close_all(&mut self) {
        self.windows.clear();
    }

    pub fn is_loading(&self) -> bool {
        self.windows.iter().any(|w| w.load.is_loading())
    }
}

#[derive(Debug)]
struct Ready {
    loaded: LoadedDiff,
    diff: FileDiff,
    notes: Vec<Note>,
    options: DiffOptions,
    /// `[old, new]`: each line's syntax spans on its display text, when the language is known.
    /// They don't depend on the options.
    syntax: Option<[Vec<Spans>; 2]>,
}

impl Ready {
    fn new(loaded: LoadedDiff, options: DiffOptions, engine: &Engine, cancel: &Cancel) -> Ready {
        let diff = Self::file_diff(&loaded, options);
        let syntax = match (&loaded.content, highlight::language_of(loaded.spec.path())) {
            (Content::Text { old, new, .. }, Some(language)) => {
                let on_display = |lines: &[DiffLine], spans: Vec<Spans>| -> Vec<Spans> {
                    lines
                        .iter()
                        .map(|l| {
                            let line_spans =
                                spans.get(l.index as usize).map_or(&[][..], Vec::as_slice);
                            syntax::display_spans(&l.raw, line_spans)
                        })
                        .collect()
                };
                let old_spans = engine.highlight(language, old, cancel);
                let new_spans = engine.highlight(language, new, cancel);
                Some([
                    on_display(&diff.old, old_spans),
                    on_display(&diff.new, new_spans),
                ])
            }
            _ => None,
        };
        Ready {
            notes: diff.notes(&loaded, options.whitespace),
            loaded,
            diff,
            options,
            syntax,
        }
    }

    fn file_diff(loaded: &LoadedDiff, options: DiffOptions) -> FileDiff {
        match &loaded.content {
            Content::Text { old, new, .. } => FileDiff::new(old, new, options),
            _ => FileDiff::default(),
        }
    }

    fn rediff(&mut self, options: DiffOptions) {
        self.diff = Self::file_diff(&self.loaded, options);
        self.notes = self.diff.notes(&self.loaded, options.whitespace);
        self.options = options;
    }
}

/// Text chosen with the mouse in one side, from `anchor` to `head`: each a row of the form and
/// a display column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Selection {
    /// Side by side, a pane's; unified, that of the line the choosing started on.
    side: Side,
    anchor: (usize, usize),
    head: (usize, usize),
    whole_lines: bool,
}

impl Selection {
    fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    fn columns(&self, side: Side, row: usize) -> Option<Range<usize>> {
        if side != self.side {
            return None;
        }
        let (start, end) = self.ordered();
        if self.whole_lines {
            return (start.0..=end.0).contains(&row).then_some(0..LINE_END);
        }
        chosen_columns(start, end, row)
    }

    fn is_empty(&self) -> bool {
        !self.whole_lines && self.anchor == self.head
    }
}

#[derive(Clone, Copy)]
enum Folding {
    Folded,
    SomeOpened,
    WholeFile,
}

#[derive(Default)]
struct RowInput {
    /// The new version's lines of a fold clicked open.
    opened_fold: Option<Range<u32>>,
    press: Option<Press>,
    hover: Option<Hover>,
    first_row: Option<usize>,
    last_row: Option<usize>,
}

/// The primary button went down on a row's text or line numbers.
struct Press {
    row: usize,
    side: Side,
    col: usize,
    on_numbers: bool,
}

/// The row under the pointer, and the display column under it in each side.
struct Hover {
    row: usize,
    old_col: usize,
    new_col: usize,
}

impl Hover {
    fn col(&self, side: Side) -> usize {
        match side {
            Side::Old => self.old_col,
            Side::New => self.new_col,
        }
    }
}

#[derive(Debug)]
struct DiffWindow {
    id: u64,
    repo: Arc<Repo>,
    spec: FileDiffSpec,
    /// The viewport builder must not change while the window is open.
    initial_size: Vec2,
    load: Load<Ready>,
    form: DiffForm,
    options: DiffOptions,
    fold: bool,
    /// By new-version line, so that they stay open when the form or the options change.
    opened_folds: Vec<Range<u32>>,
    shown: Vec<Shown>,
    shown_dirty: bool,
    /// Indices into `shown`.
    change_starts: Vec<usize>,
    current_change: Option<usize>,
    jump_to_change: Option<usize>,
    scroll_to: Option<f32>,
    /// The scroll offset a jump to `current_change` left the view at: while the view stays
    /// there, so does the change, even where the view couldn't scroll to it.
    pinned: Option<f32>,
    /// Shared by both panes.
    scroll_x: f32,
    scroll_y: f32,
    top: usize,
    selection: Option<Selection>,
    dragging: bool,
    viewport: Viewport,
    closed: bool,
    blame_request: Option<(BlameSpec, Option<usize>)>,
    fetch_requested: bool,
    /// From 0, in the new version; its change is gone to once loaded.
    goto_line: Option<usize>,
    find: Find<Found>,
    stop_colouring: Cancel,
}

impl Drop for DiffWindow {
    fn drop(&mut self) {
        self.stop_colouring.cancel();
    }
}

impl DiffWindow {
    fn new(
        id: u64,
        repo: Arc<Repo>,
        spec: FileDiffSpec,
        settings: &DiffWindowSettings,
        engine: &Engine,
        ctx: &egui::Context,
    ) -> DiffWindow {
        let mut window = DiffWindow::unloaded(id, repo, spec, settings);
        let git = parterre_core::git::Git::new(&window.repo.path);
        let (job, options) = (window.spec.clone(), window.options);
        let (engine, stop) = (engine.clone(), window.stop_colouring.clone());
        window.load = Load::Loading(worker::spawn(ctx, move || {
            git.load_file_diff(&job)
                .map(|loaded| Ready::new(loaded, options, &engine, &stop))
                .map_err(|e| e.to_string())
        }));
        window
    }

    /// Not loading yet: see [`DiffWindow::new`].
    fn unloaded(
        id: u64,
        repo: Arc<Repo>,
        spec: FileDiffSpec,
        settings: &DiffWindowSettings,
    ) -> DiffWindow {
        let [w, h] = settings.size;
        DiffWindow {
            id,
            repo,
            spec,
            initial_size: vec2(w, h),
            load: Load::Failed(String::new()),
            form: settings.form,
            options: DiffOptions {
                words: settings.words,
                whitespace: settings.whitespace,
            },
            fold: settings.fold,
            opened_folds: Vec::new(),
            shown: Vec::new(),
            shown_dirty: true,
            change_starts: Vec::new(),
            current_change: None,
            jump_to_change: Some(0),
            scroll_to: None,
            pinned: None,
            scroll_x: 0.0,
            scroll_y: 0.0,
            top: 0,
            selection: None,
            dragging: false,
            viewport: Viewport::default(),
            closed: false,
            blame_request: None,
            fetch_requested: false,
            goto_line: None,
            find: Find::default(),
            stop_colouring: Cancel::new(),
        }
    }

    fn viewport_id(&self) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(("diff", self.id))
    }

    fn title(&self) -> String {
        let version = self.spec.new.as_ref().or(self.spec.old.as_ref());
        file_window_title(self.spec.path(), version.map(|v| v.rev), &self.repo, "Diff")
    }

    fn poll(&mut self) {
        if let Some(result) = self.load.received() {
            self.load = match result {
                Ok(ready) => Load::Ready(Box::new(ready)),
                Err(e) => Load::Failed(e),
            };
            self.shown_dirty = true;
            self.refind();
        }
        // The options changed while the worker ran, or since.
        if let Load::Ready(ready) = &mut self.load
            && ready.options != self.options
        {
            ready.rediff(self.options);
            // The selection was made on the rows of the old options.
            self.selection = None;
            self.shown_dirty = true;
            self.jump_to_change = Some(self.current_change.unwrap_or(0));
            self.refind();
        }
    }

    fn ready(&self) -> Option<&Ready> {
        self.load.ready()
    }

    fn form_ix(&self) -> usize {
        match self.form {
            DiffForm::SideBySide => 0,
            DiffForm::Unified => 1,
        }
    }

    fn rows_and_changes(diff: &FileDiff, form: DiffForm) -> (&[Row], &[usize]) {
        match form {
            DiffForm::SideBySide => (&diff.side, &diff.side_change_starts),
            DiffForm::Unified => (&diff.unified, &diff.unified_change_starts),
        }
    }

    fn diff_rows(&self) -> Option<(&FileDiff, &[Row])> {
        let diff = &self.ready()?.diff;
        Some((diff, Self::rows_and_changes(diff, self.form).0))
    }

    fn refresh(&mut self) {
        if !self.shown_dirty {
            return;
        }
        self.shown_dirty = false;
        let Load::Ready(ready) = &self.load else {
            self.shown.clear();
            self.change_starts.clear();
            return;
        };
        let (rows, changes) = Self::rows_and_changes(&ready.diff, self.form);
        let shown = if self.fold {
            fold(rows, &ready.diff, &self.opened_folds)
        } else {
            (0..rows.len()).map(Shown::Row).collect()
        };
        self.change_starts = changes
            .iter()
            .filter_map(|&c| {
                let i = shown_index(&shown, c);
                (shown.get(i) == Some(&Shown::Row(c))).then_some(i)
            })
            .collect();
        self.shown = shown;
    }

    /// The change line `line` (from 0) of the new version is in, or the last one before it.
    fn change_at(&self, line: usize) -> Option<usize> {
        let (_, rows) = self.diff_rows()?;
        let at = self.shown.iter().position(
            |s| matches!(s, Shown::Row(r) if rows[*r].new.is_some_and(|n| n as usize >= line)),
        )?;
        self.change_starts.iter().rposition(|&p| p <= at)
    }

    fn set_form(&mut self, form: DiffForm) {
        if self.form != form {
            self.form = form;
            self.shown_dirty = true;
            self.selection = None;
            self.jump_to_change = Some(self.current_change.unwrap_or(0));
            self.refind();
        }
    }

    /// Opens the fold `row` is hidden in, if it is, as far around it as a change's context.
    fn unfold_at(&mut self, row: usize) {
        self.refresh();
        let Some(Shown::Fold(_)) = self.shown.get(shown_index(&self.shown, row)) else {
            return;
        };
        // Folded rows are unchanged, so they have a new line.
        let Some(n) = self.diff_rows().and_then(|(_, rows)| rows.get(row)?.new) else {
            return;
        };
        let context = CONTEXT_LINES as u32;
        self.opened_folds
            .push(n.saturating_sub(context)..n + context + 1);
        self.shown_dirty = true;
        self.refresh();
    }

    fn folding(&self) -> Folding {
        match (self.fold, self.opened_folds.is_empty()) {
            (true, true) => Folding::Folded,
            (true, false) => Folding::SomeOpened,
            (false, _) => Folding::WholeFile,
        }
    }

    /// Folded, then the whole file, then folded again; folds opened by hand fold again first.
    fn toggle_fold(&mut self, settings: &mut DiffWindowSettings) {
        if !matches!(self.folding(), Folding::SomeOpened) {
            self.fold = !self.fold;
            settings.fold = self.fold;
        }
        self.opened_folds.clear();
        self.shown_dirty = true;
        self.jump_to_change = Some(self.current_change.unwrap_or(0));
    }

    fn step_change(&mut self, step: Step) {
        let changes = self.change_starts.len();
        if changes == 0 {
            return;
        }
        let target = match (self.current_change, step) {
            (None, Step::Next) => 0,
            (None, Step::Previous) => return,
            (Some(c), Step::Next) => (c + 1).min(changes - 1),
            (Some(c), Step::Previous) => c.saturating_sub(1),
        };
        self.jump_to_change = Some(target);
    }

    fn handle_keys(&mut self, ui: &Ui, settings: &mut DiffWindowSettings) {
        if self.find_keys(ui) {
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                self.close_find(ui.ctx());
            }
            return;
        }
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
            self.select_all();
        }
        let (switch_form, previous, next, close) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::D),
                // Shift+F7 first: a plain F7 would match it too.
                i.consume_key(Modifiers::COMMAND, Key::ArrowUp)
                    || i.consume_key(Modifiers::SHIFT, Key::F7),
                i.consume_key(Modifiers::COMMAND, Key::ArrowDown)
                    || i.consume_key(Modifiers::NONE, Key::F7),
                keys::close_window().consume(i),
            )
        });
        if switch_form {
            let other = match self.form {
                DiffForm::SideBySide => DiffForm::Unified,
                DiffForm::Unified => DiffForm::SideBySide,
            };
            self.set_form(other);
            settings.form = other;
        }
        if next {
            self.step_change(Step::Next);
        }
        if previous {
            self.step_change(Step::Previous);
        }
        if close {
            self.closed = true;
        }
    }

    fn contents(&mut self, ui: &mut Ui, settings: &mut DiffWindowSettings, syntax: &mut bool) {
        self.refresh();
        let c = colors(ui);
        self.toolbar(ui, settings, syntax);
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
                    self.body(&mut child, body, *syntax, &c);
                }
            },
        }
        ui.allocate_rect(body, Sense::hover());
    }

    fn toolbar(&mut self, ui: &mut Ui, settings: &mut DiffWindowSettings, syntax: &mut bool) {
        let (rect, mut bar) = toolbar_row(ui);
        let ui = &mut bar;

        let forms = [
            (DiffForm::SideBySide, glyphs::DIFF_SIDE_BY_SIDE),
            (DiffForm::Unified, glyphs::DIFF_UNIFIED),
        ];
        let picked = widgets::segmented(ui, self.form, &forms, |form, r| {
            let text = match form {
                DiffForm::SideBySide => "Side by side",
                DiffForm::Unified => "Unified",
            };
            widgets::tip(r, text, &keys::Shortcut::command(Key::D).label())
        });
        if let Some(form) = picked {
            self.set_form(form);
            settings.form = form;
        }
        ui.add_space(TOOL_GAP);

        let changes = self.change_starts.len();
        let at = self.current_change;
        let previous = ui
            .add_enabled_ui(at.is_some_and(|c| c > 0), |ui| {
                widgets::tip(
                    widgets::icon_button(ui, glyphs::CHEVRON_UP, false),
                    "Previous change",
                    &keys::Shortcut::command(Key::ArrowUp).label(),
                )
            })
            .inner;
        let next = ui
            .add_enabled_ui(changes > 0 && at.is_none_or(|c| c + 1 < changes), |ui| {
                widgets::tip(
                    widgets::icon_button(ui, glyphs::CHEVRON_DOWN, false),
                    "Next change",
                    &keys::Shortcut::command(Key::ArrowDown).label(),
                )
            })
            .inner;
        if previous.clicked() {
            self.step_change(Step::Previous);
        }
        if next.clicked() {
            self.step_change(Step::Next);
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
        ui.add_space(TOOL_GAP);

        let folding = self.folding();
        let (title, body) = match folding {
            Folding::Folded => (
                "Unchanged lines folded",
                "Click a fold to open it, or here to show the whole file.",
            ),
            Folding::SomeOpened => ("Some folds are open", "Click to fold them all again."),
            Folding::WholeFile => (
                "Whole file",
                "Click to fold the unchanged stretches between changes.",
            ),
        };
        let r = widgets::tip_explained(fold_button(ui, folding), title, "", body);
        if r.clicked() {
            self.toggle_fold(settings);
        }
        ui.add_space(TOOL_GAP);

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
        ui.add_space(TOOL_GAP);
        widgets::syntax_button(ui, syntax);

        let no_blame = if self.spec.is_submodule() {
            Some("A submodule has no lines to blame")
        } else if self.spec.binary {
            Some("A binary file has no lines to blame")
        } else {
            None
        };
        // Word mode and Blame at the right edge take about as much room as the tools on the
        // left, so that the find field between them can be centred: in the window, as in the
        // others, if it fits between the tools; else in the middle of the room there is,
        // squeezed rather than the tools.
        let room = ui.available_rect_before_wrap();
        let room = room.with_max_x(room.right() - widgets::trailing_width(ui, "diff-tools"));
        let width = (room.width() - 16.0).clamp(60.0, 380.0);
        let (lo, hi) = (room.left() + 8.0, room.right() - 8.0 - width);
        let left = if lo <= hi {
            (rect.center().x - width / 2.0).clamp(lo, hi)
        } else {
            room.center().x - width / 2.0
        };
        let at = Rect::from_center_size(
            pos2(left + width / 2.0, rect.center().y),
            vec2(width, widgets::BUTTON_SIZE),
        );
        let layout = egui::Layout::left_to_right(egui::Align::Center);
        ui.scope_builder(UiBuilder::new().max_rect(at).layout(layout), |ui| {
            self.find_field(ui, width, "Find in the diff");
        });

        widgets::trailing(ui, "diff-tools", |ui| {
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

            ui.add_space(TOOL_GAP);

            let r = ui.add_enabled_ui(no_blame.is_none(), |ui| widgets::text_button(ui, "Blame"));
            let r = match no_blame {
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
                self.blame_request = self.blame_spec();
            }
        });
    }

    /// The new version, or the old one of a deleted file, and its line at the change in view.
    fn blame_spec(&self) -> Option<(BlameSpec, Option<usize>)> {
        let (version, old) = match (&self.spec.new, &self.spec.old) {
            (Some(v), _) => (v, false),
            (None, Some(v)) => (v, true),
            (None, None) => return None,
        };
        let line = self.diff_rows().and_then(|(_, rows)| {
            let at = *self.change_starts.get(self.current_change?)?;
            let Shown::Row(r) = self.shown.get(at)? else {
                return None;
            };
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

    fn header(&self, ui: &mut Ui, c: &Colors) {
        let notes: &[Note] = self.ready().map_or(&[], |r| r.notes.as_slice());
        let height = if notes.is_empty() {
            HEADER_HEIGHT
        } else {
            HEADER_HEIGHT + NOTES_HEIGHT
        };
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
        let middle_y = rect.top() + HEADER_HEIGHT / 2.0;
        let g = painter.layout_job(job);
        painter.galley(
            pos2(rect.left() + 12.0, middle_y - g.size().y / 2.0),
            g,
            text,
        );
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
                pos2(
                    rect.right() - 12.0 - g.size().x,
                    middle_y - g.size().y / 2.0,
                ),
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
            painter.galley(
                pos2(rect.left() + 12.0, rect.top() + HEADER_HEIGHT),
                g,
                c.note,
            );
        }
    }

    fn body(&mut self, ui: &mut Ui, full: Rect, syntax_on: bool, c: &Colors) {
        // The toolbar, drawn just before, may have changed the form or the folding.
        self.refresh();
        if let Some(line) = self.goto_line.take()
            && let Some(k) = self.change_at(line)
        {
            self.jump_to_change = Some(k);
        }
        let found = self.find.take_reveal().cloned();
        if let Some(m) = &found {
            self.unfold_at(m.row);
        }
        let Load::Ready(ready) = &self.load else {
            return;
        };
        let diff = &ready.diff;
        let TextMetrics { row_h, char_w } = TextMetrics::new(ui.ctx());
        let numbers_w = numbers_width(diff, char_w);
        let side_by_side = self.form == DiffForm::SideBySide;

        let mut top = full.top();
        if side_by_side {
            let bar = Rect::from_min_max(
                full.min,
                pos2(full.right() - OVERVIEW_WIDTH, top + PANE_TITLE_HEIGHT),
            );
            self.pane_titles(ui, bar, c);
            top = bar.bottom();
        }

        let body_w = full.width() - OVERVIEW_WIDTH;
        let text_w = if side_by_side {
            body_w / 2.0 - numbers_w - MARKER_WIDTH
        } else {
            body_w - 2.0 * numbers_w - MARKER_WIDTH
        };
        let content_w = diff.widest as f32 * char_w + 24.0;
        let max_scroll_x = (content_w - text_w).max(0.0);
        let bottom = full.bottom()
            - if max_scroll_x > 0.0 {
                SCROLLBAR_HEIGHT
            } else {
                0.0
            };
        let area = Rect::from_min_max(
            pos2(full.left(), top),
            pos2(full.right() - OVERVIEW_WIDTH, bottom),
        );

        let (rows, _) = Self::rows_and_changes(diff, self.form);
        if let Some(m) = found {
            let at = shown_index(&self.shown, m.row);
            if let Some(offset) = reveal(at, self.scroll_y, area.height(), row_h) {
                self.scroll_to = Some(offset);
            }
            if let Some(x) = reveal_columns(self.scroll_x, &m.columns, char_w, text_w) {
                self.scroll_x = x;
            }
        }
        let mut scroll = ScrollArea::vertical()
            .auto_shrink(false)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .id_salt(("diff", self.id, self.form_ix(), self.fold));
        if let Some(offset) = self.scroll_to.take() {
            scroll = scroll.vertical_scroll_offset(offset.max(0.0));
            self.pinned = None;
        }
        let jumped = self
            .jump_to_change
            .take()
            .filter(|&k| k < self.change_starts.len());
        if let Some(&at) = jumped.and_then(|k| self.change_starts.get(k)) {
            let above = at.saturating_sub(ROWS_ABOVE_CHANGE);
            scroll = scroll.vertical_scroll_offset(above as f32 * row_h);
            self.scroll_x = match first_word_column(diff, rows, &self.shown, at) {
                Some(col) if col as f32 * char_w > text_w - 40.0 => {
                    (col as f32 * char_w - SIDEWAYS_MARGIN).clamp(0.0, max_scroll_x)
                }
                _ => 0.0,
            };
        }
        scroll_sideways(ui, area, &mut self.scroll_x, max_scroll_x);

        let (pressed, shift, ctrl) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.modifiers.shift,
                i.modifiers.command,
            )
        });
        let painter = RowPainter {
            diff,
            rows,
            side_by_side,
            style: RowStyle {
                row_h,
                numbers_w,
                scroll_x: self.scroll_x,
                font: text_font(),
                colors: c,
            },
            syntax: ready.syntax.as_ref().filter(|_| syntax_on),
            selection: self.selection,
            dragging: self.dragging,
            query: &self.find.query,
            current_found: self.find.current_match(),
            pointer: ui.input(|i| i.pointer.interact_pos()),
            pressed,
            extend: self.selection.filter(|_| shift).map(|s| s.side),
            ctrl,
        };
        let shown = &self.shown;
        let mut input = RowInput::default();
        let mut child = ui.new_child(UiBuilder::new().max_rect(area));
        child.set_clip_rect(area.intersect(ui.clip_rect()));
        child.spacing_mut().item_spacing = Vec2::ZERO;
        let out = scroll.show_rows(&mut child, row_h, shown.len(), |ui, range| {
            for i in range {
                let (rect, response) = ui.allocate_exact_size(
                    vec2(ui.available_width(), row_h),
                    crate::focus::pointer_only(),
                );
                match &shown[i] {
                    Shown::Fold(hidden) => {
                        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                        fold_row(ui, rect, hidden.len(), response.hovered(), c);
                        if response.clicked() && !painter.dragging {
                            input.opened_fold = fold_lines(rows, hidden);
                        }
                    }
                    Shown::Row(r) => painter.paint(ui, rect, &response, *r, &mut input),
                }
            }
        });
        let view = ScrollView::of(&out);
        self.scroll_y = view.offset;
        if let Some(r) = input.first_row {
            self.top = r;
        }
        self.update_current_change(jumped, row_h);

        let strip = Rect::from_min_max(pos2(full.right() - OVERVIEW_WIDTH, top), full.max);
        self.overview(ui, strip, view, row_h, c);
        self.select(ui, &input, area, row_h);
        if let Some(lines) = input.opened_fold {
            self.opened_folds.push(lines);
            self.shown_dirty = true;
        }
        let id = egui::Id::new(("diff-overview", self.id));
        if let Some(offset) = overview_scroll(ui, id, strip, self.shown.len(), row_h, view) {
            self.scroll_to = Some(offset);
        }
        if max_scroll_x > 0.0 {
            let track = Rect::from_min_max(
                pos2(full.left(), bottom),
                pos2(full.right() - OVERVIEW_WIDTH, full.bottom()),
            );
            let id = egui::Id::new(("diff-hbar", self.id));
            let visible = text_w / content_w;
            hscrollbar(ui, id, track, &mut self.scroll_x, visible, max_scroll_x, c);
        }
    }

    /// The change in view: the one jumped to while the view stays put, else the last one at or
    /// above the reading line (as far down as a jump puts a change).
    fn update_current_change(&mut self, jumped: Option<usize>, row_h: f32) {
        let offset = self.scroll_y;
        if let Some(k) = jumped {
            self.current_change = Some(k);
            self.pinned = Some(offset);
        } else if self.pinned.is_none_or(|p| (p - offset).abs() > 0.5) {
            self.pinned = None;
            let first = (offset / row_h).round() as usize;
            let starts = &self.change_starts;
            self.current_change = starts
                .iter()
                .rposition(|&p| p <= first + ROWS_ABOVE_CHANGE)
                .or((!starts.is_empty() && first == 0).then_some(0));
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

    fn overview(&self, ui: &Ui, strip: Rect, view: ScrollView, row_h: f32, c: &Colors) {
        let Some((diff, rows)) = self.diff_rows() else {
            return;
        };
        overview_background(ui, strip, c);
        let painter = ui.painter();
        let scale = overview_scale(strip, self.shown.len(), row_h);
        let side_by_side = self.form == DiffForm::SideBySide;
        let mark = |i: usize, kind: LineKind| {
            let y = strip.top() + i as f32 * scale;
            let color = if kind == LineKind::Removed {
                c.removed
            } else {
                c.added
            };
            let (x0, x1) = match (side_by_side, kind) {
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
        for (i, s) in self.shown.iter().enumerate() {
            let Shown::Row(r) = s else { continue };
            let row = rows[*r];
            let kind = |side| diff.line(row, side).map(|l| l.kind);
            if kind(Side::Old) == Some(LineKind::Removed) {
                mark(i, LineKind::Removed);
            }
            if kind(Side::New) == Some(LineKind::Added) {
                mark(i, LineKind::Added);
            }
        }
        let found_rows = self
            .find
            .matches
            .iter()
            .map(|m| shown_index(&self.shown, m.row));
        overview_found(ui, strip, scale, found_rows, c);
        overview_view(ui, strip, view, c);
    }

    fn select(&mut self, ui: &Ui, input: &RowInput, area: Rect, row_h: f32) {
        let (down, shift, double) = ui.input(|i| {
            (
                i.pointer.primary_down(),
                i.modifiers.shift,
                i.pointer
                    .button_double_clicked(egui::PointerButton::Primary),
            )
        });
        if let Some(press) = &input.press {
            let col = if press.on_numbers {
                LINE_END
            } else {
                press.col
            };
            let here = (press.row, col);
            self.selection = match self.selection {
                Some(s) if shift && s.side == press.side => Some(Selection { head: here, ..s }),
                _ => Some(Selection {
                    side: press.side,
                    anchor: here,
                    head: here,
                    whole_lines: press.on_numbers,
                }),
            };
            self.dragging = true;
        }
        // Not only after a press: a quick double-click can end in the frame it started.
        if double
            && let Some(hover) = &input.hover
            && let Some(s) = self.selection
            && !s.whole_lines
            && let Some(word) = self.word_at(s.side, hover.row, hover.col(s.side))
        {
            self.selection = Some(Selection {
                anchor: (hover.row, word.start),
                head: (hover.row, word.end),
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
        if let Some(hover) = &input.hover {
            let col = if s.whole_lines {
                LINE_END
            } else {
                hover.col(s.side)
            };
            s.head = (hover.row, col);
        } else if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
            // Past the top or bottom: take the first or last row drawn, and scroll on.
            if p.y < area.top() {
                if let Some(first) = input.first_row {
                    s.head = (first, 0);
                }
                self.scroll_to = Some(self.scroll_y - row_h);
            } else if p.y > area.bottom() {
                if let Some(last) = input.last_row {
                    s.head = (last, LINE_END);
                }
                self.scroll_to = Some(self.scroll_y + row_h);
            }
            ui.ctx().request_repaint();
        }
        self.selection = Some(s);
    }

    fn word_at(&self, side: Side, row: usize, col: usize) -> Option<Range<usize>> {
        let (diff, rows) = self.diff_rows()?;
        let line = diff.line(*rows.get(row)?, side)?;
        word_at(&line.text, col)
    }

    /// Chooses every line of the side in use.
    fn select_all(&mut self) {
        let Some((_, rows)) = self.diff_rows() else {
            return;
        };
        let Some(last) = rows.len().checked_sub(1) else {
            return;
        };
        let side = self.selection.map_or(Side::New, |s| s.side);
        self.selection = Some(Selection {
            side,
            anchor: (0, 0),
            head: (last, LINE_END),
            whole_lines: true,
        });
    }

    fn show(
        &mut self,
        ctx: &egui::Context,
        settings: &mut DiffWindowSettings,
        syntax: &mut bool,
        text_size: &mut f32,
        window_theme: Option<egui::SystemTheme>,
        icon: &Arc<egui::IconData>,
    ) {
        let id = self.viewport_id();
        let builder = viewport::builder(self.title(), self.initial_size, [520.0, 320.0], icon);
        self.viewport.before_show(ctx, id, Screen::Diff);
        ctx.show_viewport_immediate(id, builder, |ui, class| {
            let input = self.viewport.begin(ui, class, window_theme);
            self.poll();
            if let Some(input) = input {
                input.keep_size(&mut settings.size);
                self.fetch_requested |= ui.input_mut(|i| keys::fetch().consume(i));
                self.handle_keys(ui, settings);
                text_size::read_input(ui, text_size, true);
                if input.close_requested {
                    self.closed = true;
                }
            }
            if let Some(text) = self.copied(ui) {
                ui.ctx().copy_text(text);
            }
            viewport::central_panel(ui).show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                self.contents(ui, settings, syntax);
            });
        });
    }
}

impl FindInText for DiffWindow {
    type Place = Found;

    fn find(&self) -> &Find<Found> {
        &self.find
    }

    fn find_mut(&mut self) -> &mut Find<Found> {
        &mut self.find
    }

    fn find_id(&self) -> egui::Id {
        egui::Id::new(("diff-find", self.id))
    }

    fn places(&self) -> Vec<Found> {
        let Some(ready) = self.ready() else {
            return Vec::new();
        };
        let query = &self.find.query;
        match self.form {
            DiffForm::SideBySide => file_diff::find_side_by_side(&ready.diff, query),
            DiffForm::Unified => file_diff::find_unified(&ready.diff, query),
        }
    }

    fn top(&self) -> usize {
        self.top
    }

    fn chosen_in_line(&self) -> Option<String> {
        let s = self.selection?;
        let (start, end) = s.ordered();
        (!s.whole_lines && start.0 == end.0 && end.1 != LINE_END)
            .then(|| self.selected_text())
            .flatten()
    }

    /// A line per row; whole lines end with a newline.
    fn selected_text(&self) -> Option<String> {
        let s = self.selection.filter(|s| !s.is_empty())?;
        let (diff, rows) = self.diff_rows()?;
        let (start, end) = s.ordered();
        let mut lines = Vec::new();
        for (r, &row) in rows.iter().enumerate().take(end.0 + 1).skip(start.0) {
            let Some(line) = diff.line(row, s.side) else {
                continue;
            };
            lines.push(raw_slice(&line.raw, s.columns(s.side, r)?));
        }
        let mut text = lines.join("\n");
        if s.whole_lines || end.1 == LINE_END {
            text.push('\n');
        }
        Some(text)
    }
}

/// What every row of a frame is drawn with.
struct RowStyle<'a> {
    row_h: f32,
    numbers_w: f32,
    scroll_x: f32,
    font: FontId,
    colors: &'a Colors,
}

/// Paints the rows of a frame, noting what the pointer does over them.
struct RowPainter<'a> {
    diff: &'a FileDiff,
    rows: &'a [Row],
    side_by_side: bool,
    style: RowStyle<'a>,
    syntax: Option<&'a [Vec<Spans>; 2]>,
    selection: Option<Selection>,
    dragging: bool,
    query: &'a str,
    current_found: Option<&'a Found>,
    pointer: Option<Pos2>,
    pressed: bool,
    /// With Shift held, a press extends the selection, in its side.
    extend: Option<Side>,
    ctrl: bool,
}

/// A row's text in one side, or unified in both.
struct DrawnColumn {
    side: Side,
    rect: Rect,
    numbers_w: f32,
    text: Option<TextAt>,
}

impl RowPainter<'_> {
    fn paint(&self, ui: &Ui, rect: Rect, response: &Response, r: usize, input: &mut RowInput) {
        input.first_row.get_or_insert(r);
        input.last_row = Some(r);
        let columns: Vec<DrawnColumn> = if self.side_by_side {
            self.paint_sides(ui, rect, r).into()
        } else {
            vec![self.paint_unified(ui, rect, r)]
        };
        let Some(p) = self.pointer.filter(|p| rect.y_range().contains(p.y)) else {
            return;
        };
        let col_at = |column: &DrawnColumn| column.text.as_ref().map_or(0, |t| t.column_at(p));
        let old_col = col_at(&columns[0]);
        // Unified, the one column stands for both sides.
        let new_col = columns.get(1).map_or(old_col, col_at);
        input.hover = Some(Hover {
            row: r,
            old_col,
            new_col,
        });
        let Some(column) = columns.iter().find(|c| c.rect.x_range().contains(p.x)) else {
            return;
        };
        let on_numbers = p.x < column.rect.left() + column.numbers_w;
        if !on_numbers && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        let side = if self.side_by_side {
            column.side
        } else {
            let row = self.rows[r];
            let line = self.diff.line(row, row.unified_side());
            self.press_side(line, p.x, column.rect.left())
        };
        if self.pressed && response.hovered() {
            input.press = Some(Press {
                row: r,
                side,
                col: col_at(column),
                on_numbers,
            });
        }
    }

    fn paint_sides(&self, ui: &Ui, rect: Rect, r: usize) -> [DrawnColumn; 2] {
        let row = self.rows[r];
        let mid = rect.center().x;
        let halves = [
            (Side::Old, rect.left()..=mid),
            (Side::New, mid..=rect.right()),
        ];
        let drawn = halves.map(|(side, xs)| {
            let half = Rect::from_x_y_ranges(xs, rect.y_range());
            let line = self.diff.line(row, side);
            let marks = Marks {
                chosen: self
                    .selection
                    .and_then(|s| Some((s.columns(side, r)?, s.whole_lines))),
                query: self.query,
                current_found: self.current_found_columns(r, side),
            };
            let spans = line.and_then(|l| spans_of(self.syntax, l, side));
            let text = paint_line(ui, half, line, spans, Numbers::Own, &self.style, &marks);
            DrawnColumn {
                side,
                rect: half,
                numbers_w: self.style.numbers_w,
                text,
            }
        });
        ui.painter().vline(
            mid,
            rect.y_range(),
            Stroke::new(1.0, self.style.colors.line),
        );
        drawn
    }

    fn paint_unified(&self, ui: &Ui, rect: Rect, r: usize) -> DrawnColumn {
        let row = self.rows[r];
        let side = row.unified_side();
        let line = self.diff.line(row, side);
        // Only lines of the chosen side are chosen.
        let chosen = self
            .selection
            .filter(|s| self.diff.line(row, s.side).is_some())
            .and_then(|s| Some((s.columns(s.side, r)?, s.whole_lines)));
        // The side a press would choose lights up in the numbers under the pointer; while
        // choosing, the chosen one.
        let lit = self
            .pointer
            .filter(|p| rect.intersect(ui.clip_rect()).contains(*p))
            .map(|p| match self.selection.filter(|_| self.dragging) {
                Some(s) => s.side,
                None => self.press_side(line, p.x, rect.left()),
            });
        let numbers = Numbers::Both {
            old: row.old,
            new: row.new,
            lit,
        };
        let marks = Marks {
            chosen,
            query: self.query,
            current_found: self.current_found_columns(r, side),
        };
        let spans = line.and_then(|l| spans_of(self.syntax, l, side));
        let text = paint_line(ui, rect, line, spans, numbers, &self.style, &marks);
        // Its side is settled when a press starts choosing.
        DrawnColumn {
            side: Side::New,
            rect,
            numbers_w: 2.0 * self.style.numbers_w,
            text,
        }
    }

    /// Unified: the side a press at `x` chooses, on a row from `left`.
    fn press_side(&self, line: Option<&DiffLine>, x: f32, left: f32) -> Side {
        if let Some(side) = self.extend {
            return side;
        }
        let numbers_w = self.style.numbers_w;
        if x < left + 2.0 * numbers_w {
            return if x < left + numbers_w {
                Side::Old
            } else {
                Side::New
            };
        }
        match line.map(|l| l.kind) {
            Some(LineKind::Removed) => Side::Old,
            Some(LineKind::Same) if self.ctrl => Side::Old,
            _ => Side::New,
        }
    }

    /// The columns of the place gone to in row `r`'s line of a side. An unchanged line is found
    /// once, but both sides show the place if their text is the same.
    fn current_found_columns(&self, r: usize, side: Side) -> Option<Range<usize>> {
        let m = self.current_found.filter(|m| m.row == r)?;
        let same_text = || {
            let row = self.rows[r];
            match (
                self.diff.line(row, Side::Old),
                self.diff.line(row, Side::New),
            ) {
                (Some(o), Some(n)) => o.kind == LineKind::Same && o.raw == n.raw,
                _ => false,
            }
        };
        (m.side == side || same_text()).then(|| m.columns.clone())
    }
}

enum Numbers {
    Own,
    Both {
        old: Option<u32>,
        new: Option<u32>,
        lit: Option<Side>,
    },
}

struct Marks<'a> {
    /// Display columns chosen, and whether as whole lines.
    chosen: Option<(Range<usize>, bool)>,
    query: &'a str,
    current_found: Option<Range<usize>>,
}

fn spans_of<'a>(
    syntax: Option<&'a [Vec<Spans>; 2]>,
    line: &DiffLine,
    side: Side,
) -> Option<&'a Spans> {
    let [old_spans, new_spans] = syntax?;
    let version = match side {
        Side::Old => old_spans,
        Side::New => new_spans,
    };
    version.get(line.index as usize)
}

/// Paints a line into `rect`, or a filler for `None`.
fn paint_line(
    ui: &Ui,
    rect: Rect,
    line: Option<&DiffLine>,
    spans: Option<&Spans>,
    numbers: Numbers,
    style: &RowStyle,
    marks: &Marks,
) -> Option<TextAt> {
    let c = style.colors;
    let (chosen, whole_lines) = match marks.chosen.clone() {
        Some((columns, whole_lines)) => (Some(columns), whole_lines),
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
    let number = |x: f32, index: Option<u32>, lit: Option<Color32>| {
        let Some(index) = index else { return };
        let cell = Rect::from_min_size(pos2(x, rect.top()), vec2(style.numbers_w, style.row_h));
        let color = if whole_lines {
            painter.rect_filled(cell, 0.0, c.selected_bg);
            c.selected_fg
        } else if let Some(tint) = lit {
            painter.rect_filled(cell, 0.0, tint);
            c.text
        } else {
            c.weak
        };
        let galley = painter.layout_no_wrap((index + 1).to_string(), style.font.clone(), color);
        let right = x + style.numbers_w - 8.0;
        painter.galley(pos2(right - galley.size().x, y), galley, color);
    };
    let mut x = rect.left();
    match numbers {
        Numbers::Own => {
            number(x, Some(line.index), None);
            x += style.numbers_w;
        }
        Numbers::Both { old, new, lit } => {
            number(x, old, (lit == Some(Side::Old)).then_some(c.removed_word));
            x += style.numbers_w;
            number(x, new, (lit == Some(Side::New)).then_some(c.added_word));
            x += style.numbers_w;
        }
    }
    if !marker.is_empty() {
        let m = painter.layout_no_wrap(marker.into(), style.font.clone(), marker_color);
        painter.galley(pos2(x + 2.0, y), m, marker_color);
    }
    x += MARKER_WIDTH;
    let spans = spans.map_or(&[][..], Vec::as_slice);
    let job = syntax::layout_job(
        &line.text,
        &line.spans,
        spans,
        &style.font,
        ui.visuals(),
        word,
    );
    let text = TextAt {
        galley: painter.layout_job(job),
        origin: pos2(x - style.scroll_x, y),
    };
    let found = find::find([&line.raw], marks.query).into_iter().map(|m| {
        let columns =
            display_column(&line.raw, m.range.start)..display_column(&line.raw, m.range.end);
        let current = marks.current_found.as_ref() == Some(&columns);
        (columns, current)
    });
    let clip = Rect::from_min_max(pos2(x, rect.top()), rect.max).intersect(ui.clip_rect());
    let row = TextRow {
        y_range: rect.y_range(),
        row_h: style.row_h,
        colors: c,
    };
    paint_text(&painter.with_clip_rect(clip), &text, &row, found, chosen);
    Some(text)
}

fn fold_button(ui: &mut Ui, folding: Folding) -> egui::Response {
    let (glyph, tinted) = match folding {
        Folding::Folded => (glyphs::FOLD, true),
        Folding::SomeOpened => (glyphs::FOLD, false),
        Folding::WholeFile => (glyphs::UNFOLD, false),
    };
    widgets::icon_button(ui, glyph, tinted)
}

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

/// Where row `row` of the form is among the `shown` rows: its own, or the fold it is in.
fn shown_index(shown: &[Shown], row: usize) -> usize {
    shown.partition_point(|s| match s {
        Shown::Row(r) => *r < row,
        Shown::Fold(hidden) => hidden.end <= row,
    })
}

fn numbers_width(diff: &FileDiff, char_w: f32) -> f32 {
    line_numbers_width(diff.old.len().max(diff.new.len()), char_w) + 18.0
}

/// The display column of the first changed word in shown row `at`.
fn first_word_column(diff: &FileDiff, rows: &[Row], shown: &[Shown], at: usize) -> Option<usize> {
    let Some(Shown::Row(r)) = shown.get(at) else {
        return None;
    };
    [Side::Old, Side::New]
        .into_iter()
        .filter_map(|side| diff.line(rows[*r], side))
        .filter_map(|l| l.spans.first().map(|s| l.text[..s.start].chars().count()))
        .min()
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

impl ParterreApp {
    pub(super) fn diff_windows(&mut self, ctx: &egui::Context) {
        let settings = &mut self.settings;
        let mut blames = Vec::new();
        let mut fetches = Vec::new();
        for window in &mut self.diffs.windows {
            window.show(
                ctx,
                &mut settings.diff_window,
                &mut settings.syntax_colour,
                &mut settings.text_size,
                self.window_theme,
                &self.window_icon,
            );
            if let Some(blame) = window.blame_request.take() {
                blames.push((window.repo.clone(), blame));
            }
            if std::mem::take(&mut window.fetch_requested) {
                fetches.push(window.viewport_id());
            }
        }
        self.diffs.windows.retain(|w| !w.closed);
        for (repo, (spec, line)) in blames {
            self.open_blame(repo, spec, line, ctx);
        }
        for window in fetches {
            self.fetch(ctx, window);
        }
    }

    /// A script's `open diff:REV:PATH`: the commit's change to the file, against its first
    /// parent. `REV` is a ref or a hash prefix.
    pub(super) fn open_named_diff(
        &mut self,
        repo: &Arc<Repo>,
        spec: &str,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let (rev, path) = spec.split_once(':').ok_or("expected COMMIT:PATH")?;
        let ix = repo
            .resolve(rev)
            .ok_or_else(|| format!("no commit named {rev}"))?;
        let commit = repo.commit(ix);
        let git = parterre_core::git::Git::new(&repo.path);
        let files = git.changed_files(&commit.oid).map_err(|e| e.to_string())?;
        let file = files
            .iter()
            .find(|f| f.path == path)
            .ok_or_else(|| format!("{path} is not among the files {rev} changed"))?;
        let parent = commit.parents.first().map(|&p| repo.commit(p).oid);
        let spec = FileDiffSpec::of_commit(commit.oid, parent, file);
        self.diffs
            .open(repo.clone(), spec, &self.settings.diff_window, ctx);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::text_view::TOOLBAR_HEIGHT;
    use super::*;
    use parterre_core::file_diff::Version;
    use parterre_core::repo::Head;

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
        let mut w = DiffWindow::unloaded(1, repo, spec, settings);
        let options = w.options;
        let ready = Ready::new(loaded, options, &Engine::InProcess, &Cancel::new());
        w.load = Load::Ready(Box::new(ready));
        w
    }

    fn frame(
        ctx: &egui::Context,
        w: &mut DiffWindow,
        settings: &mut DiffWindowSettings,
        events: Vec<egui::Event>,
    ) {
        frame_with(ctx, w, settings, events, Modifiers::NONE);
    }

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
        // As `show` does, less the viewport; nothing renders the textures.
        w.poll();
        let mut syntax = true;
        ctx.run_ui(input, |ui| {
            w.handle_keys(ui, settings);
            w.contents(ui, settings, &mut syntax);
        })
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
        for events in click(pos2(27.0, TOOLBAR_HEIGHT / 2.0)) {
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
        w.jump_to_change = None;
        w.goto_line = Some(59);
        let ctx = egui::Context::default();
        frame(&ctx, &mut w, &mut settings, Vec::new());
        frame(&ctx, &mut w, &mut settings, Vec::new());
        assert_eq!(w.current_change, Some(1));
    }

    #[test]
    fn f7_goes_to_the_next_change_and_shift_f7_to_the_previous() {
        let old = numbered(120);
        let new = old
            .replace("line 5\n", "line five\n")
            .replace("line 50\n", "line fifty\n")
            .replace("line 100\n", "line hundred\n");
        let mut settings = DiffWindowSettings::default();
        let mut w = window(&old, &new, &settings);
        let ctx = egui::Context::default();
        frame(&ctx, &mut w, &mut settings, Vec::new());
        frame(&ctx, &mut w, &mut settings, Vec::new());
        assert_eq!(w.current_change, Some(0));
        let steps = [
            (Modifiers::NONE, Some(1)),
            (Modifiers::NONE, Some(2)),
            (Modifiers::SHIFT, Some(1)),
            (Modifiers::SHIFT, Some(0)),
        ];
        for (modifiers, expected) in steps {
            let f7 = egui::Event::Key {
                key: Key::F7,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            };
            // As `show` does: the keys first.
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))),
                events: vec![egui::Event::ModifiersChanged(modifiers), f7],
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                w.handle_keys(ui, &mut settings);
                w.contents(ui, &mut settings, &mut true);
            });
            frame(&ctx, &mut w, &mut settings, Vec::new());
            assert_eq!(w.current_change, expected, "after F7 with {modifiers:?}");
        }
    }

    /// Where display column `col` of row `row` starts in the new (right) pane, side by side and
    /// unfolded, with no notes in the header.
    fn at(ctx: &egui::Context, w: &DiffWindow, row: usize, col: usize) -> egui::Pos2 {
        let TextMetrics { row_h, char_w } = TextMetrics::new(ctx);
        let numbers_w = numbers_width(&w.ready().unwrap().diff, char_w);
        let mid = (1200.0 - OVERVIEW_WIDTH) / 2.0;
        let top = TOOLBAR_HEIGHT + HEADER_HEIGHT + PANE_TITLE_HEIGHT;
        pos2(
            mid + numbers_w + MARKER_WIDTH + col as f32 * char_w + 1.0,
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
        let (a, b) = (
            a - vec2(MARKER_WIDTH + 12.0, 0.0),
            b - vec2(MARKER_WIDTH + 12.0, 0.0),
        );
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

        // Also when each click comes in a single frame. A second later, not to count as a
        // triple click.
        for _ in 0..60 {
            frame(&ctx, &mut w, &mut settings, Vec::new());
        }
        let p = at(&ctx, &w, 1, 10);
        for _ in 0..2 {
            let clicked = vec![
                egui::Event::PointerMoved(p),
                button(p, true, none),
                button(p, false, none),
            ];
            frame(&ctx, &mut w, &mut settings, clicked);
        }
        assert_eq!(w.selected_text().as_deref(), Some("three"));

        w.select_all();
        assert_eq!(
            w.selected_text().as_deref(),
            Some("one\n\ttwo three\nfour five\nsix\n")
        );
    }

    /// Where display column `col` of row `row` starts in the unified form, unfolded.
    fn at_unified(ctx: &egui::Context, w: &DiffWindow, row: usize, col: usize) -> egui::Pos2 {
        let right_pane = at(ctx, w, row, col);
        let char_w = TextMetrics::new(ctx).char_w;
        let numbers_w = numbers_width(&w.ready().unwrap().diff, char_w);
        let mid = (1200.0 - OVERVIEW_WIDTH) / 2.0;
        // No pane titles, two number columns, from the left edge.
        pos2(
            right_pane.x - mid + numbers_w,
            right_pane.y - PANE_TITLE_HEIGHT,
        )
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
        let old_numbers = |row| at_unified(&ctx, &w, row, 0) - vec2(MARKER_WIDTH + 40.0, 0.0);
        let (a, b) = (old_numbers(0), old_numbers(3));
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
        w.opened_folds.push(lines);
        w.shown_dirty = true;
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
        assert!(w.fold && w.opened_folds.is_empty());
        assert_eq!(folds(&w), 2);

        // Then it turns folding off.
        w.toggle_fold(&mut settings);
        frame(&ctx, &mut w, &mut settings, Vec::new());
        assert!(!w.fold && !settings.fold);
        assert_eq!(folds(&w), 0);
    }

    fn key_with(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn key(key: Key) -> egui::Event {
        key_with(key, Modifiers::NONE)
    }

    fn ctrl(key: Key) -> egui::Event {
        key_with(key, Modifiers::COMMAND)
    }

    fn typed(text: &str) -> egui::Event {
        egui::Event::Text(text.into())
    }

    /// The places found, as row, version and the text found there.
    fn found(w: &DiffWindow) -> Vec<(usize, char, String)> {
        let ready = w.ready().unwrap();
        let (rows, _) = DiffWindow::rows_and_changes(&ready.diff, w.form);
        w.find
            .matches
            .iter()
            .map(|m| {
                let line = ready.diff.line(rows[m.row], m.side).unwrap();
                let text = line.text.chars().collect::<Vec<_>>()[m.columns.clone()]
                    .iter()
                    .collect();
                let sign = match m.side {
                    Side::Old => '-',
                    Side::New => '+',
                };
                (m.row, sign, text)
            })
            .collect()
    }

    fn has_find_focus(ctx: &egui::Context, w: &DiffWindow) -> bool {
        ctx.memory(|m| m.has_focus(w.find_id()))
    }

    #[test]
    fn ctrl_f_finds_as_typed_and_f3_and_enter_go_round_the_places() {
        let mut settings = DiffWindowSettings {
            fold: false,
            words: WordMode::Position,
            ..DiffWindowSettings::default()
        };
        let mut w = window("one\ntwo\nfour\n", "one\nTWO\nfour\n", &settings);
        let ctx = egui::Context::default();
        frame(&ctx, &mut w, &mut settings, Vec::new());
        frame(&ctx, &mut w, &mut settings, vec![ctrl(Key::F)]);
        assert!(has_find_focus(&ctx, &w));
        // Any case; an unchanged line once, the changed row on both sides.
        frame(&ctx, &mut w, &mut settings, vec![typed("O")]);
        assert_eq!(w.find.query, "O");
        assert_eq!(
            found(&w),
            [
                (0, '+', "o".into()),
                (1, '-', "o".into()),
                (1, '+', "O".into()),
                (2, '+', "o".into()),
            ]
        );
        assert_eq!(w.find.current, Some(0));
        // Enter and F3 go on, round the end; Shift goes back.
        frame(&ctx, &mut w, &mut settings, vec![key(Key::Enter)]);
        assert_eq!(w.find.current, Some(1));
        assert!(has_find_focus(&ctx, &w));
        for _ in 0..3 {
            frame(
                &ctx,
                &mut w,
                &mut settings,
                vec![crate::keys::find_next().event()],
            );
        }
        assert_eq!(w.find.current, Some(0));
        let shift = Modifiers::SHIFT;
        frame_with(
            &ctx,
            &mut w,
            &mut settings,
            vec![crate::keys::find_previous().event()],
            shift,
        );
        assert_eq!(w.find.current, Some(3));
        // F3 works outside the field too.
        ctx.memory_mut(|m| m.surrender_focus(w.find_id()));
        frame(
            &ctx,
            &mut w,
            &mut settings,
            vec![crate::keys::find_next().event()],
        );
        assert_eq!(w.find.current, Some(0));

        // The other form finds in its own rows.
        w.set_form(DiffForm::Unified);
        frame(&ctx, &mut w, &mut settings, Vec::new());
        assert_eq!(
            found(&w),
            [
                (0, '+', "o".into()),
                (1, '-', "o".into()),
                (2, '+', "O".into()),
                (3, '+', "o".into()),
            ]
        );
    }

    #[test]
    fn going_to_a_place_in_a_fold_opens_it_around_the_place_and_scrolls_to_it() {
        let old = numbered(300).replace("line 150\n", "a needle\n");
        let new = old
            .replace("line 5\n", "line five\n")
            .replace("line 290\n", "line two hundred and ninety\n");
        let mut settings = DiffWindowSettings::default();
        let mut w = window(&old, &new, &settings);
        let ctx = egui::Context::default();
        frame(&ctx, &mut w, &mut settings, Vec::new());
        let side = &w.ready().unwrap().diff.side;
        let row = side.iter().position(|r| r.new == Some(149)).unwrap();
        assert!(!w.shown.contains(&Shown::Row(row)), "folded at first");
        frame(&ctx, &mut w, &mut settings, vec![ctrl(Key::F)]);
        frame(&ctx, &mut w, &mut settings, vec![typed("NEEDLE")]);
        assert_eq!(found(&w), [(row, '+', "needle".into())]);
        assert_eq!(w.find.current, Some(0));
        // Opened with the context a change keeps, still folded on either side.
        let at = w.shown.iter().position(|s| *s == Shown::Row(row)).unwrap();
        let context = parterre_core::file_diff::CONTEXT_LINES;
        assert_eq!(w.shown[at - context - 1], Shown::Fold(8..row - context));
        assert!(matches!(w.shown[at + context + 1], Shown::Fold(_)));

        // Unfolded, the rows don't fit: going to it again scrolls it into view.
        w.toggle_fold(&mut settings);
        w.toggle_fold(&mut settings);
        frame(&ctx, &mut w, &mut settings, Vec::new());
        frame(&ctx, &mut w, &mut settings, Vec::new());
        assert!(w.top < 10, "at the first change, top {}", w.top);
        frame(
            &ctx,
            &mut w,
            &mut settings,
            vec![crate::keys::find_next().event()],
        );
        for _ in 0..2 {
            frame(&ctx, &mut w, &mut settings, Vec::new());
        }
        assert!(
            w.top <= row && row < w.top + 20,
            "row {row} in view, top {}",
            w.top
        );
    }

    #[test]
    fn esc_in_the_find_field_leaves_it_and_only_then_closes_the_window() {
        let (ctx, mut w, mut settings) = text_window();
        frame(&ctx, &mut w, &mut settings, vec![ctrl(Key::F)]);
        frame(&ctx, &mut w, &mut settings, vec![typed("f")]);
        assert_eq!(w.find.matches.len(), 2);
        frame(&ctx, &mut w, &mut settings, vec![key(Key::Escape)]);
        assert!(!w.closed);
        assert!(!has_find_focus(&ctx, &w));
        assert_eq!(w.find.query, "");
        assert!(w.find.matches.is_empty());
        frame(
            &ctx,
            &mut w,
            &mut settings,
            vec![crate::keys::close_window().event()],
        );
        assert!(w.closed);
    }

    #[test]
    fn ctrl_f_offers_the_chosen_characters_or_the_last_query_and_ctrl_c_stays_in_the_field() {
        let (ctx, mut w, mut settings) = text_window();
        // Characters chosen in one line: they are the query.
        let (a, b) = (at(&ctx, &w, 2, 1), at(&ctx, &w, 2, 4));
        drag(&ctx, &mut w, &mut settings, a, b);
        frame(&ctx, &mut w, &mut settings, vec![ctrl(Key::F)]);
        assert_eq!(w.find.query, "our");
        assert_eq!(found(&w), [(2, '+', "our".into())]);
        frame(&ctx, &mut w, &mut settings, vec![key(Key::Escape)]);

        // Otherwise the last query, selected: a paste replaces it, less its line break.
        let (a, b) = (at(&ctx, &w, 0, 1), at(&ctx, &w, 1, 1));
        drag(&ctx, &mut w, &mut settings, a, b);
        frame(&ctx, &mut w, &mut settings, vec![ctrl(Key::F)]);
        assert_eq!(w.find.query, "our");
        let paste = egui::Event::Paste("three\n".into());
        frame(&ctx, &mut w, &mut settings, vec![paste]);
        assert_eq!(w.find.query, "three");
        // Ctrl+A and Ctrl+C in the field are the field's.
        let chosen = w.selection;
        frame(&ctx, &mut w, &mut settings, vec![ctrl(Key::A)]);
        assert_eq!(w.selection, chosen);
        let copied = std::cell::Cell::new(None);
        let input = egui::RawInput {
            events: vec![egui::Event::Copy],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| copied.set(Some(w.copied(ui))));
        assert_eq!(copied.take(), Some(None));
    }
}
