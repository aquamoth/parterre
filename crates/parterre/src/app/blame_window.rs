//! Blame windows: a file with the commit that last changed each of its lines, in a window of
//! its own (an immediate viewport, like the diff windows). Several can be open at once; they
//! close with the repository. The model is [`parterre_core::blame`].
//!
//! A gutter on the left names each run of lines from one commit (hash, author, date), shaded by
//! the commit's age among the file's commits as TortoiseGitBlame shades lines. Clicking a line
//! chooses it and its commit, whose lines are all highlighted; choosing more lines (drag,
//! Shift+click) keeps the commit of the first, and the chosen commit stays chosen until another
//! is. An overview strip on the right marks the chosen commit's lines in the whole file and
//! shows the part in view; the text never scrolls by itself when a commit is chosen, the strip
//! scrolls it. The bar at the bottom describes the commit under the pointer or chosen. A line's
//! menu goes on from there, as TortoiseGitBlame's does: blame the version before its commit (in
//! a window of its own, the line chosen at its place there), show its commit's change to the
//! file in a diff window, or show the log from its commit. The toolbar says whether whitespace
//! changes and moved lines count, remembered for the next window.
//!
//! Deliberate deviations from TortoiseGitBlame (see #104): no log pane of the file's
//! history beside the text (the log window shows history), and only whole lines are chosen
//! for copying.

use std::sync::{Arc, mpsc};

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{
    self, Color32, FontId, Key, Modifiers, Rect, RichText, ScrollArea, Sense, Stroke, Ui,
    UiBuilder, Vec2, pos2, vec2,
};
use parterre_core::blame::{Blame, BlameOptions, BlameSpec, Moves, Origin};
use parterre_core::file_diff::{FileDiffSpec, Rev};
use parterre_core::glyphs;
use parterre_core::text::thousands;
use parterre_core::{Oid, Repo};

use super::ParterreApp;
use super::diff_window::{
    Colors, OVERVIEW, SCROLLBAR, colors, hscrollbar, message, overview_background, overview_scale,
    overview_scroll, overview_view,
};
use super::log_window::cell;
use crate::settings::BlameWindowSettings;
use crate::text_size;
use crate::widgets;

/// Height of the toolbar.
const TOOLBAR: f32 = 44.0;
/// Height of the header with the file's path.
const HEADER: f32 = 34.0;
/// Height of the bar describing a commit at the bottom.
const INFO: f32 = 26.0;
const FONT_SIZE: f32 = 13.0;
/// Width of the author column in the gutter.
const AUTHOR: f32 = 130.0;
/// Width of the date column in the gutter.
const DATE: f32 = 84.0;
/// Padding inside the gutter's columns.
const PAD: f32 = 8.0;

/// What a blame window asks the app for.
#[derive(Debug)]
pub enum BlameRequest {
    /// A blame window for the file at another revision, with a line (from 0) chosen.
    Blame(Arc<Repo>, BlameSpec, usize),
    /// A diff window: a line's commit's change to the file, at the line (from 0, in the new
    /// version).
    Diff(Arc<Repo>, FileDiffSpec, usize),
    /// The log window, from a line's commit.
    Log(Oid),
}

/// The open blame windows.
#[derive(Debug, Default)]
pub struct BlameWindows {
    windows: Vec<BlameWindow>,
    /// How many were opened, to give each its own viewport id.
    opened: u64,
    requests: Vec<BlameRequest>,
}

impl BlameWindows {
    /// Opens a blame window for `spec`, or brings the one already showing it to the front
    /// (reloaded, if it reads the working tree). `line` (from 0) is chosen and scrolled to.
    pub fn open(
        &mut self,
        repo: Arc<Repo>,
        spec: BlameSpec,
        line: Option<usize>,
        settings: &BlameWindowSettings,
        ctx: &egui::Context,
    ) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.spec == spec) {
            w.repo = repo;
            w.reload(ctx);
            if line.is_some() {
                w.pending_line = line;
            }
            w.focus = true;
            return;
        }
        self.opened += 1;
        let mut w = BlameWindow::new(self.opened, repo, spec, settings);
        w.pending_line = line;
        w.load(ctx);
        self.windows.push(w);
    }

    /// Closes every blame window (the repository they belong to is closing).
    pub fn close_all(&mut self) {
        self.windows.clear();
        self.requests.clear();
    }

    /// True while git is still blaming for any window.
    pub fn is_loading(&self) -> bool {
        self.windows
            .iter()
            .any(|w| matches!(w.load, Load::Loading(_)))
    }

    /// What the windows asked for since the last call.
    pub fn take_requests(&mut self) -> Vec<BlameRequest> {
        std::mem::take(&mut self.requests)
    }
}

/// A blame and what the window shows for each origin.
#[derive(Debug)]
struct Ready {
    blame: Blame,
    /// Per origin: its age among the file's commits, 0 (oldest) to 1 (newest).
    ages: Vec<f32>,
    /// Per origin: the author date as the log shows it (local time), or in the author's zone
    /// for a commit the snapshot doesn't have; empty for uncommitted lines.
    dates: Vec<String>,
    /// Per origin: the commit is in the snapshot, so the log can show it.
    in_repo: Vec<bool>,
    commits: usize,
}

impl Ready {
    fn new(blame: Blame, repo: &Repo) -> Ready {
        let ages = blame.ages();
        let (dates, in_repo) = blame
            .origins
            .iter()
            .map(|o| match o.commit.and_then(|oid| repo.lookup(&oid)) {
                Some(ix) => (repo.commit(ix).author_date.clone(), true),
                None if o.commit.is_some() => (o.author_date(), false),
                None => (String::new(), false),
            })
            .unzip();
        Ready {
            commits: blame.commit_count(),
            blame,
            ages,
            dates,
            in_repo,
        }
    }
}

#[derive(Debug)]
enum Load {
    /// git runs on a worker thread.
    Loading(mpsc::Receiver<Result<Ready, String>>),
    Ready(Box<Ready>),
    Failed(String),
}

/// What a line's menu asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineAction {
    BlamePrevious(usize),
    ShowChanges(usize),
    ShowLog(usize),
    CopyHash(usize),
    CopyLines,
}

#[derive(Debug)]
struct BlameWindow {
    id: u64,
    repo: Arc<Repo>,
    spec: BlameSpec,
    /// The size the window opened with (the viewport builder must not change while it is open).
    size: Vec2,
    load: Load,
    /// The options chosen in the toolbar, and those of the last load started.
    options: BlameOptions,
    requested: BlameOptions,
    /// Lines chosen, from and to (indices into the lines, either way round).
    selection: Option<(usize, usize)>,
    /// The chosen commit, whose lines are highlighted (`None` inside is uncommitted). Set with
    /// the line a choice of lines starts at, or on its own ([`BlameWindow::choose_commit`]).
    chosen: Option<Option<Oid>>,
    /// The mouse is choosing lines.
    dragging: bool,
    /// Choose this line and scroll to it once loaded.
    pending_line: Option<usize>,
    /// Scroll to this offset once loaded (a reload).
    pending_scroll: Option<f32>,
    /// Scroll to this offset in the next frame.
    scroll_to: Option<f32>,
    /// The scroll offset in the last frame.
    scroll: f32,
    /// Sideways scroll of the text, in points.
    hoff: f32,
    /// Bring the window to the front in the next frame.
    focus: bool,
    closed: bool,
    /// The theme last given to the window's title bar.
    title_theme: Option<egui::SystemTheme>,
}

impl BlameWindow {
    /// A window for `spec`, not loading yet ([`BlameWindow::load`]).
    fn new(
        id: u64,
        repo: Arc<Repo>,
        spec: BlameSpec,
        settings: &BlameWindowSettings,
    ) -> BlameWindow {
        let [w, h] = settings.size;
        let options = BlameOptions {
            ignore_whitespace: settings.ignore_whitespace,
            moves: settings.moves,
        };
        BlameWindow {
            id,
            repo,
            spec,
            size: vec2(w, h),
            load: Load::Failed(String::new()),
            options,
            requested: options,
            selection: None,
            chosen: None,
            dragging: false,
            pending_line: None,
            pending_scroll: None,
            scroll_to: None,
            scroll: 0.0,
            hoff: 0.0,
            focus: false,
            closed: false,
            title_theme: None,
        }
    }

    /// Blames `spec` with the current options on a worker thread.
    fn load(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let git = parterre_core::git::Git::new(&self.repo.path);
        let (spec, options, repo) = (self.spec.clone(), self.options, self.repo.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = git
                .blame(&spec, options)
                .map(|blame| Ready::new(blame, &repo))
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.requested = options;
        self.load = Load::Loading(rx);
        self.dragging = false;
    }

    /// Takes the worker's result when it is there, and blames again if the options changed.
    fn poll(&mut self, ctx: &egui::Context) {
        if let Load::Loading(rx) = &self.load
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(ready) => self.loaded(ready),
                Err(e) => self.load = Load::Failed(e),
            }
        }
        if self.options != self.requested {
            // Keep the place: the lines stay, only their commits may change.
            self.pending_scroll = Some(self.scroll);
            self.load(ctx);
        }
    }

    /// Shows a blame, keeping the chosen commit if it still owns lines (a reload), and the
    /// chosen lines if the file still has them.
    fn loaded(&mut self, ready: Ready) {
        let blame = &ready.blame;
        if let Some(commit) = self.chosen
            && !blame.origins.iter().any(|o| o.commit == commit)
        {
            self.chosen = None;
        }
        if self
            .selection
            .is_some_and(|(a, b)| a.max(b) >= blame.lines.len())
        {
            self.selection = None;
        }
        self.load = Load::Ready(Box::new(ready));
    }

    /// F5: blames the working tree again, keeping the place. A blame at a commit can't change.
    fn reload(&mut self, ctx: &egui::Context) {
        if self.spec.reads_working_tree() {
            self.pending_scroll = Some(self.scroll);
            self.load(ctx);
        }
    }

    fn ready(&self) -> Option<&Ready> {
        match &self.load {
            Load::Ready(r) => Some(r),
            _ => None,
        }
    }

    fn viewport_id(&self) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(("blame", self.id))
    }

    /// `<short hash>` or "working tree".
    fn rev_name(&self, rev: Rev) -> String {
        match rev {
            Rev::Commit(oid) => oid.short(self.repo.abbrev_len),
            Rev::WorkingTree => "working tree".to_owned(),
        }
    }

    /// `<file> (<short hash>) – <repo> – Blame`
    fn title(&self) -> String {
        let path = &self.spec.path;
        let file = path.rsplit('/').next().unwrap_or(path);
        let rev = self.rev_name(self.spec.rev);
        format!("{file} ({rev}) – {} – Blame", self.repo.display_name())
    }

    /// The origin of line `i`.
    fn origin(&self, i: usize) -> Option<&Origin> {
        let blame = &self.ready()?.blame;
        blame.origins.get(blame.lines.get(i)?.origin)
    }

    /// Chooses line `i` and its commit.
    fn choose_line(&mut self, i: usize) {
        self.selection = Some((i, i));
        if let Some(origin) = self.origin(i) {
            self.chosen = Some(origin.commit);
        }
    }

    /// Chooses `commit` (`None` for the uncommitted lines) and its first line, or no line if it
    /// owns none in this version. The text stays where it is. (For the history pane, #113.)
    #[cfg_attr(not(test), allow(dead_code))]
    fn choose_commit(&mut self, commit: Option<Oid>) {
        self.chosen = Some(commit);
        self.selection = self.ready().and_then(|r| {
            let first = r
                .blame
                .lines
                .iter()
                .position(|l| r.blame.origins[l.origin].commit == commit)?;
            Some((first, first))
        });
    }

    /// The chosen lines as in the file, each ending with a newline.
    fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection?;
        let lines = &self.ready()?.blame.lines;
        let (a, b) = (a.min(b), a.max(b).min(lines.len().checked_sub(1)?));
        let mut text = String::new();
        for line in &lines[a..=b] {
            text.push_str(&line.raw);
            text.push('\n');
        }
        Some(text)
    }

    fn select_all(&mut self) {
        if let Some(n) = self.ready().map(|r| r.blame.lines.len())
            && n > 0
        {
            self.selection = Some((0, n - 1));
        }
    }

    /// Asks for a blame of the version before line `i`'s commit, in a window of its own, with
    /// the line's place in that version chosen, near where it came in. This window stays as it
    /// is.
    fn blame_previous(&self, i: usize, requests: &mut Vec<BlameRequest>) {
        let Some(spec) = self.origin(i).and_then(Origin::previous_blame) else {
            return;
        };
        if let Some(line) = self.ready().map(|r| r.blame.lines[i].orig_line as usize) {
            requests.push(BlameRequest::Blame(self.repo.clone(), spec, line));
        }
    }

    /// Esc closes; Ctrl+A chooses every line.
    fn handle_keys(&mut self, ui: &Ui) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
            self.select_all();
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            self.closed = true;
        }
    }

    fn contents(&mut self, ui: &mut Ui, settings: &mut BlameWindowSettings) -> Vec<BlameRequest> {
        let c = colors(ui);
        self.toolbar(ui, settings);
        self.header(ui, &c);
        let rest = ui.available_rect_before_wrap();
        let body = Rect::from_min_max(rest.min, pos2(rest.right(), rest.bottom() - INFO));
        let info = Rect::from_min_max(pos2(rest.left(), body.bottom()), rest.max);
        ui.painter().rect_filled(body, 0.0, c.pane);
        let mut requests = Vec::new();
        let mut hovered = None;
        match &self.load {
            Load::Loading(_) => message(ui, body, "Blaming…", ui.visuals().weak_text_color()),
            Load::Failed(e) => message(
                ui,
                body,
                &format!("Could not blame the file: {e}"),
                c.removed,
            ),
            Load::Ready(_) => {
                let mut child = ui.new_child(UiBuilder::new().max_rect(body));
                child.set_clip_rect(body.intersect(ui.clip_rect()));
                hovered = self.body(&mut child, body, &c, &mut requests);
            }
        }
        self.info_bar(ui, info, hovered, &c);
        ui.allocate_rect(rest, Sense::hover());
        requests
    }

    /// Whether whitespace changes and moved lines count.
    fn toolbar(&mut self, ui: &mut Ui, settings: &mut BlameWindowSettings) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), TOOLBAR), Sense::hover());
        ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
        let mut bar = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(10.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let ui = &mut bar;
        ui.spacing_mut().item_spacing.x = 4.0;
        let weak = ui.visuals().weak_text_color();

        let spaces = [
            (false, glyphs::WHITESPACE_COMPARE),
            (true, glyphs::WHITESPACE_IGNORE_ALL),
        ];
        let picked = widgets::segmented(
            ui,
            self.options.ignore_whitespace,
            &spaces,
            |ignore, r| {
                let (title, body) = if ignore {
                    (
                        "Ignore whitespace",
                        "A line whose blanks alone changed keeps the commit before. As git blame -w.",
                    )
                } else {
                    (
                        "Whitespace counts",
                        "A line whose blanks changed belongs to the commit that changed them.",
                    )
                };
                widgets::tip_explained(r, title, "", body)
            },
        );
        if let Some(ignore) = picked {
            self.options.ignore_whitespace = ignore;
            settings.ignore_whitespace = ignore;
        }
        ui.add_space(14.0);

        let label = ui.label(RichText::new("Moved lines").size(12.5).color(weak));
        let mut moves = self.options.moves;
        let items = Moves::ALL.map(|m| (m, m.label()));
        widgets::text_segmented(ui, &mut moves, &items);
        let explained = Moves::ALL
            .map(|m| format!("{}: {}", m.label(), m.description()))
            .join("\n\n");
        widgets::tip_explained(label, "Moved and copied lines", "", &explained);
        if moves != self.options.moves {
            self.options.moves = moves;
            settings.moves = moves;
        }
        if self.spec.reads_working_tree() {
            ui.add_space(14.0);
            ui.label(RichText::new("F5 blames again").size(12.0).color(weak));
        }
    }

    /// The path and the revision; the counts on the right, and a note on bytes that aren't
    /// UTF-8.
    fn header(&self, ui: &mut Ui, c: &Colors) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, ui.visuals().panel_fill);
        painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, c.line));
        let (weak, text) = (c.weak, c.text);
        let small = FontId::proportional(12.5);
        let mut job = LayoutJob::default();
        job.append(
            &self.spec.path,
            0.0,
            TextFormat::simple(FontId::proportional(14.5), text),
        );
        match self.spec.rev {
            Rev::Commit(oid) => {
                job.append("at", 10.0, TextFormat::simple(small.clone(), weak));
                job.append(
                    &oid.short(self.repo.abbrev_len),
                    6.0,
                    TextFormat::simple(FontId::monospace(12.5), weak),
                );
                if let Some(ix) = self.repo.lookup(&oid) {
                    let subject = &self.repo.commit(ix).subject;
                    job.append(subject, 8.0, TextFormat::simple(small.clone(), weak));
                }
            }
            Rev::WorkingTree => {
                job.append(
                    "in the working tree",
                    10.0,
                    TextFormat::simple(small.clone(), weak),
                );
            }
        }
        let mut counts = String::new();
        if let Some(r) = self.ready() {
            let (lines, commits) = (r.blame.lines.len(), r.commits);
            counts = format!(
                "{} line{} from {} commit{}",
                thousands(lines),
                if lines == 1 { "" } else { "s" },
                thousands(commits),
                if commits == 1 { "" } else { "s" },
            );
            if r.blame.invalid_bytes > 0 {
                counts = format!(
                    "{} bytes not UTF-8, shown as \\xNN   ·   {counts}",
                    thousands(r.blame.invalid_bytes)
                );
            }
        }
        let counts = painter.layout_no_wrap(counts, small, weak);
        let y = rect.center().y;
        let right = rect.right() - 12.0 - counts.size().x;
        job.wrap.max_width = (right - rect.left() - 36.0).max(40.0);
        job.wrap.max_rows = 1;
        let g = painter.layout_job(job);
        painter.galley(pos2(rect.left() + 12.0, y - g.size().y / 2.0), g, text);
        painter.galley(pos2(right, y - counts.size().y / 2.0), counts, weak);
    }

    /// The lines with their gutter, the overview strip and the horizontal scrollbar. Returns
    /// the line under the pointer.
    fn body(
        &mut self,
        ui: &mut Ui,
        full: Rect,
        c: &Colors,
        requests: &mut Vec<BlameRequest>,
    ) -> Option<usize> {
        let Load::Ready(ready) = &self.load else {
            return None;
        };
        let blame = &ready.blame;
        let n = blame.lines.len();
        if n == 0 {
            message(ui, full, "The file is empty.", c.weak);
            return None;
        }
        let bc = blame_colors(ui);
        let font = FontId::monospace(FONT_SIZE);
        let hash_font = FontId::monospace(12.0);
        let small = FontId::proportional(12.5);
        let row_h = ui.fonts_mut(|f| f.row_height(&font)).ceil() + 3.0;
        let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
        let hash_w = ui.fonts_mut(|f| f.glyph_width(&hash_font, '0')) * self.repo.abbrev_len as f32
            + 2.0 * PAD;
        let gutter = hash_w + AUTHOR + DATE;
        let digits = (n as f32).log10().floor() + 1.0;
        let numbers = digits * char_w + 2.0 * PAD;
        let text_x = full.left() + gutter + numbers + PAD;

        let right = full.right() - OVERVIEW;
        let text_w = right - text_x;
        let content_w = blame.widest as f32 * char_w + 24.0;
        let hmax = (content_w - text_w).max(0.0);
        let bottom = full.bottom() - if hmax > 0.0 { SCROLLBAR } else { 0.0 };
        let area = Rect::from_min_max(full.min, pos2(right, bottom));

        // A line asked for: chosen, and a third of the way down.
        if let Some(line) = self.pending_line.take() {
            let line = line.min(n - 1);
            self.selection = Some((line, line));
            self.chosen = Some(blame.origins[blame.lines[line].origin].commit);
            let above = (area.height() / row_h / 3.0).floor();
            self.scroll_to = Some(((line as f32 - above) * row_h).max(0.0));
        } else if let Some(offset) = self.pending_scroll.take() {
            self.scroll_to = Some(offset);
        }
        let mut scroll = ScrollArea::vertical()
            .auto_shrink(false)
            .id_salt(("blame", self.id));
        if let Some(offset) = self.scroll_to.take() {
            scroll = scroll.vertical_scroll_offset(offset.max(0.0));
        }
        if ui.rect_contains_pointer(area) {
            let dx = ui.input(|i| i.smooth_scroll_delta.x);
            self.hoff -= dx;
        }
        self.hoff = self.hoff.clamp(0.0, hmax);

        let selection = self.selection.map(|(a, b)| (a.min(b), a.max(b)));
        let highlighted = self.chosen;
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let pressed = ui.input(|i| i.pointer.primary_pressed());
        let (hoff, abbrev) = (self.hoff, self.repo.abbrev_len);
        let mut hovered = None;
        let mut press = None;
        let mut secondary = None;
        let mut action = None;
        let (mut first, mut last) = (None, None);
        let mut child = ui.new_child(UiBuilder::new().max_rect(area));
        child.set_clip_rect(area.intersect(ui.clip_rect()));
        child.spacing_mut().item_spacing = Vec2::ZERO;
        let out = scroll.show_rows(&mut child, row_h, n, |ui, range| {
            let top = range.start;
            for i in range {
                first.get_or_insert(i);
                last = Some(i);
                let (rect, response) = ui.allocate_exact_size(
                    vec2(ui.available_width(), row_h),
                    Sense::click_and_drag(),
                );
                let line = &blame.lines[i];
                let origin = &blame.origins[line.origin];
                let painter = ui.painter();
                let gutter_rect =
                    Rect::from_x_y_ranges(rect.left()..=rect.left() + gutter, rect.y_range());
                let text_rect =
                    Rect::from_x_y_ranges(rect.left() + gutter..=rect.right(), rect.y_range());
                painter.rect_filled(gutter_rect, 0.0, bc.age(ready.ages[line.origin]));
                let chosen = selection.is_some_and(|(a, b)| (a..=b).contains(&i));
                if chosen {
                    painter.rect_filled(text_rect, 0.0, c.selection);
                } else if highlighted == Some(origin.commit) {
                    painter.rect_filled(text_rect, 0.0, bc.commit);
                }
                let run_start = blame.starts_run(i);
                if run_start && i > 0 {
                    painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, c.line));
                }
                let y = rect.center().y;
                let put = |g: Arc<egui::Galley>, x: f32| {
                    painter.galley(pos2(x, y - g.size().y / 2.0), g, c.text);
                };
                // The first row in view names its commit too, when its run started above.
                if run_start || i == top {
                    let mut x = rect.left() + PAD;
                    match origin.commit {
                        Some(oid) => {
                            put(
                                cell(ui, &oid.short(abbrev), hash_font.clone(), c.weak, hash_w),
                                x,
                            );
                            x += hash_w - PAD;
                            put(
                                cell(ui, &origin.author, small.clone(), c.text, AUTHOR - PAD),
                                x,
                            );
                            x += AUTHOR;
                            let date = ready.dates[line.origin].split(' ').next().unwrap_or("");
                            put(cell(ui, date, small.clone(), c.weak, DATE - PAD), x);
                        }
                        None => {
                            let text = "Not committed yet";
                            put(cell(ui, text, small.clone(), c.note, gutter - 2.0 * PAD), x);
                        }
                    }
                }
                painter.vline(
                    gutter_rect.right() - 0.5,
                    rect.y_range(),
                    Stroke::new(1.0, c.line),
                );
                let no = painter.layout_no_wrap((i + 1).to_string(), font.clone(), c.weak);
                painter.galley(
                    pos2(
                        gutter_rect.right() + numbers - PAD - no.size().x,
                        y - no.size().y / 2.0,
                    ),
                    no,
                    c.weak,
                );
                let clip = Rect::from_x_y_ranges(text_x..=rect.right(), rect.y_range())
                    .intersect(ui.clip_rect());
                let g = painter.layout_no_wrap(line.text.clone(), font.clone(), c.text);
                painter.with_clip_rect(clip).galley(
                    pos2(text_x - hoff, y - g.size().y / 2.0),
                    g,
                    c.text,
                );

                if pointer.is_some_and(|p| rect.y_range().contains(p.y)) {
                    hovered = Some(i);
                }
                if pressed && response.hovered() {
                    press = Some(i);
                }
                if response.secondary_clicked() {
                    secondary = Some(i);
                }
                let over_gutter = response
                    .hover_pos()
                    .is_some_and(|p| gutter_rect.contains(p));
                let response = if over_gutter {
                    let date = &ready.dates[line.origin];
                    response.on_hover_ui(|ui| origin_tip(ui, origin, date, abbrev))
                } else {
                    response
                };
                egui::Popup::context_menu(&response)
                    .style(crate::menu::style)
                    .show(|ui| {
                        crate::menu::fit_window(ui, |ui| {
                            ui.set_min_width(crate::menu::MIN_WIDTH);
                            let chosen =
                                selection.is_some_and(|(a, b)| a != b && (a..=b).contains(&i));
                            let in_repo = ready.in_repo[line.origin];
                            if let Some(a) = line_menu(ui, i, origin, in_repo, chosen) {
                                action = Some(a);
                            }
                        });
                    });
            }
        });
        self.scroll = out.state.offset.y;

        // Every line of the chosen commit in the whole file, and the part in view. Click or
        // drag to put that place in the middle.
        let strip = Rect::from_min_max(pos2(right, full.top()), full.max);
        let view = (
            out.state.offset.y,
            out.inner_rect.height(),
            out.content_size.y,
        );
        overview_background(ui, strip, c);
        if let Some(commit) = highlighted {
            let scale = overview_scale(strip, n, row_h);
            let owned = |i: usize| blame.origins[blame.lines[i].origin].commit == commit;
            for run in runs(n, owned) {
                let (y0, y1) = (run.start as f32 * scale, run.end as f32 * scale);
                let mark = Rect::from_min_max(
                    pos2(strip.left() + 3.0, strip.top() + y0),
                    pos2(strip.right() - 2.0, strip.top() + y1.max(y0 + 2.0)),
                );
                ui.painter().rect_filled(mark, 0.0, bc.mark);
            }
        }
        overview_view(ui, strip, view, c);
        let id = egui::Id::new(("blame-overview", self.id));
        if let Some(offset) = overview_scroll(ui, id, strip, n, row_h, view) {
            self.scroll_to = Some(offset);
        }

        // Choosing lines: a press chooses a line and its commit (Shift extends, keeping the
        // commit), a drag extends, scrolling past the edges; a right-click outside the chosen
        // lines chooses its line and commit.
        let (down, shift) = ui.input(|i| (i.pointer.primary_down(), i.modifiers.shift));
        if let Some(i) = press {
            match self.selection {
                Some((a, _)) if shift => self.selection = Some((a, i)),
                _ => self.choose_line(i),
            }
            self.dragging = true;
        } else if let Some(i) = secondary
            && !selection.is_some_and(|(a, b)| (a..=b).contains(&i))
        {
            self.choose_line(i);
        }
        if self.dragging {
            if !down {
                self.dragging = false;
            } else if let Some((a, _)) = self.selection {
                if let Some(i) = hovered {
                    self.selection = Some((a, i));
                } else if let Some(p) = pointer {
                    if p.y < area.top() {
                        if let Some(f) = first {
                            self.selection = Some((a, f.saturating_sub(1)));
                        }
                        self.scroll_to = Some(self.scroll - row_h);
                    } else if p.y > area.bottom() {
                        if let Some(l) = last {
                            self.selection = Some((a, (l + 1).min(n - 1)));
                        }
                        self.scroll_to = Some(self.scroll + row_h);
                    }
                    ui.ctx().request_repaint();
                }
            }
        }

        if hmax > 0.0 {
            let track = Rect::from_min_max(pos2(text_x, bottom), pos2(right, full.bottom()));
            let id = egui::Id::new(("blame-hbar", self.id));
            hscrollbar(ui, id, track, &mut self.hoff, text_w / content_w, hmax, c);
        }

        if let Some(action) = action {
            self.act(action, ui.ctx(), requests);
        }
        hovered
    }

    fn act(&mut self, action: LineAction, ctx: &egui::Context, requests: &mut Vec<BlameRequest>) {
        match action {
            LineAction::BlamePrevious(i) => self.blame_previous(i, requests),
            LineAction::ShowChanges(i) => {
                if let Some(spec) = self.origin(i).and_then(Origin::changes)
                    && let Some(line) = self.ready().map(|r| r.blame.lines[i].orig_line)
                {
                    let repo = self.repo.clone();
                    requests.push(BlameRequest::Diff(repo, spec, line as usize));
                }
            }
            LineAction::ShowLog(i) => {
                if let Some(oid) = self.origin(i).and_then(|o| o.commit) {
                    requests.push(BlameRequest::Log(oid));
                }
            }
            LineAction::CopyHash(i) => {
                if let Some(oid) = self.origin(i).and_then(|o| o.commit) {
                    ctx.copy_text(oid.to_hex());
                }
            }
            LineAction::CopyLines => {
                if let Some(text) = self.selected_text() {
                    ctx.copy_text(text);
                }
            }
        }
    }

    /// The commit of the line under the pointer, else the chosen commit: hash, author, date
    /// and subject.
    fn info_bar(&self, ui: &Ui, rect: Rect, hovered: Option<usize>, c: &Colors) {
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, ui.visuals().panel_fill);
        painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, c.line));
        let Some(ready) = self.ready() else { return };
        let blame = &ready.blame;
        let shown = match hovered.and_then(|i| blame.lines.get(i)) {
            Some(line) => Some(line.origin),
            None => self
                .chosen
                .and_then(|commit| blame.origins.iter().position(|o| o.commit == commit)),
        };
        let small = FontId::proportional(12.5);
        let fmt = |color| TextFormat::simple(small.clone(), color);
        let mut job = LayoutJob::default();
        match (shown, self.chosen) {
            (Some(ix), _) => {
                let origin = &blame.origins[ix];
                match origin.commit {
                    Some(oid) => {
                        job.append(
                            &oid.short(self.repo.abbrev_len),
                            0.0,
                            TextFormat::simple(FontId::monospace(12.0), c.weak),
                        );
                        job.append(&origin.author, 12.0, fmt(c.text));
                        job.append(&ready.dates[ix], 12.0, fmt(c.weak));
                        job.append(&origin.summary, 12.0, fmt(c.text));
                        if origin.path != self.spec.path {
                            job.append(&format!("({})", origin.path), 12.0, fmt(c.weak));
                        }
                    }
                    None => job.append(
                        "Not committed yet: changed in the working tree",
                        0.0,
                        fmt(c.note),
                    ),
                }
            }
            // A commit chosen on its own that owns no lines here.
            (None, Some(commit)) => {
                if let Some(oid) = commit {
                    job.append(
                        &oid.short(self.repo.abbrev_len),
                        0.0,
                        TextFormat::simple(FontId::monospace(12.0), c.weak),
                    );
                }
                let text = "None of its lines remain in this version";
                job.append(text, if commit.is_some() { 12.0 } else { 0.0 }, fmt(c.note));
            }
            (None, None) => return,
        }
        job.wrap.max_width = rect.width() - 24.0;
        job.wrap.max_rows = 1;
        let g = painter.layout_job(job);
        painter.galley(
            pos2(rect.left() + 12.0, rect.center().y - g.size().y / 2.0),
            g,
            c.text,
        );
    }

    /// Shows the window; sets `closed` when it was closed. Ctrl+wheel and Ctrl+plus, minus
    /// and 0 change `text_size`.
    fn show(
        &mut self,
        ctx: &egui::Context,
        settings: &mut BlameWindowSettings,
        text_size: &mut f32,
        window_theme: Option<egui::SystemTheme>,
        icon: &Arc<egui::IconData>,
    ) -> Vec<BlameRequest> {
        let builder = egui::ViewportBuilder::default()
            .with_title(self.title())
            .with_app_id(crate::settings::APP_ID)
            .with_icon(icon.clone())
            .with_inner_size(self.size)
            .with_min_inner_size([560.0, 320.0]);
        let id = self.viewport_id();
        if self.title_theme.is_none() && !ctx.embed_viewports() {
            ctx.request_repaint();
        }
        if std::mem::take(&mut self.focus) && !ctx.embed_viewports() {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        }
        let mut requests = Vec::new();
        ctx.show_viewport_immediate(id, builder, |ui, class| {
            self.poll(ui.ctx());
            if class != egui::ViewportClass::EmbeddedWindow {
                if self.title_theme != window_theme {
                    self.title_theme = window_theme;
                    if let Some(theme) = window_theme {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::SetTheme(theme));
                    }
                }
                let (close, size, reload) = ui.input(|i| {
                    (
                        i.viewport().close_requested(),
                        i.viewport().inner_rect.map(|r| r.size()),
                        i.key_pressed(Key::F5),
                    )
                });
                if let Some(size) = size
                    && size.x > 0.0
                    && size.y > 0.0
                {
                    settings.size = [size.x, size.y];
                }
                if reload {
                    self.reload(ui.ctx());
                }
                // Keys go to the main window too when the window is embedded in it.
                self.handle_keys(ui);
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
                    requests = self.contents(ui, settings);
                });
        });
        requests
    }
}

/// The menu of a line. `lines` says several lines are chosen (for copying).
fn line_menu(
    ui: &mut Ui,
    i: usize,
    origin: &Origin,
    in_repo: bool,
    lines: bool,
) -> Option<LineAction> {
    let mut action = None;
    let mut item = |ui: &mut Ui, enabled: bool, text: &str, why: &str, a: LineAction| {
        let r = ui
            .add_enabled(enabled, egui::Button::new(text))
            .on_disabled_hover_text(why);
        if r.clicked() {
            action = Some(a);
            ui.close();
        }
    };
    let (previous_why, changes_why) = if origin.boundary {
        let why = "The history before this commit isn't in this repository";
        (why, why)
    } else {
        ("This commit added the file", "")
    };
    item(
        ui,
        origin.previous_blame().is_some(),
        "Blame previous revision",
        previous_why,
        LineAction::BlamePrevious(i),
    );
    item(
        ui,
        origin.changes().is_some(),
        "Show changes",
        changes_why,
        LineAction::ShowChanges(i),
    );
    let log_why = if origin.commit.is_none() {
        "Not committed yet"
    } else {
        "The commit isn't among the loaded history"
    };
    item(ui, in_repo, "Show log", log_why, LineAction::ShowLog(i));
    crate::menu::separator(ui);
    item(
        ui,
        origin.commit.is_some(),
        "Copy hash",
        "Not committed yet",
        LineAction::CopyHash(i),
    );
    let copy = if lines { "Copy lines" } else { "Copy line" };
    item(ui, true, copy, "", LineAction::CopyLines);
    action
}

/// The runs of consecutive indices below `n` for which `owned` holds, in order.
fn runs(n: usize, owned: impl Fn(usize) -> bool) -> Vec<std::ops::Range<usize>> {
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    for i in (0..n).filter(|&i| owned(i)) {
        match runs.last_mut() {
            Some(run) if run.end == i => run.end = i + 1,
            _ => runs.push(i..i + 1),
        }
    }
    runs
}

/// The tooltip over the gutter: the commit's subject, author and date, and hash.
fn origin_tip(ui: &mut Ui, origin: &Origin, date: &str, abbrev: usize) {
    ui.set_max_width(420.0);
    let Some(oid) = origin.commit else {
        ui.label("Not committed yet: changed in the working tree.");
        return;
    };
    ui.strong(&origin.summary);
    ui.label(format!("{} <{}>", origin.author, origin.author_email));
    ui.weak(format!("{date}   {}", oid.short(abbrev.max(12))));
}

/// Colours of the gutter by age, and of the lines of the chosen commit and their marks in the
/// overview strip.
struct BlameColors {
    old: Color32,
    new: Color32,
    commit: Color32,
    mark: Color32,
}

impl BlameColors {
    /// The gutter's colour for `age`, from the oldest commit (0) to the newest (1).
    fn age(&self, age: f32) -> Color32 {
        let t = age.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        Color32::from_rgb(
            mix(self.old.r(), self.new.r()),
            mix(self.old.g(), self.new.g()),
            mix(self.old.b(), self.new.b()),
        )
    }
}

/// Shades from plain to amber, as TortoiseGitBlame shades from white to yellow; the newest
/// lines are the most amber.
fn blame_colors(ui: &Ui) -> BlameColors {
    if ui.visuals().dark_mode {
        BlameColors {
            old: Color32::from_gray(26),
            new: Color32::from_rgb(0x5c, 0x45, 0x12),
            commit: Color32::from_rgba_unmultiplied(0x35, 0x84, 0xe4, 40),
            mark: Color32::from_rgb(0x62, 0xa0, 0xea),
        }
    } else {
        BlameColors {
            old: Color32::from_gray(250),
            new: Color32::from_rgb(0xff, 0xd9, 0x80),
            commit: Color32::from_rgba_unmultiplied(0x35, 0x84, 0xe4, 26),
            mark: Color32::from_rgb(0x1c, 0x71, 0xd8),
        }
    }
}

impl ParterreApp {
    /// Every open blame window, and what they ask for.
    pub(super) fn blame_windows(&mut self, ctx: &egui::Context) {
        let settings = &mut self.settings;
        let mut requests = Vec::new();
        for window in &mut self.blames.windows {
            requests.extend(window.show(
                ctx,
                &mut settings.blame_window,
                &mut settings.text_size,
                self.window_theme,
                &self.window_icon,
            ));
        }
        self.blames.windows.retain(|w| !w.closed);
        requests.extend(self.blames.take_requests());
        for request in requests {
            match request {
                BlameRequest::Blame(repo, spec, line) => {
                    self.open_blame(repo, spec, Some(line), ctx);
                }
                BlameRequest::Diff(repo, spec, line) => {
                    let settings = &self.settings.diff_window;
                    self.diffs.open_at(repo, spec, Some(line), settings, ctx);
                }
                BlameRequest::Log(oid) => {
                    let Some(repo) = self.repo.clone() else {
                        continue;
                    };
                    if let Some(ix) = repo.lookup(&oid) {
                        self.open_log(repo, &[ix]);
                    }
                }
            }
        }
    }

    /// Opens a blame window on `spec`, with `line` (from 0) chosen.
    pub(super) fn open_blame(
        &mut self,
        repo: Arc<Repo>,
        spec: BlameSpec,
        line: Option<usize>,
        ctx: &egui::Context,
    ) {
        self.blames
            .open(repo, spec, line, &self.settings.blame_window, ctx);
    }

    /// Opens a blame window on `<commit>:<path>` (a ref or hash prefix; `WORKING_TREE` for the
    /// working tree), for `--demo-blame`. A `:<line>` after the path chooses that line (from 1).
    pub(super) fn open_demo_blame(&mut self, spec: &str, ctx: &egui::Context) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let Some((rev, path)) = spec.split_once(':') else {
            eprintln!("--demo-blame: expected <commit>:<path>");
            return;
        };
        let (path, line) = match path.rsplit_once(':') {
            Some((p, l)) if l.parse::<usize>().is_ok() => (p, l.parse::<usize>().ok()),
            _ => (path, None),
        };
        let rev = if rev == "WORKING_TREE" {
            Rev::WorkingTree
        } else {
            let Some(ix) = repo.resolve(rev) else {
                eprintln!("--demo-blame: no commit named {rev}");
                return;
            };
            Rev::Commit(repo.commit(ix).oid)
        };
        let spec = BlameSpec {
            rev,
            path: path.to_owned(),
        };
        self.open_blame(repo, spec, line.map(|l| l.saturating_sub(1)), ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parterre_core::repo::Head;

    const A: &str = "c4275f7dbe7e820e3bb9a21c7d1cc1317657f2d4";
    const B: &str = "8e09b4551acb469f9df4ce58895b77e2e7e4190e";

    fn entry(hash: &str, orig: u32, fin: u32, time: i64, extra: &str, line: &str) -> String {
        format!(
            "{hash} {orig} {fin} 1\nauthor A B\nauthor-mail <a@b>\nauthor-time {time}\n\
             author-tz +0200\nsummary s{time}\n{extra}filename a.txt\n\t{line}\n"
        )
    }

    /// Lines 1, 3 and 4 from A; 2 from B, which changed it.
    fn sample() -> Blame {
        let out = [
            entry(A, 1, 1, 100, "", "one"),
            entry(B, 2, 2, 200, &format!("previous {A} a.txt\n"), "two\tx"),
            entry(A, 3, 3, 100, "", "three"),
            entry(A, 4, 4, 100, "", "four"),
        ]
        .concat();
        Blame::parse(out.as_bytes()).unwrap()
    }

    fn window() -> BlameWindow {
        let repo = Arc::new(Repo::new(
            "/nowhere".into(),
            Vec::new(),
            Vec::new(),
            Head::Branch {
                name: "main".into(),
                target: None,
            },
        ));
        let spec = BlameSpec {
            rev: Rev::Commit(Oid::from_hex(B).unwrap()),
            path: "a.txt".into(),
        };
        let settings = BlameWindowSettings::default();
        let mut w = BlameWindow::new(1, repo.clone(), spec, &settings);
        w.load = Load::Ready(Box::new(Ready::new(sample(), &repo)));
        w
    }

    /// Runs one frame of the window's contents with `events`.
    fn frame(
        ctx: &egui::Context,
        w: &mut BlameWindow,
        events: Vec<egui::Event>,
    ) -> Vec<BlameRequest> {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 700.0))),
            events,
            ..Default::default()
        };
        let mut settings = BlameWindowSettings::default();
        let mut requests = Vec::new();
        ctx.run_ui(input, |ui| requests = w.contents(ui, &mut settings))
            .textures_delta
            .clear();
        requests
    }

    /// The middle of line `i`'s text.
    fn at(ctx: &egui::Context, i: usize) -> egui::Pos2 {
        let font = FontId::monospace(FONT_SIZE);
        let row_h = ctx.fonts_mut(|f| f.row_height(&font)).ceil() + 3.0;
        pos2(700.0, TOOLBAR + HEADER + (i as f32 + 0.5) * row_h)
    }

    fn button(at: egui::Pos2, pressed: bool, secondary: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: if secondary {
                egui::PointerButton::Secondary
            } else {
                egui::PointerButton::Primary
            },
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    #[test]
    fn a_click_chooses_a_line_and_highlights_its_commit_and_a_drag_chooses_more() {
        let ctx = egui::Context::default();
        let mut w = window();
        frame(&ctx, &mut w, Vec::new());
        let p = at(&ctx, 2);
        frame(
            &ctx,
            &mut w,
            vec![egui::Event::PointerMoved(p), button(p, true, false)],
        );
        frame(&ctx, &mut w, vec![button(p, false, false)]);
        assert_eq!(w.selection, Some((2, 2)));
        assert_eq!(w.chosen, Some(Some(Oid::from_hex(A).unwrap())));

        let q = at(&ctx, 1);
        frame(
            &ctx,
            &mut w,
            vec![egui::Event::PointerMoved(q), button(q, true, false)],
        );
        frame(&ctx, &mut w, vec![egui::Event::PointerMoved(at(&ctx, 3))]);
        frame(&ctx, &mut w, vec![button(at(&ctx, 3), false, false)]);
        assert_eq!(w.selection, Some((1, 3)));
        // The drag started on B's line: B stays chosen over A's lines.
        assert_eq!(w.chosen, Some(Some(Oid::from_hex(B).unwrap())));
        // Copied as in the file: tabs kept, a newline after each line.
        assert_eq!(w.selected_text().as_deref(), Some("two\tx\nthree\nfour\n"));
    }

    /// A click at line `i`, pressed and released, with `modifiers`.
    fn click(ctx: &egui::Context, w: &mut BlameWindow, i: usize, modifiers: Modifiers) {
        let p = at(ctx, i);
        let press = egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers,
        };
        let changed = egui::Event::ModifiersChanged;
        frame(
            ctx,
            w,
            vec![changed(modifiers), egui::Event::PointerMoved(p), press],
        );
        frame(
            ctx,
            w,
            vec![button(p, false, false), changed(Modifiers::NONE)],
        );
    }

    #[test]
    fn shift_click_chooses_more_lines_and_keeps_the_commit_and_a_second_click_keeps_it() {
        let ctx = egui::Context::default();
        let mut w = window();
        frame(&ctx, &mut w, Vec::new());
        click(&ctx, &mut w, 1, Modifiers::NONE);
        click(&ctx, &mut w, 1, Modifiers::NONE);
        assert_eq!(w.chosen, Some(Some(Oid::from_hex(B).unwrap())));
        click(&ctx, &mut w, 3, Modifiers::SHIFT);
        assert_eq!(w.selection, Some((1, 3)));
        assert_eq!(w.chosen, Some(Some(Oid::from_hex(B).unwrap())));

        // A right-click inside the chosen lines keeps them; outside, it chooses its line.
        let p = at(&ctx, 2);
        frame(
            &ctx,
            &mut w,
            vec![egui::Event::PointerMoved(p), button(p, true, true)],
        );
        frame(&ctx, &mut w, vec![button(p, false, true)]);
        assert_eq!(w.selection, Some((1, 3)));
        assert_eq!(w.chosen, Some(Some(Oid::from_hex(B).unwrap())));
        let p = at(&ctx, 0);
        frame(
            &ctx,
            &mut w,
            vec![egui::Event::PointerMoved(p), button(p, true, true)],
        );
        frame(&ctx, &mut w, vec![button(p, false, true)]);
        assert_eq!(w.selection, Some((0, 0)));
        assert_eq!(w.chosen, Some(Some(Oid::from_hex(A).unwrap())));
    }

    #[test]
    fn choosing_a_commit_chooses_its_first_line_without_scrolling() {
        let mut w = window();
        w.choose_commit(Some(Oid::from_hex(B).unwrap()));
        assert_eq!(w.selection, Some((1, 1)));
        assert_eq!(w.chosen, Some(Some(Oid::from_hex(B).unwrap())));
        assert_eq!(w.scroll_to, None);

        // A commit that owns no lines here is chosen, with no line.
        let gone = Oid::from_hex("0123456789abcdef0123456789abcdef01234567").unwrap();
        w.choose_commit(Some(gone));
        assert_eq!(w.selection, None);
        assert_eq!(w.chosen, Some(Some(gone)));
        assert_eq!(w.scroll_to, None);
    }

    #[test]
    fn blaming_the_previous_revision_asks_for_a_window_of_its_own() {
        let ctx = egui::Context::default();
        let mut w = window();
        frame(&ctx, &mut w, Vec::new());
        w.choose_line(1);
        let mut requests = Vec::new();
        w.act(LineAction::BlamePrevious(1), &ctx, &mut requests);
        // The line's place in its commit's version is chosen there.
        let [BlameRequest::Blame(_, spec, 1)] = requests.as_slice() else {
            panic!("expected a blame: {requests:?}");
        };
        assert_eq!(
            *spec,
            BlameSpec {
                rev: Rev::Commit(Oid::from_hex(A).unwrap()),
                path: "a.txt".into()
            }
        );
        // This window stays as it was.
        assert_eq!(w.spec.rev, Rev::Commit(Oid::from_hex(B).unwrap()));
        assert!(w.ready().is_some());
        assert_eq!(w.selection, Some((1, 1)));
    }

    #[test]
    fn opening_a_blame_already_open_brings_it_forward_with_the_line_chosen() {
        let ctx = egui::Context::default();
        let w = window();
        let (repo, spec) = (w.repo.clone(), w.spec.clone());
        let mut windows = BlameWindows {
            windows: vec![w],
            opened: 1,
            requests: Vec::new(),
        };
        let settings = BlameWindowSettings::default();
        windows.open(repo, spec, Some(2), &settings, &ctx);
        let [w] = windows.windows.as_slice() else {
            panic!("expected one window");
        };
        assert!(w.focus);
        assert_eq!(w.pending_line, Some(2));
        // Not blamed again: a commit's blame can't change.
        assert!(w.ready().is_some());
    }

    #[test]
    fn a_line_of_the_first_commit_has_nothing_before_it() {
        let w = window();
        let origin = w.origin(0).unwrap();
        assert_eq!(origin.previous_blame(), None);
        // Its change is the file being added.
        let spec = origin.changes().unwrap();
        assert_eq!(spec.old, None);
        let mut requests = Vec::new();
        let mut w = w;
        w.act(
            LineAction::ShowChanges(1),
            &egui::Context::default(),
            &mut requests,
        );
        let [BlameRequest::Diff(_, spec, 1)] = requests.as_slice() else {
            panic!("expected a diff: {requests:?}");
        };
        assert_eq!(
            spec.old.as_ref().map(|v| v.rev),
            Some(Rev::Commit(Oid::from_hex(A).unwrap()))
        );
    }

    #[test]
    fn f5_blames_only_the_working_tree_again() {
        let ctx = egui::Context::default();
        let mut w = window();
        w.reload(&ctx);
        assert!(w.ready().is_some());

        w.spec.rev = Rev::WorkingTree;
        w.scroll = 40.0;
        w.reload(&ctx);
        assert!(matches!(w.load, Load::Loading(_)));
        assert_eq!(w.pending_scroll, Some(40.0));
    }

    #[test]
    fn a_reload_keeps_the_chosen_commit_while_it_owns_lines() {
        let repo = window().repo;
        let mut w = window();
        w.choose_line(1);
        w.loaded(Ready::new(sample(), &repo));
        assert_eq!(w.chosen, Some(Some(Oid::from_hex(B).unwrap())));
        assert_eq!(w.selection, Some((1, 1)));

        // B's line is gone, and so are lines 3 and 4.
        let out = [
            entry(A, 1, 1, 100, "", "one"),
            entry(A, 2, 2, 100, "", "two"),
        ]
        .concat();
        w.selection = Some((1, 3));
        w.loaded(Ready::new(Blame::parse(out.as_bytes()).unwrap(), &repo));
        assert_eq!(w.chosen, None);
        assert_eq!(w.selection, None);
    }

    #[test]
    fn runs_join_consecutive_lines() {
        assert_eq!(
            runs(8, |i| [1, 2, 3, 5, 7].contains(&i)),
            [1..4, 5..6, 7..8]
        );
        assert!(runs(3, |_| false).is_empty());
    }

    #[test]
    fn choosing_never_scrolls_and_the_overview_strip_does() {
        let ctx = egui::Context::default();
        let mut w = window();
        // 300 lines from A, but line 250 from B.
        let out: String = (1..=300)
            .map(|i| match i {
                251 => entry(B, i, i, 200, &format!("previous {A} a.txt\n"), "b"),
                _ => entry(A, i, i, 100, "", "a"),
            })
            .collect();
        let repo = w.repo.clone();
        w.loaded(Ready::new(Blame::parse(out.as_bytes()).unwrap(), &repo));
        frame(&ctx, &mut w, Vec::new());
        w.choose_commit(Some(Oid::from_hex(B).unwrap()));
        assert_eq!(w.selection, Some((250, 250)));
        frame(&ctx, &mut w, Vec::new());
        frame(&ctx, &mut w, Vec::new());
        assert_eq!(w.scroll, 0.0);

        // A click on the strip at B's mark puts line 250 in the middle.
        let font = FontId::monospace(FONT_SIZE);
        let row_h = ctx.fonts_mut(|f| f.row_height(&font)).ceil() + 3.0;
        let (top, bottom) = (TOOLBAR + HEADER, 700.0 - INFO);
        let scale = ((bottom - top) / 300.0).min(row_h);
        let p = pos2(1000.0 - OVERVIEW / 2.0, top + 250.5 * scale);
        frame(
            &ctx,
            &mut w,
            vec![egui::Event::PointerMoved(p), button(p, true, false)],
        );
        frame(&ctx, &mut w, vec![button(p, false, false)]);
        frame(&ctx, &mut w, Vec::new());
        let middle = (w.scroll + (bottom - top) / 2.0) / row_h;
        assert!((middle - 250.5).abs() < 2.0, "line {middle} in the middle");
        // The chosen commit and line stay.
        assert_eq!(w.selection, Some((250, 250)));
        assert_eq!(w.chosen, Some(Some(Oid::from_hex(B).unwrap())));
    }

    #[test]
    fn the_gutter_shades_from_old_to_new() {
        let c = BlameColors {
            old: Color32::from_rgb(0, 0, 0),
            new: Color32::from_rgb(200, 100, 50),
            commit: Color32::TRANSPARENT,
            mark: Color32::TRANSPARENT,
        };
        assert_eq!(c.age(0.0), c.old);
        assert_eq!(c.age(1.0), c.new);
        assert_eq!(c.age(0.5), Color32::from_rgb(100, 50, 25));
    }
}
