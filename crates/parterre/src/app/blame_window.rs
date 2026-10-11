use std::collections::HashMap;
use std::sync::{Arc, mpsc};

use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{
    self, Color32, FontId, Id, Key, Modifiers, PopupCloseBehavior, Pos2, Rect, RectAlign, RichText,
    ScrollArea, Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2,
};
use parterre_core::blame::{Blame, BlameOptions, BlameSpec, Moves, Origin};
use parterre_core::changed_files::FileStatus;
use parterre_core::file_diff::{FileDiffSpec, Rev, Version, display_column};
use parterre_core::file_history::{FileHistory, FileLog, HistoryRow, Source};
use parterre_core::find;
use parterre_core::git::{CommitDetails, Git};
use parterre_core::glyphs;
use parterre_core::log_graph::LogGraph;
use parterre_core::repo::cmp_refs_for_display;
use parterre_core::revgraph::GraphOptions;
use parterre_core::text::{line_number, thousands, word_at};
use parterre_core::{CommitIx, Oid, Repo};
use parterre_highlight::{self as highlight, Engine, Spans};
use parterre_util::Cancel;

use super::commit_table::{CommitList, CommitTable, ROW_HEIGHT, Row, Select};
use super::divider::{Bar, DIVIDER_THICKNESS, divider};
use super::table_style::{self, CELL_PAD, HEADING_HEIGHT, cell};
use super::text_view::{
    Colors, Find, FindInText, HEADER_HEIGHT, LINE_END, Load, OVERVIEW_WIDTH, SCROLLBAR_HEIGHT,
    ScrollView, TOOL_GAP, TextAt, TextMetrics, TextRow, chosen_columns, colors, file_window_title,
    hscrollbar, line_numbers_width, message, overview_background, overview_found, overview_mark,
    overview_scale, overview_scroll, overview_view, paint_text, raw_slice, reveal, reveal_columns,
    scroll_sideways, text_font, third_of_the_way_down, toolbar_row,
};
use super::viewport::{self, Viewport};
use super::{Details, MESSAGE_TIP_MAX_CHARS, MESSAGE_TIP_MAX_LINES, ParterreApp, syntax};
use crate::keys;
use crate::settings::{BlameWindowSettings, Settings};
use crate::text_size;
use crate::theme::Palette;
use crate::usage::Screen;
use crate::widgets;
use crate::worker;

const INFO_BAR_HEIGHT: f32 = 26.0;
const AUTHOR_WIDTH: f32 = 130.0;
const DATE_WIDTH: f32 = 84.0;
const MIN_HISTORY_HEIGHT: f32 = HEADING_HEIGHT + ROW_HEIGHT;
const MIN_TEXT_HEIGHT: f32 = 80.0;

#[derive(Debug)]
pub enum BlameRequest {
    /// With the line (from 0) to choose.
    Blame(Arc<Repo>, BlameSpec, Option<usize>),
    /// At a line (from 0) of the new version.
    Diff(Arc<Repo>, FileDiffSpec, usize),
    Log(Oid),
    Fetch(egui::ViewportId),
}

#[derive(Debug, Default)]
pub struct BlameWindows {
    windows: Vec<BlameWindow>,
    last_id: u64,
    engine: Engine,
}

impl BlameWindows {
    pub fn new(engine: Engine) -> BlameWindows {
        BlameWindows {
            engine,
            ..BlameWindows::default()
        }
    }

    /// Opens a blame window for `spec`, or brings the one already showing it to the front,
    /// blamed again if it reads the working tree. `line` (from 0) is chosen and scrolled to.
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
            w.viewport.bring_to_front = true;
            return;
        }
        self.last_id += 1;
        let mut w = BlameWindow::new(self.last_id, repo, spec, settings, &self.engine);
        w.pending_line = line;
        w.load(ctx);
        self.windows.push(w);
    }

    pub fn close_all(&mut self) {
        self.windows.clear();
    }

    pub fn is_loading(&self) -> bool {
        self.windows
            .iter()
            .any(|w| w.load.is_loading() || matches!(w.listing, Listing::Running(_)))
    }
}

#[derive(Debug)]
struct Ready {
    blame: Blame,
    /// Per origin: from 0 for the oldest of the file's commits to 1 for the newest.
    ages: Vec<f32>,
    /// Per origin: the author date as the log shows it, in local time, or in the author's zone
    /// for a commit the snapshot lacks; empty for uncommitted lines.
    dates: Vec<String>,
    /// Per origin.
    in_snapshot: Vec<bool>,
    commit_count: usize,
    /// Per line, on its display text, when the language is known.
    syntax: Option<Vec<Spans>>,
}

impl Ready {
    fn new(blame: Blame, repo: &Repo, path: &str, engine: &Engine, cancel: &Cancel) -> Ready {
        let syntax = highlight::language_of(path).map(|language| {
            let text: Vec<&str> = blame.lines.iter().map(|l| l.raw.as_str()).collect();
            let spans = engine.highlight(language, &text.join("\n"), cancel);
            blame
                .lines
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let line_spans = spans.get(i).map_or(&[][..], Vec::as_slice);
                    syntax::display_spans(&l.raw, line_spans)
                })
                .collect()
        });
        let ages = blame.ages();
        let (dates, in_snapshot) = blame
            .origins
            .iter()
            .map(|o| match o.commit.and_then(|oid| repo.lookup(&oid)) {
                Some(ix) => (repo.commit(ix).author_date.clone(), true),
                None if o.commit.is_some() => (o.author_date(), false),
                None => (String::new(), false),
            })
            .unzip();
        Ready {
            commit_count: blame.commit_count(),
            blame,
            ages,
            dates,
            in_snapshot,
            syntax,
        }
    }
}

/// The history pane's rows, or how far listing them got.
#[derive(Debug)]
enum Listing {
    Running(mpsc::Receiver<Result<FileLog, String>>),
    /// The rows wait for the blame.
    Listed(FileLog),
    Ready(Box<History>),
    Failed(String),
}

#[derive(Debug)]
struct History {
    history: FileHistory,
    graph: LogGraph,
    /// Per row: indices into the snapshot's refs at its commit, in the log's order.
    refs: Vec<Vec<usize>>,
    /// Per row, if its commit owns lines.
    ages: Vec<Option<f32>>,
    /// Per row.
    first_lines: Vec<Option<usize>>,
}

impl History {
    fn new(log: FileLog, ready: &Ready, repo: &Repo) -> History {
        let blame = &ready.blame;
        let history = FileHistory::new(log, blame, repo);
        let n = history.rows.len();
        let mut ages = vec![None; n];
        for (origin, age) in blame.origins.iter().zip(&ready.ages) {
            if let Some(row) = history.row_of(origin.commit) {
                ages[row] = Some(*age);
            }
        }
        let mut first_lines = vec![None; n];
        for (i, line) in blame.lines.iter().enumerate() {
            if let Some(row) = history.row_of(blame.origins[line.origin].commit) {
                first_lines[row].get_or_insert(i);
            }
        }
        let rows: HashMap<CommitIx, usize> = history
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| Some((r.snapshot?, i)))
            .collect();
        let mut refs = vec![Vec::new(); n];
        for (k, r) in repo.refs.iter().enumerate() {
            if let Some(&row) = rows.get(&r.target) {
                refs[row].push(k);
            }
        }
        for on in &mut refs {
            on.sort_by(|&a, &b| cmp_refs_for_display(&repo.refs[a], &repo.refs[b]));
        }
        History {
            graph: history.graph(),
            history,
            refs,
            ages,
            first_lines,
        }
    }

    /// The file's path at row `i`'s commit: where the log ran, or for a row only the blame
    /// names, the path its first line had there.
    fn path<'a>(&'a self, i: usize, spec: &'a BlameSpec) -> &'a str {
        match &self.history.rows[i].source {
            Source::WorkingTree => &spec.path,
            Source::Log => &self.history.path,
            Source::Blame { paths } => paths.first().unwrap_or(&self.history.path),
        }
    }
}

/// What the history pane takes from the app's settings.
struct Env<'a> {
    palette: Palette,
    graph: &'a GraphOptions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowAction {
    Blame(usize),
    ShowChanges(usize),
    ShowLog(usize),
    CopyHash(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineAction {
    BlamePrevious(usize),
    ShowChanges(usize),
    ShowLog(usize),
    CopyHash(usize),
    CopyLines,
}

/// What is chosen for copying, from `anchor` to `head`: each a line and a display column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Selection {
    anchor: (usize, usize),
    head: (usize, usize),
    whole_lines: bool,
}

impl Selection {
    fn of_lines(first: usize, last: usize) -> Selection {
        Selection {
            anchor: (first, 0),
            head: (last, 0),
            whole_lines: true,
        }
    }

    /// The first and last line it touches.
    fn span(&self) -> (usize, usize) {
        let (a, b) = (self.anchor.0, self.head.0);
        (a.min(b), a.max(b))
    }

    fn touches(&self, i: usize) -> bool {
        let (first, last) = self.span();
        (first..=last).contains(&i)
    }

    /// Where the chosen characters start and end, in order. `None` for whole lines, which a
    /// click on the text without a drag chooses too.
    fn chosen_chars(&self) -> Option<((usize, usize), (usize, usize))> {
        if self.whole_lines || self.anchor == self.head {
            None
        } else if self.anchor < self.head {
            Some((self.anchor, self.head))
        } else {
            Some((self.head, self.anchor))
        }
    }

    fn columns(&self, i: usize) -> Option<std::ops::Range<usize>> {
        let (start, end) = self.chosen_chars()?;
        chosen_columns(start, end, i)
    }
}

#[derive(Debug, Default)]
struct GoTo {
    open: bool,
    text: String,
    /// Focus the field once the popup is drawn for real: its first frame only sizes it.
    focus: bool,
}

#[derive(Debug)]
struct BlameWindow {
    id: u64,
    repo: Arc<Repo>,
    spec: BlameSpec,
    /// The viewport builder must not change while the window is open.
    initial_size: Vec2,
    load: Load<Ready>,
    listing: Listing,
    stop_listing: Cancel,
    /// Not `stop_listing`: a handle holds one child process at a time.
    stop_colouring: Cancel,
    engine: Engine,
    history_list: CommitList,
    /// Whole commit messages, fetched the first time a row's subject is hovered.
    details: Details,
    show_history: bool,
    history_height: f32,
    options: BlameOptions,
    requested_options: BlameOptions,
    selection: Option<Selection>,
    /// `Some(None)` is the uncommitted lines.
    chosen_commit: Option<Option<Oid>>,
    dragging: bool,
    /// Chosen and scrolled to once loaded.
    pending_line: Option<usize>,
    /// Scrolled to once loaded again.
    pending_scroll: Option<f32>,
    scroll_to: Option<f32>,
    scroll_y: f32,
    scroll_x: f32,
    top: usize,
    find: Find<find::Match>,
    go_to: GoTo,
    reveal_line: Option<usize>,
    viewport: Viewport,
    closed: bool,
}

impl BlameWindow {
    /// Not loading yet: see [`BlameWindow::load`].
    fn new(
        id: u64,
        repo: Arc<Repo>,
        spec: BlameSpec,
        settings: &BlameWindowSettings,
        engine: &Engine,
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
            initial_size: vec2(w, h),
            load: Load::Failed(String::new()),
            listing: Listing::Failed(String::new()),
            stop_listing: Cancel::new(),
            stop_colouring: Cancel::new(),
            engine: engine.clone(),
            history_list: CommitList::default(),
            details: Details::default(),
            show_history: settings.show_history,
            history_height: settings.history_height,
            options,
            requested_options: options,
            selection: None,
            chosen_commit: None,
            dragging: false,
            pending_line: None,
            pending_scroll: None,
            scroll_to: None,
            scroll_y: 0.0,
            scroll_x: 0.0,
            top: 0,
            find: Find::default(),
            go_to: GoTo::default(),
            reveal_line: None,
            viewport: Viewport::default(),
            closed: false,
        }
    }

    /// Blames the file and lists its history on worker threads, stopping those of the last
    /// load.
    fn load(&mut self, ctx: &egui::Context) {
        self.stop_listing.cancel();
        self.stop_listing = Cancel::new();
        self.stop_colouring.cancel();
        self.stop_colouring = Cancel::new();
        let git = Git::new(&self.repo.path);
        let (spec, options, repo) = (self.spec.clone(), self.options, self.repo.clone());
        let (engine, cancel) = (self.engine.clone(), self.stop_colouring.clone());
        self.load = Load::Loading(worker::spawn(ctx, move || {
            git.blame(&spec, options)
                .map(|blame| Ready::new(blame, &repo, &spec.path, &engine, &cancel))
                .map_err(|e| e.to_string())
        }));
        self.requested_options = options;
        self.dragging = false;

        let (git, spec, cancel) = (
            Git::new(&self.repo.path),
            self.spec.clone(),
            self.stop_listing.clone(),
        );
        self.listing = Listing::Running(worker::spawn(ctx, move || {
            git.file_log(&spec, &cancel).map_err(|e| e.to_string())
        }));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.load.received() {
            match result {
                Ok(ready) => self.loaded(ready),
                Err(e) => self.load = Load::Failed(e),
            }
        }
        if let Listing::Running(rx) = &self.listing
            && let Ok(result) = rx.try_recv()
        {
            self.listing = match result {
                Ok(log) => Listing::Listed(log),
                Err(e) => Listing::Failed(e),
            };
        }
        self.settle();
        if self.options != self.requested_options {
            // Keep the place: the lines stay, only their commits may change.
            self.pending_scroll = Some(self.scroll_y);
            self.load(ctx);
        }
    }

    /// Keeps the chosen lines if the file still has them; whether the chosen commit stays is
    /// up to [`BlameWindow::settle`].
    fn loaded(&mut self, ready: Ready) {
        if self
            .selection
            .is_some_and(|s| s.span().1 >= ready.blame.lines.len())
        {
            self.selection = None;
        }
        self.load = Load::Ready(Box::new(ready));
        // The lines are the same but for a working tree's: the place gone to stays if it can.
        let current = self.find.current;
        self.refind();
        self.find.current = current.filter(|&c| c < self.find.matches.len());
    }

    /// Makes the history pane's rows once the blame and the listing are both there, and selects
    /// the chosen commit's row. The chosen commit stays while it is still listed, or if the
    /// listing failed, while it still owns lines.
    fn settle(&mut self) {
        let Some(ready) = self.load.ready() else {
            return;
        };
        let keep = match std::mem::replace(&mut self.listing, Listing::Failed(String::new())) {
            Listing::Listed(log) => {
                let history = History::new(log, ready, &self.repo);
                let row = self.chosen_commit.and_then(|c| history.history.row_of(c));
                self.history_list.select(row);
                self.listing = Listing::Ready(Box::new(history));
                row.is_some()
            }
            Listing::Failed(e) => {
                self.listing = Listing::Failed(e);
                self.chosen_commit
                    .is_some_and(|c| ready.blame.origins.iter().any(|o| o.commit == c))
            }
            unchanged => {
                self.listing = unchanged;
                return;
            }
        };
        if !keep {
            self.chosen_commit = None;
        }
    }

    /// F5: blames the working tree again, keeping the place. A commit's blame can't change.
    fn reload(&mut self, ctx: &egui::Context) {
        if self.spec.reads_working_tree() {
            self.pending_scroll = Some(self.scroll_y);
            self.load(ctx);
        }
    }

    fn ready(&self) -> Option<&Ready> {
        self.load.ready()
    }

    fn history(&self) -> Option<&History> {
        match &self.listing {
            Listing::Ready(h) => Some(h),
            _ => None,
        }
    }

    fn viewport_id(&self) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(("blame", self.id))
    }

    fn title(&self) -> String {
        file_window_title(&self.spec.path, Some(self.spec.rev), &self.repo, "Blame")
    }

    fn origin(&self, i: usize) -> Option<&Origin> {
        let blame = &self.ready()?.blame;
        blame.origins.get(blame.lines.get(i)?.origin)
    }

    /// Chooses line `i` and its commit, and selects the commit's row in the history pane.
    fn choose_line(&mut self, i: usize) {
        self.selection = Some(Selection::of_lines(i, i));
        if let Some(commit) = self.origin(i).map(|o| o.commit) {
            self.chosen_commit = Some(commit);
            if let Some(h) = self.history() {
                let row = h.history.row_of(commit);
                self.history_list.select(row);
            }
        }
    }

    /// Selects row `i` of the history pane, and chooses its commit.
    fn choose_row(&mut self, i: usize) {
        let Some(commit) = self
            .history()
            .and_then(|h| h.history.rows.get(i))
            .map(|r| r.commit)
        else {
            return;
        };
        self.history_list.select(Some(i));
        self.choose_commit(commit);
    }

    /// `Up` (`by` < 0, to newer commits) or `Down`: chooses the row next to the selected one,
    /// or the first row.
    fn step_row(&mut self, by: isize) {
        let Some(n) = self.history().map(|h| h.history.rows.len()) else {
            return;
        };
        if n == 0 {
            return;
        }
        let i = match self.history_list.selected {
            Some(i) => i.saturating_add_signed(by).min(n - 1),
            None => 0,
        };
        self.choose_row(i);
    }

    /// Chooses `commit` (`None` for the uncommitted lines) and its first line, or no line if it
    /// owns none in this version. The text stays where it is.
    fn choose_commit(&mut self, commit: Option<Oid>) {
        self.chosen_commit = Some(commit);
        self.selection = self.ready().and_then(|r| {
            let first = r
                .blame
                .lines
                .iter()
                .position(|l| r.blame.origins[l.origin].commit == commit)?;
            Some(Selection::of_lines(first, first))
        });
    }

    /// Every line the choice touches, whole, as in the file, each ending with a newline.
    fn lines_text(&self) -> Option<String> {
        let (first, last) = self.selection?.span();
        let lines = &self.ready()?.blame.lines;
        let last = last.min(lines.len().checked_sub(1)?);
        let mut text = String::new();
        for line in lines.get(first..=last)? {
            text.push_str(&line.raw);
            text.push('\n');
        }
        Some(text)
    }

    fn select_all(&mut self) {
        if let Some(n) = self.ready().map(|r| r.blame.lines.len())
            && n > 0
        {
            self.selection = Some(Selection::of_lines(0, n - 1));
        }
    }

    fn go_to_id(&self) -> Id {
        Id::new(("blame-go-to", self.id))
    }

    /// Ctrl+G, once there are lines to go to.
    fn open_go_to(&mut self) {
        if self.ready().is_some_and(|r| !r.blame.lines.is_empty()) {
            self.go_to = GoTo {
                open: true,
                text: String::new(),
                focus: true,
            };
        }
    }

    fn close_go_to(&mut self, ctx: &egui::Context) {
        self.go_to.open = false;
        ctx.memory_mut(|m| m.surrender_focus(self.go_to_id()));
    }

    /// The line (from 0) the go-to field names, if the file has it.
    fn typed_line(&self) -> Option<usize> {
        line_number(&self.go_to.text, self.ready()?.blame.lines.len())
    }

    /// Chooses the line typed, whole, and so its commit and row, scrolls it into view and
    /// closes the popup. False, doing nothing, if the file has no such line.
    fn go_to_typed_line(&mut self, ctx: &egui::Context) -> bool {
        let Some(i) = self.typed_line() else {
            return false;
        };
        self.choose_line(i);
        self.reveal_line = Some(i);
        self.close_go_to(ctx);
        true
    }

    /// Asks for a blame of the version before line `i`'s commit, with the line's place in it
    /// chosen.
    fn blame_previous(&self, i: usize, requests: &mut Vec<BlameRequest>) {
        let Some(spec) = self.origin(i).and_then(Origin::previous_blame) else {
            return;
        };
        if let Some(line) = self.ready().map(|r| r.blame.lines[i].orig_line as usize) {
            requests.push(BlameRequest::Blame(self.repo.clone(), spec, Some(line)));
        }
    }

    fn handle_keys(&mut self, ui: &Ui) {
        let in_find = self.find_keys(ui);
        if ui.input_mut(|i| keys::go_to_line().consume(i)) {
            self.open_go_to();
        }
        if self.go_to.open {
            // Esc closes the popup, never the window.
            if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                self.close_go_to(ui.ctx());
            }
            return;
        }
        if in_find {
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
        // Wherever the pointer is.
        if self.show_history && self.history().is_some() {
            let step = ui.input_mut(|i| {
                if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    Some(-1)
                } else if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    Some(1)
                } else {
                    None
                }
            });
            if let Some(by) = step {
                self.step_row(by);
            }
        }
        if ui.input_mut(|i| keys::close_window().consume(i)) {
            self.closed = true;
        }
    }

    /// The text, and the divider and the history pane below it, in `above`: the pane as tall as
    /// asked, less if the text would get less than [`MIN_TEXT_HEIGHT`].
    fn arrange(&self, above: Rect) -> (Rect, Option<(Rect, Rect)>) {
        if !self.show_history {
            return (above, None);
        }
        let height = self
            .history_height
            .min(above.height() - DIVIDER_THICKNESS - MIN_TEXT_HEIGHT);
        let top = above.bottom() - height.max(0.0);
        let bar = Rect::from_x_y_ranges(above.x_range(), top - DIVIDER_THICKNESS..=top);
        let pane = Rect::from_min_max(pos2(above.left(), top), above.max);
        let body = Rect::from_min_max(above.min, pos2(above.right(), bar.top().max(above.top())));
        (body, Some((bar, pane)))
    }

    fn contents(
        &mut self,
        ui: &mut Ui,
        settings: &mut BlameWindowSettings,
        syntax: &mut bool,
        env: &Env,
    ) -> Vec<BlameRequest> {
        let c = colors(ui);
        self.toolbar(ui, settings, syntax);
        self.header(ui, &c);
        let rest = ui.available_rect_before_wrap();
        let above = Rect::from_min_max(
            rest.min,
            pos2(rest.right(), rest.bottom() - INFO_BAR_HEIGHT),
        );
        let info = Rect::from_min_max(pos2(rest.left(), above.bottom()), rest.max);
        let (body, history) = self.arrange(above);
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
                hovered = self.body(&mut child, body, *syntax, &c, &mut requests);
            }
        }
        if let Some((bar, pane)) = history {
            self.history_pane(ui, pane, env, &mut requests);
            let bar = Bar {
                rect: bar,
                vertical: false,
                origin: 0.0,
                room: 1.0,
            };
            if let Some(middle) = divider(ui, Id::new(("blame-divider", self.id)), &bar) {
                let most =
                    (above.height() - DIVIDER_THICKNESS - MIN_TEXT_HEIGHT).max(MIN_HISTORY_HEIGHT);
                let height = above.bottom() - (middle + DIVIDER_THICKNESS / 2.0);
                self.history_height = height.clamp(MIN_HISTORY_HEIGHT, most);
                settings.history_height = self.history_height;
            }
        }
        self.info_bar(ui, info, hovered, &c);
        ui.allocate_rect(rest, Sense::hover());
        requests
    }

    fn toolbar(&mut self, ui: &mut Ui, settings: &mut BlameWindowSettings, syntax: &mut bool) {
        let (_, mut bar) = toolbar_row(ui);
        let ui = &mut bar;
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
        ui.add_space(TOOL_GAP);

        let moves = Moves::ALL.map(|m| (m, moves_glyph(m)));
        let picked = widgets::segmented(ui, self.options.moves, &moves, |m, r| {
            let title = format!("Moved lines: {}", m.label().to_lowercase());
            widgets::tip_explained(r, &title, "", m.description())
        });
        if let Some(moves) = picked {
            self.options.moves = moves;
            settings.moves = moves;
        }
        ui.add_space(TOOL_GAP);
        widgets::syntax_button(ui, syntax);
        if self.spec.reads_working_tree() {
            ui.add_space(TOOL_GAP);
            let text = format!("{} blames again", keys::reload_label());
            ui.label(RichText::new(text).size(12.0).color(weak));
        }

        let find = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let on = self.show_history;
            let title = if on {
                "Hide the history pane"
            } else {
                "Show the history pane"
            };
            let body = "Every commit that changed this file, in a list below the text. Choosing \
                        a row chooses its commit's lines; Up and Down step through the rows.";
            let response = widgets::icon_button(ui, glyphs::HISTORY, on);
            if widgets::tip_explained(response, title, "", body).clicked() {
                self.show_history = !on;
                settings.show_history = self.show_history;
            }

            // In the middle of what is left, as in the main window.
            let room = ui.available_width();
            ui.allocate_ui_with_layout(
                vec2(room, widgets::BUTTON_SIZE),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    // Narrow windows squeeze the field rather than the tools.
                    let width = (room - 16.0).clamp(60.0, 380.0);
                    ui.add_space(((room - width) / 2.0).max(0.0));
                    let left = ui.cursor().left();
                    self.find_field(ui, width, "Find in the file");
                    let rect = ui.min_rect();
                    Rect::from_x_y_ranges(left..=rect.right(), rect.y_range())
                },
            )
            .inner
        });
        self.go_to_popup(ui, find.inner);
    }

    /// The go-to-line popup, while it is open, centred under `under`.
    fn go_to_popup(&mut self, ui: &Ui, under: Rect) {
        let Some(n) = self.ready().map(|r| r.blame.lines.len()) else {
            return;
        };
        let mut open = self.go_to.open;
        let id = self.go_to_id().with("popup");
        egui::Popup::new(id, ui.ctx().clone(), under, ui.layer_id())
            .open_bool(&mut open)
            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
            .align(RectAlign::BOTTOM)
            .gap(4.0)
            .style(crate::menu::popover_style)
            .show(|ui| self.go_to_field(ui, n));
        if !open && self.go_to.open {
            self.close_go_to(ui.ctx());
        }
    }

    /// The field, marked while it names none of the `n` lines, and the lines there are.
    fn go_to_field(&mut self, ui: &mut Ui, n: usize) {
        ui.weak("Go to line");
        let invalid = !self.go_to.text.trim().is_empty() && self.typed_line().is_none();
        let id = self.go_to_id();
        let focused = ui.memory(|m| m.has_focus(id));
        let t = widgets::tones(ui);
        let error = ui.visuals().error_fg_color;
        let stroke = if invalid {
            Stroke::new(1.5, error)
        } else if focused {
            Stroke::new(1.5, t.accent)
        } else {
            Stroke::new(1.0, t.field_line)
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let response = egui::Frame::new()
                .fill(t.field)
                .stroke(stroke)
                .corner_radius(7)
                .inner_margin(egui::Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.go_to.text)
                            .id(id)
                            .frame(egui::Frame::NONE)
                            .hint_text("Line number")
                            .desired_width(110.0),
                    )
                })
                .inner;
            if self.go_to.focus && !ui.is_sizing_pass() {
                response.request_focus();
                self.go_to.focus = false;
            }
            let range = format!("1–{}", thousands(n));
            let color = if invalid {
                error
            } else {
                ui.visuals().weak_text_color()
            };
            ui.label(RichText::new(range).color(color));
            if response.lost_focus()
                && ui.input(|i| i.key_pressed(Key::Enter))
                && !self.go_to_typed_line(ui.ctx())
            {
                response.request_focus();
            }
        });
    }

    fn header(&self, ui: &mut Ui, c: &Colors) {
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), HEADER_HEIGHT), Sense::hover());
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
            let (lines, commits) = (r.blame.lines.len(), r.commit_count);
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

    /// Returns the line under the pointer.
    fn body(
        &mut self,
        ui: &mut Ui,
        full: Rect,
        syntax_on: bool,
        c: &Colors,
        requests: &mut Vec<BlameRequest>,
    ) -> Option<usize> {
        let blame = &self.ready()?.blame;
        let (n, widest) = (blame.lines.len(), blame.widest);
        if n == 0 {
            message(ui, full, "The file is empty.", c.weak);
            return None;
        }
        let metrics = Metrics::new(ui.ctx(), n, self.repo.abbrev_len);
        let text_x = full.left() + metrics.gutter_w + metrics.numbers_w + CELL_PAD;
        let right = full.right() - OVERVIEW_WIDTH;
        let text_w = right - text_x;
        let content_w = widest as f32 * metrics.char_w + 24.0;
        let max_scroll_x = (content_w - text_w).max(0.0);
        let bottom = full.bottom()
            - if max_scroll_x > 0.0 {
                SCROLLBAR_HEIGHT
            } else {
                0.0
            };
        let area = Rect::from_min_max(full.min, pos2(right, bottom));

        self.take_scroll_requests(n, area.height(), &metrics, text_w);
        let mut scroll = ScrollArea::vertical()
            .auto_shrink(false)
            .id_salt(("blame", self.id));
        if let Some(offset) = self.scroll_to.take() {
            scroll = scroll.vertical_scroll_offset(offset.max(0.0));
        }
        scroll_sideways(ui, area, &mut self.scroll_x, max_scroll_x);

        let Load::Ready(ready) = &self.load else {
            return None;
        };
        let painter = LinePainter {
            ready,
            metrics: &metrics,
            text_x,
            syntax_on,
            selection: self.selection,
            chosen_commit: self.chosen_commit,
            find: &self.find,
            pointer: ui.input(|i| i.pointer.interact_pos()),
            pressed: ui.input(|i| i.pointer.primary_pressed()),
            scroll_x: self.scroll_x,
            abbrev_len: self.repo.abbrev_len,
            colors: c,
            blame_colors: blame_colors(ui),
        };
        let mut input = LineInput::default();
        let mut child = ui.new_child(UiBuilder::new().max_rect(area));
        child.set_clip_rect(area.intersect(ui.clip_rect()));
        child.spacing_mut().item_spacing = Vec2::ZERO;
        let out = scroll.show_rows(&mut child, metrics.row_h, n, |ui, range| {
            let top = range.start;
            for i in range {
                painter.paint(ui, i, top, &mut input);
            }
        });
        let view = ScrollView::of(&out);
        self.scroll_y = view.offset;
        self.top = input.first.unwrap_or(0);

        let strip = Rect::from_min_max(pos2(right, full.top()), full.max);
        self.overview(ui, strip, view, metrics.row_h, c);
        let id = egui::Id::new(("blame-overview", self.id));
        if let Some(offset) = overview_scroll(ui, id, strip, n, metrics.row_h, view) {
            self.scroll_to = Some(offset);
        }

        self.select(ui, &input, area, metrics.row_h);

        if max_scroll_x > 0.0 {
            let track = Rect::from_min_max(pos2(text_x, bottom), pos2(right, full.bottom()));
            let id = egui::Id::new(("blame-hbar", self.id));
            let visible = text_w / content_w;
            hscrollbar(ui, id, track, &mut self.scroll_x, visible, max_scroll_x, c);
        }

        if let Some(action) = input.action {
            self.act(action, ui.ctx(), requests);
        }
        input.hover.map(|h| h.line)
    }

    /// Sets `scroll_to` for what was asked since the last frame: a line asked for, which is
    /// chosen too, the place of a reload, a line gone to, or the place found gone to.
    fn take_scroll_requests(&mut self, lines: usize, height: f32, m: &Metrics, text_w: f32) {
        if let Some(line) = self.pending_line.take() {
            let line = line.min(lines - 1);
            self.choose_line(line);
            self.scroll_to = Some(third_of_the_way_down(line, height, m.row_h));
        } else if let Some(offset) = self.pending_scroll.take() {
            self.scroll_to = Some(offset);
        }
        if let Some(line) = self.reveal_line.take()
            && let Some(offset) = reveal(line.min(lines - 1), self.scroll_y, height, m.row_h)
        {
            self.scroll_to = Some(offset);
        }
        if let Some(found) = self.find.take_reveal() {
            if let Some(offset) = reveal(found.line, self.scroll_y, height, m.row_h) {
                self.scroll_to = Some(offset);
            }
            if let Some(x) = reveal_columns(self.scroll_x, &found.range, m.char_w, text_w) {
                self.scroll_x = x;
            }
        }
    }

    fn overview(&self, ui: &Ui, strip: Rect, view: ScrollView, row_h: f32, c: &Colors) {
        let Some(ready) = self.ready() else {
            return;
        };
        let blame = &ready.blame;
        let n = blame.lines.len();
        overview_background(ui, strip, c);
        let scale = overview_scale(strip, n, row_h);
        if let Some(commit) = self.chosen_commit {
            let mark = blame_colors(ui).chosen_commit_mark;
            let owned = |i: usize| blame.origins[blame.lines[i].origin].commit == commit;
            for run in runs(n, owned) {
                overview_mark(ui, strip, scale, run, mark);
            }
        }
        let found_lines = self.find.matches.iter().map(|m| m.line);
        overview_found(ui, strip, scale, found_lines, c);
        overview_view(ui, strip, view, c);
    }

    /// The chosen commit stays the one of the line the choosing started on. A right-click
    /// outside the chosen lines chooses its line.
    fn select(&mut self, ui: &Ui, input: &LineInput, area: Rect, row_h: f32) {
        let Some(n) = self.ready().map(|r| r.blame.lines.len()) else {
            return;
        };
        let (down, shift, double) = ui.input(|i| {
            (
                i.pointer.primary_down(),
                i.modifiers.shift,
                i.pointer
                    .button_double_clicked(egui::PointerButton::Primary),
            )
        });
        if let Some(press) = &input.press {
            match self.selection {
                Some(s) if shift => {
                    let whole_lines = s.whole_lines || press.on_gutter;
                    let head = (press.line, if whole_lines { 0 } else { press.col });
                    self.selection = Some(Selection {
                        head,
                        whole_lines,
                        ..s
                    });
                }
                _ => {
                    self.choose_line(press.line);
                    if !press.on_gutter {
                        let at = (press.line, press.col);
                        self.selection = Some(Selection {
                            anchor: at,
                            head: at,
                            whole_lines: false,
                        });
                    }
                }
            }
            self.dragging = true;
        }
        // Not only after a press: a quick double-click can end in the frame it started.
        if double
            && let Some(hover) = input.hover
            && self
                .selection
                .is_some_and(|s| !s.whole_lines && s.touches(hover.line))
            && let Some(word) = self
                .ready()
                .and_then(|r| word_at(&r.blame.lines[hover.line].text, hover.col))
        {
            self.selection = Some(Selection {
                anchor: (hover.line, word.start),
                head: (hover.line, word.end),
                whole_lines: false,
            });
            self.dragging = false;
        } else if input.press.is_none()
            && let Some(i) = input.secondary_click
            && !self.selection.is_some_and(|s| s.touches(i))
        {
            self.choose_line(i);
        }
        if !self.dragging {
            return;
        }
        if !down {
            self.dragging = false;
            return;
        }
        let Some(mut s) = self.selection else { return };
        if let Some(hover) = input.hover {
            s.head = (hover.line, if s.whole_lines { 0 } else { hover.col });
        } else if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
            // Past the top or bottom: take the line beyond those drawn, and scroll on.
            if p.y < area.top() {
                if let Some(first) = input.first {
                    s.head = (first.saturating_sub(1), 0);
                }
                self.scroll_to = Some(self.scroll_y - row_h);
            } else if p.y > area.bottom() {
                if let Some(last) = input.last {
                    let col = if s.whole_lines { 0 } else { LINE_END };
                    s.head = ((last + 1).min(n - 1), col);
                }
                self.scroll_to = Some(self.scroll_y + row_h);
            }
            ui.ctx().request_repaint();
        }
        self.selection = Some(s);
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
                if let Some(text) = self.lines_text() {
                    ctx.copy_text(text);
                }
            }
        }
    }

    fn history_pane(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        env: &Env,
        requests: &mut Vec<BlameRequest>,
    ) {
        let c = table_style::colors(ui);
        ui.painter().rect_filled(rect, 0.0, c.pane);
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .id_salt(("blame-history", self.id)),
        );
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        let ui = &mut child;
        let weak = ui.visuals().weak_text_color();
        let bc = blame_colors(ui);
        let ready = self.load.ready();
        let (history, status) = match &self.listing {
            Listing::Ready(h) => (Some(&**h), None),
            Listing::Failed(e) => (
                None,
                Some((format!("Could not list the file's history: {e}"), c.removed)),
            ),
            _ if matches!(self.load, Load::Failed(_)) => (
                None,
                Some((
                    "Not listed, as the file could not be blamed.".to_owned(),
                    weak,
                )),
            ),
            _ => (None, Some(("Listing…".to_owned(), weak))),
        };
        let no_graph = LogGraph::default();
        let table = CommitTable {
            id: Id::new(("blame-history", self.id)),
            rows: history.map_or(0, |h| h.history.rows.len()),
            graph: history.map_or(&no_graph, |h| &h.graph),
            abbrev_len: self.repo.abbrev_len,
            palette: &env.palette,
            select: Select::One,
            icons: false,
        };
        let (repo, spec) = (&*self.repo, &self.spec);
        let details = &mut self.details;
        let mut subject_tip = |ui: &mut Ui, i: usize| {
            let Some(row) = history.and_then(|h| h.history.rows.get(i)) else {
                return;
            };
            let ctx = ui.ctx().clone();
            let fetched = row
                .commit
                .and_then(|oid| details.get(&repo.path, oid, &ctx));
            message_tip(ui, &MessageTip::new(row, fetched), repo.abbrev_len);
        };
        let mut action = None;
        let clicks = table.show(
            ui,
            &c,
            &mut self.history_list,
            |i| match history {
                Some(h) => history_row(h, i, repo, env.graph, &bc),
                None => Row::default(),
            },
            |ui, i, _| {
                let (Some(h), Some(ready)) = (history, ready) else {
                    return;
                };
                let has_changes = row_changes(h, i, ready, repo, spec).is_some();
                if let Some(a) = history_menu(ui, i, &h.history.rows[i], has_changes) {
                    action = Some(a);
                }
            },
            Some(&mut subject_tip),
        );
        if let Some((text, color)) = status {
            let below =
                Rect::from_min_max(pos2(rect.left(), rect.top() + HEADING_HEIGHT), rect.max);
            message(ui, below, &text, color);
        }
        if let Some(i) = clicks.clicked {
            self.choose_row(i);
        }
        if let Some(i) = clicks.double_clicked {
            action = Some(RowAction::ShowChanges(i));
        }
        if let Some(action) = action {
            self.act_row(action, ui.ctx(), requests);
        }
    }

    fn act_row(
        &mut self,
        action: RowAction,
        ctx: &egui::Context,
        requests: &mut Vec<BlameRequest>,
    ) {
        let (Some(h), Some(ready)) = (self.history(), self.ready()) else {
            return;
        };
        let row = |i: usize| &h.history.rows[i];
        match action {
            RowAction::Blame(i) => {
                if let Some(oid) = row(i).commit {
                    let spec = BlameSpec {
                        rev: Rev::Commit(oid),
                        path: h.path(i, &self.spec).to_owned(),
                    };
                    // The commit's first line, at its place in that version.
                    let line = h.first_lines[i].map(|l| ready.blame.lines[l].orig_line as usize);
                    requests.push(BlameRequest::Blame(self.repo.clone(), spec, line));
                }
            }
            RowAction::ShowChanges(i) => {
                if let Some((spec, line)) = row_changes(h, i, ready, &self.repo, &self.spec) {
                    requests.push(BlameRequest::Diff(self.repo.clone(), spec, line));
                }
            }
            RowAction::ShowLog(i) => {
                if let Some(oid) = row(i).commit.filter(|_| row(i).snapshot.is_some()) {
                    requests.push(BlameRequest::Log(oid));
                }
            }
            RowAction::CopyHash(i) => {
                if let Some(oid) = row(i).commit {
                    ctx.copy_text(oid.to_hex());
                }
            }
        }
    }

    /// The commit of the line under the pointer, else the chosen commit.
    fn info_bar(&self, ui: &Ui, rect: Rect, hovered: Option<usize>, c: &Colors) {
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, ui.visuals().panel_fill);
        painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, c.line));
        let Some(ready) = self.ready() else { return };
        let blame = &ready.blame;
        let shown = match hovered.and_then(|i| blame.lines.get(i)) {
            Some(line) => Some(line.origin),
            None => self
                .chosen_commit
                .and_then(|commit| blame.origins.iter().position(|o| o.commit == commit)),
        };
        let small = FontId::proportional(12.5);
        let fmt = |color| TextFormat::simple(small.clone(), color);
        let mut job = LayoutJob::default();
        match (shown, self.chosen_commit) {
            (Some(ix), _) => {
                let origin = &blame.origins[ix];
                match origin.commit {
                    Some(oid) => {
                        job.append(
                            &oid.short(self.repo.abbrev_len),
                            0.0,
                            TextFormat::simple(hash_font(), c.weak),
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
                        TextFormat::simple(hash_font(), c.weak),
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

    fn show(
        &mut self,
        ctx: &egui::Context,
        settings: &mut Settings,
        window_theme: Option<egui::SystemTheme>,
        icon: &Arc<egui::IconData>,
    ) -> Vec<BlameRequest> {
        let id = self.viewport_id();
        let builder = viewport::builder(self.title(), self.initial_size, [560.0, 320.0], icon);
        self.viewport.before_show(ctx, id, Screen::Blame);
        let mut requests = Vec::new();
        ctx.show_viewport_immediate(id, builder, |ui, class| {
            let input = self.viewport.begin(ui, class, window_theme);
            self.poll(ui.ctx());
            if let Some(input) = input {
                input.keep_size(&mut settings.blame_window.size);
                let (reload, fetch) = ui.input_mut(keys::reload_and_fetch);
                if reload {
                    self.reload(ui.ctx());
                }
                if fetch {
                    requests.push(BlameRequest::Fetch(id));
                }
                self.handle_keys(ui);
                text_size::read_input(ui, &mut settings.text_size, true);
                if input.close_requested {
                    self.closed = true;
                }
            }
            if let Some(text) = self.copied(ui) {
                ui.ctx().copy_text(text);
            }
            let env = Env {
                palette: Palette::new(ui.visuals().dark_mode, &settings.branch_colors),
                graph: &settings.graph,
            };
            viewport::central_panel(ui).show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                requests.extend(self.contents(
                    ui,
                    &mut settings.blame_window,
                    &mut settings.syntax_colour,
                    &env,
                ));
            });
        });
        requests
    }
}

impl Drop for BlameWindow {
    fn drop(&mut self) {
        self.stop_listing.cancel();
        self.stop_colouring.cancel();
    }
}

impl FindInText for BlameWindow {
    type Place = find::Match;

    fn find(&self) -> &Find<find::Match> {
        &self.find
    }

    fn find_mut(&mut self) -> &mut Find<find::Match> {
        &mut self.find
    }

    fn find_id(&self) -> Id {
        Id::new(("blame-find", self.id))
    }

    /// Each a line and display columns of it.
    fn places(&self) -> Vec<find::Match> {
        let Some(ready) = self.ready() else {
            return Vec::new();
        };
        let lines = &ready.blame.lines;
        find::find(lines.iter().map(|l| &l.raw), &self.find.query)
            .into_iter()
            .map(|m| {
                let raw = &lines[m.line].raw;
                let columns = display_column(raw, m.range.start)..display_column(raw, m.range.end);
                find::Match {
                    line: m.line,
                    range: columns,
                }
            })
            .collect()
    }

    fn top(&self) -> usize {
        self.top
    }

    fn chosen_in_line(&self) -> Option<String> {
        let (start, end) = self.selection?.chosen_chars()?;
        (start.0 == end.0 && end.1 != LINE_END)
            .then(|| self.selected_text())
            .flatten()
    }

    /// The chosen characters, a line per line they span, ending with a newline if they run to
    /// the end of the last; otherwise the chosen lines ([`BlameWindow::lines_text`]).
    fn selected_text(&self) -> Option<String> {
        let s = self.selection?;
        let Some((_, end)) = s.chosen_chars() else {
            return self.lines_text();
        };
        let lines = &self.ready()?.blame.lines;
        let (first, last) = s.span();
        let mut parts = Vec::new();
        for (i, line) in lines.iter().enumerate().take(last + 1).skip(first) {
            parts.push(raw_slice(&line.raw, s.columns(i)?));
        }
        let mut text = parts.join("\n");
        if end.1 == LINE_END {
            text.push('\n');
        }
        Some(text)
    }
}

/// Paints the lines of a frame, noting what the pointer does over them.
struct LinePainter<'a> {
    ready: &'a Ready,
    metrics: &'a Metrics,
    text_x: f32,
    syntax_on: bool,
    selection: Option<Selection>,
    chosen_commit: Option<Option<Oid>>,
    find: &'a Find<find::Match>,
    pointer: Option<Pos2>,
    pressed: bool,
    scroll_x: f32,
    abbrev_len: usize,
    colors: &'a Colors,
    blame_colors: BlameColors,
}

#[derive(Default)]
struct LineInput {
    hover: Option<Hover>,
    press: Option<Press>,
    secondary_click: Option<usize>,
    action: Option<LineAction>,
    first: Option<usize>,
    last: Option<usize>,
}

/// The line and display column under the pointer.
#[derive(Clone, Copy)]
struct Hover {
    line: usize,
    col: usize,
}

/// The primary button went down on a line's text, or on its gutter or number.
struct Press {
    line: usize,
    col: usize,
    on_gutter: bool,
}

impl LinePainter<'_> {
    fn paint(&self, ui: &mut Ui, i: usize, top: usize, input: &mut LineInput) {
        input.first.get_or_insert(i);
        input.last = Some(i);
        let (c, bc, m) = (self.colors, &self.blame_colors, self.metrics);
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), m.row_h), Sense::click_and_drag());
        let blame = &self.ready.blame;
        let line = &blame.lines[i];
        let origin = &blame.origins[line.origin];
        let painter = ui.painter();
        let font = text_font();
        let gutter = Rect::from_x_y_ranges(rect.left()..=rect.left() + m.gutter_w, rect.y_range());
        let text_rect =
            Rect::from_x_y_ranges(rect.left() + m.gutter_w..=rect.right(), rect.y_range());
        painter.rect_filled(gutter, 0.0, bc.age(self.ready.ages[line.origin]));
        // Chosen characters are painted with the text.
        let whole = self
            .selection
            .is_some_and(|s| s.chosen_chars().is_none() && s.touches(i));
        if whole {
            painter.rect_filled(text_rect, 0.0, c.selection);
        } else if self.chosen_commit == Some(origin.commit) {
            painter.rect_filled(text_rect, 0.0, bc.chosen_commit);
        }
        let run_start = blame.starts_run(i);
        if run_start && i > 0 {
            painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, c.line));
        }
        let y = rect.center().y;
        // The first line in view names its commit too, when its run started above.
        if run_start || i == top {
            self.name_commit(ui, origin, &self.ready.dates[line.origin], rect.left(), y);
        }
        painter.vline(
            gutter.right() - 0.5,
            rect.y_range(),
            Stroke::new(1.0, c.line),
        );
        let number = painter.layout_no_wrap((i + 1).to_string(), font.clone(), c.weak);
        let number_x = gutter.right() + m.numbers_w - CELL_PAD - number.size().x;
        painter.galley(pos2(number_x, y - number.size().y / 2.0), number, c.weak);
        let spans = self.ready.syntax.as_ref().filter(|_| self.syntax_on);
        let galley = match spans.and_then(|s| s.get(i)) {
            Some(spans) if !spans.is_empty() => {
                let job = syntax::layout_job(
                    &line.text,
                    &[],
                    spans,
                    &font,
                    ui.visuals(),
                    Color32::TRANSPARENT,
                );
                painter.layout_job(job)
            }
            _ => painter.layout_no_wrap(line.text.clone(), font, c.text),
        };
        let text = TextAt {
            origin: pos2(self.text_x - self.scroll_x, y - galley.size().y / 2.0),
            galley,
        };
        let matches = &self.find.matches;
        let from = matches.partition_point(|m| m.line < i);
        let found = matches[from..]
            .iter()
            .enumerate()
            .take_while(|(_, m)| m.line == i)
            .map(|(k, m)| (m.range.clone(), self.find.current == Some(from + k)));
        let clip = Rect::from_x_y_ranges(self.text_x..=rect.right(), rect.y_range())
            .intersect(ui.clip_rect());
        let row = TextRow {
            y_range: rect.y_range(),
            row_h: m.row_h,
            colors: c,
        };
        let chosen = self.selection.and_then(|s| s.columns(i));
        paint_text(&painter.with_clip_rect(clip), &text, &row, found, chosen);

        if let Some(p) = self.pointer.filter(|p| rect.y_range().contains(p.y)) {
            let col = text.column_at(p);
            input.hover = Some(Hover { line: i, col });
            let on_gutter = p.x < self.text_x - CELL_PAD;
            if self.pressed && response.hovered() {
                input.press = Some(Press {
                    line: i,
                    col,
                    on_gutter,
                });
            }
            if !on_gutter && response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
            }
        }
        if response.secondary_clicked() {
            input.secondary_click = Some(i);
        }
        let over_gutter = response.hover_pos().is_some_and(|p| gutter.contains(p));
        let response = if over_gutter {
            let date = &self.ready.dates[line.origin];
            response.on_hover_ui(|ui| origin_tip(ui, origin, date, self.abbrev_len))
        } else {
            response
        };
        egui::Popup::context_menu(&response)
            .style(crate::menu::style)
            .show(|ui| {
                crate::menu::fit_window(ui, |ui| {
                    crate::usage::menu(ui.ctx(), crate::usage::Menu::Blame);
                    ui.set_min_width(crate::menu::MIN_WIDTH);
                    let several_lines = self.selection.is_some_and(|s| {
                        let (first, last) = s.span();
                        first != last && s.touches(i)
                    });
                    let in_snapshot = self.ready.in_snapshot[line.origin];
                    if let Some(a) = line_menu(ui, i, origin, in_snapshot, several_lines) {
                        input.action = Some(a);
                    }
                });
            });
    }

    /// The commit's hash, author and date in the gutter, or that the line isn't committed.
    fn name_commit(&self, ui: &Ui, origin: &Origin, date: &str, left: f32, y: f32) {
        let (c, m) = (self.colors, self.metrics);
        let put = |g: Arc<egui::Galley>, x: f32| {
            ui.painter()
                .galley(pos2(x, y - g.size().y / 2.0), g, c.text);
        };
        let small = FontId::proportional(12.5);
        let mut x = left + CELL_PAD;
        match origin.commit {
            Some(oid) => {
                let hash = oid.short(self.abbrev_len);
                put(cell(ui, &hash, hash_font(), c.weak, m.hash_w), x);
                x += m.hash_w - CELL_PAD;
                let author_w = AUTHOR_WIDTH - CELL_PAD;
                put(cell(ui, &origin.author, small.clone(), c.text, author_w), x);
                x += AUTHOR_WIDTH;
                let day = date.split(' ').next().unwrap_or("");
                put(cell(ui, day, small, c.weak, DATE_WIDTH - CELL_PAD), x);
            }
            None => {
                let text = "Not committed yet";
                let width = m.gutter_w - 2.0 * CELL_PAD;
                put(cell(ui, text, small, c.note, width), x);
            }
        }
    }
}

/// A menu item, disabled with `why` unless `enabled`. Clicked, it takes `action` and closes the
/// menu.
fn menu_item<A>(
    ui: &mut Ui,
    taken: &mut Option<A>,
    enabled: bool,
    text: &str,
    why: &str,
    action: A,
) {
    let response = ui
        .add_enabled(enabled, egui::Button::new(text))
        .on_disabled_hover_text(why);
    if response.clicked() {
        *taken = Some(action);
        ui.close();
    }
}

fn line_menu(
    ui: &mut Ui,
    i: usize,
    origin: &Origin,
    in_snapshot: bool,
    several_lines: bool,
) -> Option<LineAction> {
    let mut action = None;
    let (previous_why, changes_why) = if origin.boundary {
        let why = "The history before this commit isn't in this repository";
        (why, why)
    } else {
        ("This commit added the file", "")
    };
    menu_item(
        ui,
        &mut action,
        origin.previous_blame().is_some(),
        "Blame previous revision",
        previous_why,
        LineAction::BlamePrevious(i),
    );
    menu_item(
        ui,
        &mut action,
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
    let show_log = LineAction::ShowLog(i);
    menu_item(ui, &mut action, in_snapshot, "Show log", log_why, show_log);
    crate::menu::separator(ui);
    menu_item(
        ui,
        &mut action,
        origin.commit.is_some(),
        "Copy hash",
        "Not committed yet",
        LineAction::CopyHash(i),
    );
    let copy = if several_lines {
        "Copy lines"
    } else {
        "Copy line"
    };
    menu_item(ui, &mut action, true, copy, "", LineAction::CopyLines);
    action
}

fn history_row<'a>(
    h: &'a History,
    i: usize,
    repo: &'a Repo,
    graph: &GraphOptions,
    bc: &BlameColors,
) -> Row<'a> {
    let row = &h.history.rows[i];
    let (hash, subject) = match row.commit {
        Some(oid) => (oid.short(repo.abbrev_len), row.subject.as_str()),
        None => (String::new(), "Working tree changes"),
    };
    Row {
        hash,
        hash_fill: h.ages[i].filter(|_| row.owns_lines).map(|age| bc.age(age)),
        refs: table_style::badges(repo, &h.refs[i], row.snapshot, graph),
        outlined_label: (row.commit.is_some() && row.snapshot.is_none())
            .then_some("not in the graph"),
        subject,
        note: match &row.source {
            Source::Blame { paths } => Some(format!("(as {})", paths.join(", "))),
            _ => None,
        },
        author: &row.author_name,
        author_email: &row.author_email,
        date: &row.author_date,
        greyed: !row.owns_lines,
        ..Row::default()
    }
}

/// Row `i`'s change to the file, and the line (from 0, in the new version) to show: for a
/// commit that owns lines, the change its first line's menu shows; otherwise the file at the
/// commit against its first parent (the working tree against `HEAD`). `None` where the
/// history before the commit isn't in the repository.
fn row_changes(
    h: &History,
    i: usize,
    ready: &Ready,
    repo: &Repo,
    spec: &BlameSpec,
) -> Option<(FileDiffSpec, usize)> {
    if let Some(first) = h.first_lines[i] {
        let line = &ready.blame.lines[first];
        let changes = ready.blame.origins[line.origin].changes()?;
        return Some((changes, line.orig_line as usize));
    }
    let row = &h.history.rows[i];
    let old_path = h.history.path.clone();
    let (old, new) = match row.commit {
        None => {
            let head = repo.head_commit().map(|ix| repo.commit(ix).oid)?;
            (
                Some(Rev::Commit(head)),
                (Rev::WorkingTree, spec.path.clone()),
            )
        }
        Some(oid) => {
            let commit = row.snapshot.map(|ix| repo.commit(ix));
            if commit.is_some_and(|c| c.truncated) {
                return None;
            }
            // No parents in the file's history: the commit added the file. Otherwise its first
            // parent, or where the snapshot doesn't have it, the first parent in the file's
            // history, which has the same version of the file.
            let parent = (!row.parents.is_empty()).then(|| {
                commit
                    .and_then(|c| c.parents.first())
                    .map(|&p| repo.commit(p).oid)
                    .unwrap_or(row.parents[0])
            });
            (
                parent.map(Rev::Commit),
                (Rev::Commit(oid), old_path.clone()),
            )
        }
    };
    let status = match &old {
        None => FileStatus::Added,
        Some(_) if old_path != new.1 => FileStatus::Renamed,
        Some(_) => FileStatus::Modified,
    };
    // Blamed files are regular text files; their exact modes don't change the diff.
    const REGULAR_FILE_MODE: u32 = 0o100644;
    let spec = FileDiffSpec {
        modes: [
            if old.is_some() { REGULAR_FILE_MODE } else { 0 },
            REGULAR_FILE_MODE,
        ],
        old: old.map(|rev| Version {
            rev,
            path: old_path,
        }),
        new: Some(Version {
            rev: new.0,
            path: new.1,
        }),
        status,
        binary: false,
    };
    Some((spec, 0))
}

fn history_menu(ui: &mut Ui, i: usize, row: &HistoryRow, has_changes: bool) -> Option<RowAction> {
    let mut action = None;
    let show_changes = RowAction::ShowChanges(i);
    if row.commit.is_none() {
        let why = "Nothing is committed yet to compare with";
        menu_item(
            ui,
            &mut action,
            has_changes,
            "Show changes",
            why,
            show_changes,
        );
        return action;
    }
    let blame = RowAction::Blame(i);
    menu_item(ui, &mut action, true, "Blame this revision", "", blame);
    let why = "The history before this commit isn't in this repository";
    menu_item(
        ui,
        &mut action,
        has_changes,
        "Show changes",
        why,
        show_changes,
    );
    let why = "Not in the graph: the commit isn't among the loaded history";
    let in_snapshot = row.snapshot.is_some();
    let show_log = RowAction::ShowLog(i);
    menu_item(ui, &mut action, in_snapshot, "Show log", why, show_log);
    crate::menu::separator(ui);
    menu_item(
        ui,
        &mut action,
        true,
        "Copy hash",
        "",
        RowAction::CopyHash(i),
    );
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

fn hash_font() -> FontId {
    FontId::monospace(12.0)
}

struct Metrics {
    row_h: f32,
    char_w: f32,
    hash_w: f32,
    gutter_w: f32,
    numbers_w: f32,
}

impl Metrics {
    fn new(ctx: &egui::Context, lines: usize, abbrev_len: usize) -> Metrics {
        let TextMetrics { row_h, char_w } = TextMetrics::new(ctx);
        let hash_char_w = ctx.fonts_mut(|f| f.glyph_width(&hash_font(), '0'));
        let hash_w = hash_char_w * abbrev_len as f32 + 2.0 * CELL_PAD;
        Metrics {
            row_h,
            char_w,
            hash_w,
            gutter_w: hash_w + AUTHOR_WIDTH + DATE_WIDTH,
            numbers_w: line_numbers_width(lines, char_w) + 2.0 * CELL_PAD,
        }
    }
}

fn moves_glyph(moves: Moves) -> glyphs::Glyph {
    match moves {
        Moves::Off => glyphs::MOVES_OFF,
        Moves::WithinFile => glyphs::MOVES_WITHIN_FILE,
        Moves::AcrossFiles => glyphs::MOVES_ACROSS_FILES,
    }
}

fn origin_tip(ui: &mut Ui, origin: &Origin, date: &str, abbrev_len: usize) {
    ui.set_max_width(420.0);
    let Some(oid) = origin.commit else {
        ui.label("Not committed yet: changed in the working tree.");
        return;
    };
    ui.strong(&origin.summary);
    ui.label(format!("{} <{}>", origin.author, origin.author_email));
    ui.weak(format!("{date}   {}", oid.short(abbrev_len.max(12))));
}

/// The tooltip over a row's subject: the commit's whole message, author and date, as the log
/// window's details show them; until git has given the message, or if it failed, the subject.
#[derive(Debug, PartialEq, Eq)]
struct MessageTip<'a> {
    /// `None` for the working tree changes.
    commit: Option<Oid>,
    author: String,
    date: &'a str,
    message: String,
    truncated: bool,
    error: Option<&'a str>,
}

impl<'a> MessageTip<'a> {
    /// `fetched` is `None` while git fetches the message.
    fn new(row: &'a HistoryRow, fetched: Option<&'a Result<CommitDetails, String>>) -> Self {
        let (message, date, error) = match fetched {
            Some(Ok(d)) => (d.message.as_str(), d.author_date.as_str(), None),
            Some(Err(e)) => (
                row.subject.as_str(),
                row.author_date.as_str(),
                Some(e.as_str()),
            ),
            None => (row.subject.as_str(), row.author_date.as_str(), None),
        };
        let lines: Vec<&str> = message.trim_end().lines().collect();
        let mut shown = lines[..lines.len().min(MESSAGE_TIP_MAX_LINES)].join("\n");
        let mut truncated = lines.len() > MESSAGE_TIP_MAX_LINES;
        if let Some((at, _)) = shown.char_indices().nth(MESSAGE_TIP_MAX_CHARS) {
            shown.truncate(at);
            truncated = true;
        }
        MessageTip {
            commit: row.commit,
            author: format!("{} <{}>", row.author_name, row.author_email),
            date,
            message: shown,
            truncated,
            error,
        }
    }
}

fn message_tip(ui: &mut Ui, tip: &MessageTip, abbrev_len: usize) {
    let Some(oid) = tip.commit else {
        ui.label("Changes in the working tree, not committed yet.");
        return;
    };
    ui.set_max_width(560.0);
    ui.label(&tip.author);
    ui.weak(format!("{}   {}", tip.date, oid.short(abbrev_len.max(12))));
    ui.add_space(6.0);
    let font = FontId::monospace(12.5);
    let (subject, body) = tip.message.split_once('\n').unwrap_or((&tip.message, ""));
    let strong = ui.visuals().strong_text_color();
    ui.add(egui::Label::new(RichText::new(subject).font(font.clone()).color(strong)).wrap());
    let body = body.trim_start_matches('\n');
    if !body.is_empty() {
        ui.add_space(8.0);
        ui.add(egui::Label::new(RichText::new(body).font(font.clone())).wrap());
    }
    if tip.truncated {
        ui.weak("…");
    }
    if let Some(e) = tip.error {
        ui.add_space(6.0);
        ui.colored_label(ui.visuals().error_fg_color, e);
    }
}

struct BlameColors {
    oldest: Color32,
    newest: Color32,
    chosen_commit: Color32,
    chosen_commit_mark: Color32,
}

impl BlameColors {
    /// The gutter's colour for `age`, from 0 for the oldest commit to 1 for the newest.
    fn age(&self, age: f32) -> Color32 {
        let t = age.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        Color32::from_rgb(
            mix(self.oldest.r(), self.newest.r()),
            mix(self.oldest.g(), self.newest.g()),
            mix(self.oldest.b(), self.newest.b()),
        )
    }
}

/// From plain to amber, as TortoiseGitBlame shades from white to yellow.
fn blame_colors(ui: &Ui) -> BlameColors {
    if ui.visuals().dark_mode {
        BlameColors {
            oldest: Color32::from_gray(26),
            newest: Color32::from_rgb(0x5c, 0x45, 0x12),
            chosen_commit: Color32::from_rgba_unmultiplied(0x35, 0x84, 0xe4, 40),
            chosen_commit_mark: Color32::from_rgb(0x62, 0xa0, 0xea),
        }
    } else {
        BlameColors {
            oldest: Color32::from_gray(250),
            newest: Color32::from_rgb(0xff, 0xd9, 0x80),
            chosen_commit: Color32::from_rgba_unmultiplied(0x35, 0x84, 0xe4, 26),
            chosen_commit_mark: Color32::from_rgb(0x1c, 0x71, 0xd8),
        }
    }
}

impl ParterreApp {
    pub(super) fn blame_windows(&mut self, ctx: &egui::Context) {
        let mut requests = Vec::new();
        for window in &mut self.blames.windows {
            requests.extend(window.show(
                ctx,
                &mut self.settings,
                self.window_theme,
                &self.window_icon,
            ));
        }
        self.blames.windows.retain(|w| !w.closed);
        for request in requests {
            match request {
                BlameRequest::Blame(repo, spec, line) => {
                    self.open_blame(repo, spec, line, ctx);
                }
                BlameRequest::Diff(repo, spec, line) => {
                    let settings = &self.settings.diff_window;
                    self.diffs.open_at(repo, spec, Some(line), settings, ctx);
                }
                BlameRequest::Fetch(window) => self.fetch(ctx, window),
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

    /// With `line` (from 0) chosen.
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

    /// A script's `open blame:REV:PATH[:LINE]`: `REV` is a ref, a hash prefix or
    /// `WORKING_TREE`, and `LINE` counts from 1.
    pub(super) fn open_named_blame(
        &mut self,
        repo: &Arc<Repo>,
        spec: &str,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let (rev, path) = spec.split_once(':').ok_or("expected COMMIT:PATH")?;
        let (path, line) = match path.rsplit_once(':').map(|(p, l)| (p, l.parse::<usize>())) {
            Some((p, Ok(line))) => (p, Some(line)),
            _ => (path, None),
        };
        let rev = if rev == "WORKING_TREE" {
            Rev::WorkingTree
        } else {
            let ix = repo
                .resolve(rev)
                .ok_or_else(|| format!("no commit named {rev}"))?;
            Rev::Commit(repo.commit(ix).oid)
        };
        let spec = BlameSpec {
            rev,
            path: path.to_owned(),
        };
        self.open_blame(repo.clone(), spec, line.map(|l| l.saturating_sub(1)), ctx);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::text_view::TOOLBAR_HEIGHT;
    use super::*;
    use parterre_core::file_history::LogCommit;
    use parterre_core::repo::Head;

    const A: &str = "c4275f7dbe7e820e3bb9a21c7d1cc1317657f2d4";
    const B: &str = "8e09b4551acb469f9df4ce58895b77e2e7e4190e";
    /// Between A and B, it changed the file, but none of its lines remain.
    const GONE: &str = "0123456789abcdef0123456789abcdef01234567";

    fn oid(hex: &str) -> Oid {
        Oid::from_hex(hex).unwrap()
    }

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
        let mut w = BlameWindow::new(1, repo.clone(), spec, &settings, &Engine::InProcess);
        w.load = Load::Ready(Box::new(Ready::new(
            sample(),
            &repo,
            "",
            &Engine::InProcess,
            &Cancel::new(),
        )));
        w
    }

    /// A commit as git lists it for the file.
    fn logged(hex: &str, time: i64, parents: &[&str]) -> LogCommit {
        LogCommit {
            oid: oid(hex),
            parents: parents.iter().map(|p| oid(p)).collect(),
            author_name: "A B".into(),
            author_email: "a@b".into(),
            author_time: time,
            author_date: String::new(),
            commit_time: time,
            subject: format!("s{time}"),
        }
    }

    /// [`window`] with its history listed: B, GONE (which owns no lines), A.
    fn listed_window() -> BlameWindow {
        let mut w = window();
        w.listing = Listing::Listed(FileLog {
            path: "a.txt".into(),
            working_tree_changed: false,
            commits: vec![
                logged(B, 200, &[GONE]),
                logged(GONE, 150, &[A]),
                logged(A, 100, &[]),
            ],
        });
        w.settle();
        w
    }

    fn frame(
        ctx: &egui::Context,
        w: &mut BlameWindow,
        events: Vec<egui::Event>,
    ) -> Vec<BlameRequest> {
        frame_with(ctx, w, &mut BlameWindowSettings::default(), events)
    }

    fn frame_with(
        ctx: &egui::Context,
        w: &mut BlameWindow,
        settings: &mut BlameWindowSettings,
        events: Vec<egui::Event>,
    ) -> Vec<BlameRequest> {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 700.0))),
            events,
            ..Default::default()
        };
        let graph = GraphOptions::default();
        let mut requests = Vec::new();
        ctx.run_ui(input, |ui| {
            let env = Env {
                palette: Palette::new(false, &[]),
                graph: &graph,
            };
            w.handle_keys(ui);
            requests = w.contents(ui, settings, &mut true, &env);
        })
        .textures_delta
        .clear();
        requests
    }

    /// The middle of row `i` of the history pane (in the 1000×700 window of [`frame`]).
    fn row_at(i: usize) -> egui::Pos2 {
        let top = 700.0 - INFO_BAR_HEIGHT - BlameWindowSettings::default().history_height;
        pos2(500.0, top + HEADING_HEIGHT + (i as f32 + 0.5) * ROW_HEIGHT)
    }

    fn ctrl(key: Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }
    }

    fn key(key: Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    /// The middle of line `i`'s text, past the end of the sample's lines.
    fn at(ctx: &egui::Context, i: usize) -> egui::Pos2 {
        let row_h = TextMetrics::new(ctx).row_h;
        pos2(
            700.0,
            TOOLBAR_HEIGHT + HEADER_HEIGHT + (i as f32 + 0.5) * row_h,
        )
    }

    /// Where column `col` of line `i`'s text starts, in `w`.
    fn char_at(ctx: &egui::Context, w: &BlameWindow, i: usize, col: usize) -> egui::Pos2 {
        let n = w.ready().unwrap().blame.lines.len();
        let m = Metrics::new(ctx, n, w.repo.abbrev_len);
        let x = m.gutter_w + m.numbers_w + CELL_PAD + col as f32 * m.char_w;
        pos2(x, at(ctx, i).y)
    }

    /// The middle of line `i`'s number, in `w`.
    fn number_at(ctx: &egui::Context, w: &BlameWindow, i: usize) -> egui::Pos2 {
        let n = w.ready().unwrap().blame.lines.len();
        let m = Metrics::new(ctx, n, w.repo.abbrev_len);
        pos2(m.gutter_w + m.numbers_w / 2.0, at(ctx, i).y)
    }

    fn span(w: &BlameWindow) -> Option<(usize, usize)> {
        w.selection.map(|s| s.span())
    }

    /// A press at `from`, a move to `to` and a release there, with `modifiers` held.
    fn drag(
        ctx: &egui::Context,
        w: &mut BlameWindow,
        from: egui::Pos2,
        to: egui::Pos2,
        modifiers: Modifiers,
    ) {
        let changed = egui::Event::ModifiersChanged;
        let press = egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers,
        };
        let events = vec![changed(modifiers), egui::Event::PointerMoved(from), press];
        frame(ctx, w, events);
        frame(ctx, w, vec![egui::Event::PointerMoved(to)]);
        frame(ctx, w, vec![egui::Event::PointerMoved(to)]);
        let release = vec![button(to, false, false), changed(Modifiers::NONE)];
        frame(ctx, w, release);
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
        assert_eq!(span(&w), Some((2, 2)));
        assert_eq!(w.chosen_commit, Some(Some(Oid::from_hex(A).unwrap())));

        // A drag over the line numbers chooses whole lines.
        let (from, to) = (number_at(&ctx, &w, 1), number_at(&ctx, &w, 3));
        drag(&ctx, &mut w, from, to, Modifiers::NONE);
        assert_eq!(span(&w), Some((1, 3)));
        assert_eq!(w.selection.and_then(|s| s.chosen_chars()), None);
        // The drag started on B's line: B stays chosen over A's lines.
        assert_eq!(w.chosen_commit, Some(Some(Oid::from_hex(B).unwrap())));
        // Copied as in the file: tabs kept, a newline after each line.
        assert_eq!(w.selected_text().as_deref(), Some("two\tx\nthree\nfour\n"));

        // So does one over the commit column.
        let (from, to) = (pos2(20.0, at(&ctx, 0).y), pos2(20.0, at(&ctx, 2).y));
        drag(&ctx, &mut w, from, to, Modifiers::NONE);
        assert_eq!(span(&w), Some((0, 2)));
        assert_eq!(w.selected_text().as_deref(), Some("one\ntwo\tx\nthree\n"));
        assert_eq!(w.chosen_commit, Some(Some(Oid::from_hex(A).unwrap())));
    }

    #[test]
    fn a_drag_over_the_text_chooses_characters_and_copies_just_those() {
        let ctx = egui::Context::default();
        let mut w = window();
        frame(&ctx, &mut w, Vec::new());
        // From "tw|o    x" (the tab shown as spaces) to "thr|ee".
        let (from, to) = (char_at(&ctx, &w, 1, 2), char_at(&ctx, &w, 2, 3));
        drag(&ctx, &mut w, from, to, Modifiers::NONE);
        assert_eq!(
            w.selection.and_then(|s| s.chosen_chars()),
            Some(((1, 2), (2, 3)))
        );
        // Copied as in the file, the tab kept.
        assert_eq!(w.selected_text().as_deref(), Some("o\tx\nthr"));
        // The chosen commit is that of the line the drag started on.
        assert_eq!(w.chosen_commit, Some(Some(oid(B))));
        // The line menu's Copy lines still copies them whole.
        let mut requests = Vec::new();
        w.act(LineAction::CopyLines, &ctx, &mut requests);
        assert_eq!(w.lines_text().as_deref(), Some("two\tx\nthree\n"));

        // Backwards, within a line; then Shift+click on the text extends it.
        let (from, to) = (char_at(&ctx, &w, 3, 3), char_at(&ctx, &w, 3, 1));
        drag(&ctx, &mut w, from, to, Modifiers::NONE);
        assert_eq!(w.selected_text().as_deref(), Some("ou"));
        assert_eq!(w.chosen_commit, Some(Some(oid(A))));
        let p = char_at(&ctx, &w, 2, 0);
        drag(&ctx, &mut w, p, p, Modifiers::SHIFT);
        assert_eq!(w.selected_text().as_deref(), Some("three\nfou"));
        assert_eq!(w.chosen_commit, Some(Some(oid(A))));

        // Past the last character, a line break is included.
        let (from, to) = (char_at(&ctx, &w, 2, 3), at(&ctx, 2));
        drag(&ctx, &mut w, from, to, Modifiers::NONE);
        assert_eq!(w.selected_text().as_deref(), Some("ee"));
    }

    #[test]
    fn a_click_on_the_text_chooses_its_line_whole_and_a_double_click_a_word() {
        let ctx = egui::Context::default();
        let mut w = window();
        frame(&ctx, &mut w, Vec::new());
        let p = char_at(&ctx, &w, 2, 2);
        drag(&ctx, &mut w, p, p, Modifiers::NONE);
        assert_eq!(span(&w), Some((2, 2)));
        assert_eq!(w.selection.and_then(|s| s.chosen_chars()), None);
        // Ctrl+C copies the line, as before.
        assert_eq!(w.selected_text().as_deref(), Some("three\n"));

        // A second later (not to count as a triple click), a double-click takes the word.
        for _ in 0..60 {
            frame(&ctx, &mut w, Vec::new());
        }
        let p = char_at(&ctx, &w, 1, 1);
        for _ in 0..2 {
            let press = vec![egui::Event::PointerMoved(p), button(p, true, false)];
            frame(&ctx, &mut w, press);
            frame(&ctx, &mut w, vec![button(p, false, false)]);
        }
        assert_eq!(w.selected_text().as_deref(), Some("two"));
        assert_eq!(w.chosen_commit, Some(Some(oid(B))));

        // Also when each click comes in a single frame.
        for _ in 0..60 {
            frame(&ctx, &mut w, Vec::new());
        }
        let p = char_at(&ctx, &w, 2, 1);
        for _ in 0..2 {
            let clicked = vec![
                egui::Event::PointerMoved(p),
                button(p, true, false),
                button(p, false, false),
            ];
            frame(&ctx, &mut w, clicked);
        }
        assert_eq!(w.selected_text().as_deref(), Some("three"));

        // Ctrl+A chooses every line, whole, keeping the chosen commit.
        frame(&ctx, &mut w, vec![ctrl(Key::A)]);
        assert_eq!(span(&w), Some((0, 3)));
        assert_eq!(
            w.selected_text().as_deref(),
            Some("one\ntwo\tx\nthree\nfour\n")
        );
        assert_eq!(w.chosen_commit, Some(Some(oid(A))));
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
        assert_eq!(w.chosen_commit, Some(Some(Oid::from_hex(B).unwrap())));
        // Shift+click on a line number chooses whole lines, from the text's line too.
        let p = number_at(&ctx, &w, 3);
        drag(&ctx, &mut w, p, p, Modifiers::SHIFT);
        assert_eq!(span(&w), Some((1, 3)));
        assert_eq!(w.selection.and_then(|s| s.chosen_chars()), None);
        assert_eq!(w.chosen_commit, Some(Some(Oid::from_hex(B).unwrap())));

        // A right-click inside the chosen lines keeps them; outside, it chooses its line.
        let p = at(&ctx, 2);
        frame(
            &ctx,
            &mut w,
            vec![egui::Event::PointerMoved(p), button(p, true, true)],
        );
        frame(&ctx, &mut w, vec![button(p, false, true)]);
        assert_eq!(span(&w), Some((1, 3)));
        assert_eq!(w.chosen_commit, Some(Some(Oid::from_hex(B).unwrap())));
        let p = at(&ctx, 0);
        frame(
            &ctx,
            &mut w,
            vec![egui::Event::PointerMoved(p), button(p, true, true)],
        );
        frame(&ctx, &mut w, vec![button(p, false, true)]);
        assert_eq!(span(&w), Some((0, 0)));
        assert_eq!(w.chosen_commit, Some(Some(Oid::from_hex(A).unwrap())));
    }

    #[test]
    fn choosing_a_commit_chooses_its_first_line_without_scrolling() {
        let mut w = window();
        w.choose_commit(Some(Oid::from_hex(B).unwrap()));
        assert_eq!(span(&w), Some((1, 1)));
        assert_eq!(w.chosen_commit, Some(Some(Oid::from_hex(B).unwrap())));
        assert_eq!(w.scroll_to, None);

        // A commit that owns no lines here is chosen, with no line.
        let gone = Oid::from_hex("0123456789abcdef0123456789abcdef01234567").unwrap();
        w.choose_commit(Some(gone));
        assert_eq!(span(&w), None);
        assert_eq!(w.chosen_commit, Some(Some(gone)));
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
        let [BlameRequest::Blame(_, spec, Some(1))] = requests.as_slice() else {
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
        assert_eq!(span(&w), Some((1, 1)));
    }

    #[test]
    fn opening_a_blame_already_open_brings_it_forward_with_the_line_chosen() {
        let ctx = egui::Context::default();
        let w = window();
        let (repo, spec) = (w.repo.clone(), w.spec.clone());
        let mut windows = BlameWindows {
            windows: vec![w],
            last_id: 1,
            engine: Engine::InProcess,
        };
        let settings = BlameWindowSettings::default();
        windows.open(repo, spec, Some(2), &settings, &ctx);
        let [w] = windows.windows.as_slice() else {
            panic!("expected one window");
        };
        assert!(w.viewport.bring_to_front);
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
        w.scroll_y = 40.0;
        w.reload(&ctx);
        assert!(matches!(w.load, Load::Loading(_)));
        assert_eq!(w.pending_scroll, Some(40.0));
    }

    #[test]
    fn a_reload_keeps_the_chosen_commit_while_it_is_listed() {
        let repo = window().repo.clone();
        let mut w = listed_window();
        w.choose_commit(Some(oid(GONE)));
        let log = |commits| {
            Listing::Listed(FileLog {
                path: "a.txt".into(),
                working_tree_changed: false,
                commits,
            })
        };
        w.loaded(Ready::new(
            sample(),
            &repo,
            "",
            &Engine::InProcess,
            &Cancel::new(),
        ));
        w.listing = log(vec![logged(B, 200, &[GONE]), logged(GONE, 150, &[A])]);
        w.settle();
        // Listed, though it owns no lines.
        assert_eq!(w.chosen_commit, Some(Some(oid(GONE))));
        assert_eq!(w.history_list.selected, Some(1));

        // Gone from the history, and lines 3 and 4 from the file.
        let out = [
            entry(A, 1, 1, 100, "", "one"),
            entry(A, 2, 2, 100, "", "two"),
        ]
        .concat();
        w.selection = Some(Selection::of_lines(1, 3));
        w.loaded(Ready::new(
            Blame::parse(out.as_bytes()).unwrap(),
            &repo,
            "",
            &Engine::InProcess,
            &Cancel::new(),
        ));
        w.listing = log(vec![logged(A, 100, &[])]);
        w.settle();
        assert_eq!(w.chosen_commit, None);
        assert_eq!(span(&w), None);
        assert_eq!(w.history_list.selected, None);

        // Without a history, the chosen commit stays while it owns lines.
        w.choose_line(0);
        w.listing = Listing::Failed("no".into());
        w.settle();
        assert_eq!(w.chosen_commit, Some(Some(oid(A))));
        w.chosen_commit = Some(Some(oid(B)));
        w.settle();
        assert_eq!(w.chosen_commit, None);
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
        w.show_history = false;
        // 300 lines from A, but line 250 from B.
        let out: String = (1..=300)
            .map(|i| match i {
                251 => entry(B, i, i, 200, &format!("previous {A} a.txt\n"), "b"),
                _ => entry(A, i, i, 100, "", "a"),
            })
            .collect();
        let repo = w.repo.clone();
        w.loaded(Ready::new(
            Blame::parse(out.as_bytes()).unwrap(),
            &repo,
            "",
            &Engine::InProcess,
            &Cancel::new(),
        ));
        frame(&ctx, &mut w, Vec::new());
        w.choose_commit(Some(Oid::from_hex(B).unwrap()));
        assert_eq!(span(&w), Some((250, 250)));
        frame(&ctx, &mut w, Vec::new());
        frame(&ctx, &mut w, Vec::new());
        assert_eq!(w.scroll_y, 0.0);

        // A click on the strip at B's mark puts line 250 in the middle.
        let row_h = TextMetrics::new(&ctx).row_h;
        let (top, bottom) = (TOOLBAR_HEIGHT + HEADER_HEIGHT, 700.0 - INFO_BAR_HEIGHT);
        let scale = ((bottom - top) / 300.0).min(row_h);
        let p = pos2(1000.0 - OVERVIEW_WIDTH / 2.0, top + 250.5 * scale);
        frame(
            &ctx,
            &mut w,
            vec![egui::Event::PointerMoved(p), button(p, true, false)],
        );
        frame(&ctx, &mut w, vec![button(p, false, false)]);
        frame(&ctx, &mut w, Vec::new());
        let middle = (w.scroll_y + (bottom - top) / 2.0) / row_h;
        assert!((middle - 250.5).abs() < 2.0, "line {middle} in the middle");
        // The chosen commit and line stay.
        assert_eq!(span(&w), Some((250, 250)));
        assert_eq!(w.chosen_commit, Some(Some(Oid::from_hex(B).unwrap())));
    }

    #[test]
    fn the_gutter_shades_from_old_to_new() {
        let c = BlameColors {
            oldest: Color32::from_rgb(0, 0, 0),
            newest: Color32::from_rgb(200, 100, 50),
            chosen_commit: Color32::TRANSPARENT,
            chosen_commit_mark: Color32::TRANSPARENT,
        };
        assert_eq!(c.age(0.0), c.oldest);
        assert_eq!(c.age(1.0), c.newest);
        assert_eq!(c.age(0.5), Color32::from_rgb(100, 50, 25));
    }

    #[test]
    fn clicking_a_row_chooses_its_commit_and_first_line_without_scrolling() {
        let ctx = egui::Context::default();
        let mut w = listed_window();
        frame(&ctx, &mut w, Vec::new());
        let p = row_at(2);
        frame(
            &ctx,
            &mut w,
            vec![egui::Event::PointerMoved(p), button(p, true, false)],
        );
        frame(&ctx, &mut w, vec![button(p, false, false)]);
        assert_eq!(w.history_list.selected, Some(2));
        assert_eq!(w.chosen_commit, Some(Some(oid(A))));
        assert_eq!(span(&w), Some((0, 0)));
        assert_eq!(w.scroll_y, 0.0);

        // A commit that owns no lines: chosen, with no line, and greyed out without an age.
        let p = row_at(1);
        frame(
            &ctx,
            &mut w,
            vec![egui::Event::PointerMoved(p), button(p, true, false)],
        );
        frame(&ctx, &mut w, vec![button(p, false, false)]);
        assert_eq!(w.chosen_commit, Some(Some(oid(GONE))));
        assert_eq!(span(&w), None);
        let h = w.history().unwrap();
        let bc = blame_colors(&egui::Ui::new(ctx.clone(), Id::new("t"), UiBuilder::new()));
        let row = history_row(h, 1, &w.repo, &GraphOptions::default(), &bc);
        assert!(row.greyed);
        assert_eq!(row.hash_fill, None);
        let row = history_row(h, 0, &w.repo, &GraphOptions::default(), &bc);
        assert!(!row.greyed);
        assert_eq!(row.hash_fill, Some(bc.age(1.0)));
    }

    #[test]
    fn clicking_a_line_selects_its_commits_row() {
        let ctx = egui::Context::default();
        let mut w = listed_window();
        frame(&ctx, &mut w, Vec::new());
        click(&ctx, &mut w, 1, Modifiers::NONE);
        assert_eq!(w.chosen_commit, Some(Some(oid(B))));
        assert_eq!(w.history_list.selected, Some(0));
        click(&ctx, &mut w, 3, Modifiers::NONE);
        assert_eq!(w.history_list.selected, Some(2));
    }

    #[test]
    fn up_and_down_step_through_the_rows_wherever_the_pointer_is() {
        let ctx = egui::Context::default();
        let mut w = listed_window();
        frame(&ctx, &mut w, Vec::new());
        // Over the text.
        let over_text = egui::Event::PointerMoved(at(&ctx, 1));
        frame(&ctx, &mut w, vec![over_text, key(Key::ArrowDown)]);
        assert_eq!(w.history_list.selected, Some(0));
        assert_eq!(w.chosen_commit, Some(Some(oid(B))));
        frame(&ctx, &mut w, vec![key(Key::ArrowDown)]);
        assert_eq!((w.history_list.selected, span(&w)), (Some(1), None));
        frame(&ctx, &mut w, vec![key(Key::ArrowDown)]);
        frame(&ctx, &mut w, vec![key(Key::ArrowDown)]);
        assert_eq!(w.history_list.selected, Some(2));
        assert_eq!(w.chosen_commit, Some(Some(oid(A))));
        assert_eq!(span(&w), Some((0, 0)));
        frame(&ctx, &mut w, vec![key(Key::ArrowUp)]);
        assert_eq!(w.chosen_commit, Some(Some(oid(GONE))));
        assert_eq!(w.scroll_y, 0.0);

        // Not while the pane is hidden.
        w.show_history = false;
        frame(&ctx, &mut w, vec![key(Key::ArrowUp)]);
        assert_eq!(w.history_list.selected, Some(1));
    }

    #[test]
    fn a_rows_menu_blames_that_revision_and_shows_its_changes() {
        let ctx = egui::Context::default();
        let mut w = listed_window();
        let mut requests = Vec::new();
        w.act_row(RowAction::Blame(2), &ctx, &mut requests);
        // A's first line is line 0 in its version.
        let [BlameRequest::Blame(_, spec, Some(0))] = requests.as_slice() else {
            panic!("expected a blame: {requests:?}");
        };
        assert_eq!(spec.rev, Rev::Commit(oid(A)));
        assert_eq!(spec.path, "a.txt");

        // A commit that owns no lines: against its parent in the file's history.
        requests.clear();
        w.act_row(RowAction::ShowChanges(1), &ctx, &mut requests);
        let [BlameRequest::Diff(_, spec, 0)] = requests.as_slice() else {
            panic!("expected a diff: {requests:?}");
        };
        assert_eq!(spec.old.as_ref().map(|v| v.rev), Some(Rev::Commit(oid(A))));
        assert_eq!(
            spec.new.as_ref().map(|v| v.rev),
            Some(Rev::Commit(oid(GONE)))
        );

        // The snapshot has none of them, so there is no log to show.
        requests.clear();
        w.act_row(RowAction::ShowLog(0), &ctx, &mut requests);
        assert!(requests.is_empty());
    }

    #[test]
    fn the_pane_starts_as_the_last_window_left_it_and_its_divider_sets_the_height() {
        let settings = BlameWindowSettings {
            show_history: false,
            history_height: 150.0,
            ..BlameWindowSettings::default()
        };
        let w = BlameWindow::new(
            2,
            window().repo.clone(),
            window().spec.clone(),
            &settings,
            &Engine::InProcess,
        );
        assert!(!w.show_history);
        assert_eq!(w.history_height, 150.0);

        // Dragging the divider 50 points up makes the pane 50 points taller.
        let ctx = egui::Context::default();
        let mut w = listed_window();
        let mut settings = BlameWindowSettings::default();
        let graph = GraphOptions::default();
        let run = |w: &mut BlameWindow, settings: &mut BlameWindowSettings, events| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 700.0))),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                let env = Env {
                    palette: Palette::new(false, &[]),
                    graph: &graph,
                };
                w.contents(ui, settings, &mut true, &env);
            })
            .textures_delta
            .clear();
        };
        let before = settings.history_height;
        let middle = 700.0 - INFO_BAR_HEIGHT - before - DIVIDER_THICKNESS / 2.0;
        let (p, q) = (pos2(500.0, middle), pos2(500.0, middle - 50.0));
        run(&mut w, &mut settings, Vec::new());
        run(
            &mut w,
            &mut settings,
            vec![egui::Event::PointerMoved(p), button(p, true, false)],
        );
        run(&mut w, &mut settings, vec![egui::Event::PointerMoved(q)]);
        run(&mut w, &mut settings, vec![egui::Event::PointerMoved(q)]);
        run(&mut w, &mut settings, vec![button(q, false, false)]);
        assert!((settings.history_height - (before + 50.0)).abs() < 1.0);
        assert_eq!(w.history_height, settings.history_height);
    }

    #[test]
    fn blaming_again_or_closing_stops_the_listing() {
        let ctx = egui::Context::default();
        let mut w = window();
        w.spec.rev = Rev::WorkingTree;
        let first = w.stop_listing.clone();
        w.reload(&ctx);
        assert!(first.is_cancelled());
        assert!(matches!(w.listing, Listing::Running(_)));
        let second = w.stop_listing.clone();
        assert!(!second.is_cancelled());
        drop(w);
        assert!(second.is_cancelled());
    }

    #[test]
    fn the_toolbar_icons_pick_moved_lines_and_hide_the_history_pane() {
        let ctx = egui::Context::default();
        let mut w = listed_window();
        let mut settings = BlameWindowSettings::default();
        frame_with(&ctx, &mut w, &mut settings, Vec::new());
        let clicked = |ctx: &egui::Context, w: &mut BlameWindow, s: &mut _, p| {
            let events = vec![egui::Event::PointerMoved(p), button(p, true, false)];
            frame_with(ctx, w, s, events);
            frame_with(ctx, w, s, vec![button(p, false, false)]);
        };
        // The third of the moved-lines segments, after the two whitespace ones.
        clicked(
            &ctx,
            &mut w,
            &mut settings,
            pos2(177.0, TOOLBAR_HEIGHT / 2.0),
        );
        assert_eq!(w.options.moves, Moves::AcrossFiles);
        assert_eq!(settings.moves, Moves::AcrossFiles);

        // The history icon, at the right end.
        assert!(w.show_history);
        clicked(
            &ctx,
            &mut w,
            &mut settings,
            pos2(975.0, TOOLBAR_HEIGHT / 2.0),
        );
        assert!(!w.show_history);
        assert!(!settings.show_history);
    }

    #[test]
    fn the_toolbar_palette_toggles_the_syntax_colour_setting() {
        let ctx = egui::Context::default();
        let mut w = listed_window();
        let mut settings = BlameWindowSettings::default();
        let graph = GraphOptions::default();
        let mut syntax = true;
        let mut run = |w: &mut BlameWindow, syntax: &mut bool, events| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 700.0))),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                let env = Env {
                    palette: Palette::new(false, &[]),
                    graph: &graph,
                };
                w.contents(ui, &mut settings, syntax, &env);
            })
            .textures_delta
            .clear();
        };
        run(&mut w, &mut syntax, Vec::new());
        // After the whitespace and moved-lines segments and the gaps between: the palette.
        let p = pos2(211.0, TOOLBAR_HEIGHT / 2.0);
        for expected in [false, true] {
            let press = vec![egui::Event::PointerMoved(p), button(p, true, false)];
            run(&mut w, &mut syntax, press);
            run(&mut w, &mut syntax, vec![button(p, false, false)]);
            assert_eq!(syntax, expected);
        }
    }

    fn shift(key: Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::SHIFT,
        }
    }

    fn found(w: &BlameWindow) -> Vec<(usize, String)> {
        let lines = &w.ready().unwrap().blame.lines;
        w.find
            .matches
            .iter()
            .map(|m| {
                let text: String = lines[m.line].text.chars().collect::<Vec<_>>()[m.range.clone()]
                    .iter()
                    .collect();
                (m.line, text)
            })
            .collect()
    }

    fn has_find_focus(ctx: &egui::Context, w: &BlameWindow) -> bool {
        ctx.memory(|m| m.has_focus(w.find_id()))
    }

    #[test]
    fn ctrl_f_finds_as_typed_and_f3_and_enter_go_round_the_places() {
        let ctx = egui::Context::default();
        let mut w = window();
        frame(&ctx, &mut w, Vec::new());
        click(&ctx, &mut w, 1, Modifiers::NONE);
        let chosen = w.chosen_commit;
        frame(&ctx, &mut w, vec![ctrl(Key::F)]);
        assert!(has_find_focus(&ctx, &w));
        // Any case: "O" finds one, two and four; the first at or after the top is gone to.
        frame(&ctx, &mut w, vec![egui::Event::Text("O".into())]);
        assert_eq!(w.find.query, "O");
        assert_eq!(
            found(&w),
            [(0, "o".into()), (1, "o".into()), (3, "o".into())]
        );
        assert_eq!(w.find.current, Some(0));
        // Enter and F3 go on, round the end; Shift goes back.
        frame(&ctx, &mut w, vec![key(Key::Enter)]);
        assert_eq!(w.find.current, Some(1));
        assert!(has_find_focus(&ctx, &w));
        frame(&ctx, &mut w, vec![crate::keys::find_next().event()]);
        frame(&ctx, &mut w, vec![crate::keys::find_next().event()]);
        assert_eq!(w.find.current, Some(0));
        frame(&ctx, &mut w, vec![crate::keys::find_previous().event()]);
        assert_eq!(w.find.current, Some(2));
        let held = egui::Event::ModifiersChanged;
        frame(
            &ctx,
            &mut w,
            vec![held(Modifiers::SHIFT), shift(Key::Enter)],
        );
        frame(&ctx, &mut w, vec![held(Modifiers::NONE)]);
        assert_eq!(w.find.current, Some(1));
        // Tabs are found as in the file.
        frame(&ctx, &mut w, vec![egui::Event::Text("\tx".into())]);
        frame(
            &ctx,
            &mut w,
            vec![ctrl(Key::A), egui::Event::Text("o\tx".into())],
        );
        assert_eq!(found(&w), [(1, "o x".into())]);
        // Finding never changes the chosen commit or lines.
        assert_eq!(w.chosen_commit, chosen);
        assert_eq!(span(&w), Some((1, 1)));
        // F3 works outside the field too.
        w.find.current = None;
        ctx.memory_mut(|m| m.surrender_focus(w.find_id()));
        frame(&ctx, &mut w, vec![crate::keys::find_next().event()]);
        assert_eq!(w.find.current, Some(0));
    }

    #[test]
    fn typing_goes_to_the_first_place_from_the_top_of_the_view_and_scrolls_to_it() {
        let ctx = egui::Context::default();
        let mut w = window();
        w.show_history = false;
        // 300 lines; "needle" in lines 10 and 250.
        let out: String = (1..=300)
            .map(|i| {
                let text = if i == 11 || i == 251 {
                    "a needle"
                } else {
                    "hay"
                };
                entry(A, i, i, 100, "", text)
            })
            .collect();
        let repo = w.repo.clone();
        w.loaded(Ready::new(
            Blame::parse(out.as_bytes()).unwrap(),
            &repo,
            "",
            &Engine::InProcess,
            &Cancel::new(),
        ));
        frame(&ctx, &mut w, Vec::new());
        // Scrolled so that line 100 is at the top.
        let row_h = Metrics::new(&ctx, 300, w.repo.abbrev_len).row_h;
        w.scroll_to = Some(100.0 * row_h);
        frame(&ctx, &mut w, Vec::new());
        frame(&ctx, &mut w, Vec::new());
        assert_eq!(w.top, 100);
        frame(&ctx, &mut w, vec![ctrl(Key::F)]);
        frame(&ctx, &mut w, vec![egui::Event::Text("NEEDLE".into())]);
        assert_eq!(w.find.current, Some(1));
        frame(&ctx, &mut w, Vec::new());
        frame(&ctx, &mut w, Vec::new());
        let middle = w.top as f32;
        assert!(
            w.top <= 250 && 250 < w.top + 20,
            "line 250 in view, top {middle}"
        );
        // Round the end to line 10.
        frame(&ctx, &mut w, vec![key(Key::Enter)]);
        frame(&ctx, &mut w, Vec::new());
        frame(&ctx, &mut w, Vec::new());
        assert_eq!(w.find.current, Some(0));
        assert!(w.top <= 10, "line 10 in view, top {}", w.top);
    }

    #[test]
    fn esc_in_the_find_field_leaves_it_and_only_then_closes_the_window() {
        let ctx = egui::Context::default();
        let mut w = window();
        frame(&ctx, &mut w, Vec::new());
        frame(&ctx, &mut w, vec![ctrl(Key::F)]);
        frame(&ctx, &mut w, vec![egui::Event::Text("t".into())]);
        assert_eq!(w.find.matches.len(), 2);
        frame(&ctx, &mut w, vec![key(Key::Escape)]);
        assert!(!w.closed);
        assert!(!has_find_focus(&ctx, &w));
        assert_eq!(w.find.query, "");
        assert!(w.find.matches.is_empty());
        frame(&ctx, &mut w, vec![crate::keys::close_window().event()]);
        assert!(w.closed);
    }

    #[test]
    fn ctrl_f_offers_the_chosen_characters_or_the_windows_last_query_to_type_over() {
        let ctx = egui::Context::default();
        let mut w = window();
        let mut other = window();
        frame(&ctx, &mut w, Vec::new());
        // Characters chosen in one line: they are the query.
        let (from, to) = (char_at(&ctx, &w, 2, 1), char_at(&ctx, &w, 2, 4));
        drag(&ctx, &mut w, from, to, Modifiers::NONE);
        frame(&ctx, &mut w, vec![ctrl(Key::F)]);
        assert_eq!(w.find.query, "hre");
        assert_eq!(found(&w), [(2, "hre".into())]);
        frame(&ctx, &mut w, vec![key(Key::Escape)]);
        assert_eq!(w.find.query, "");

        // Otherwise the window's last query, selected: a paste replaces it. Characters over
        // several lines don't count.
        let (from, to) = (char_at(&ctx, &w, 0, 1), char_at(&ctx, &w, 1, 1));
        drag(&ctx, &mut w, from, to, Modifiers::NONE);
        frame(&ctx, &mut w, vec![ctrl(Key::F)]);
        assert_eq!(w.find.query, "hre");
        // A line copied whole: its line break is left out.
        frame(&ctx, &mut w, vec![egui::Event::Paste("four\n".into())]);
        assert_eq!(w.find.query, "four");
        assert_eq!(found(&w), [(3, "four".into())]);
        // Ctrl+A and Ctrl+C in the field are the field's: the lines stay as chosen, and the
        // text's characters aren't copied.
        let chosen = w.selection;
        frame(&ctx, &mut w, vec![ctrl(Key::A)]);
        assert_eq!(w.selection, chosen);
        assert!(has_find_focus(&ctx, &w));
        let copied = std::cell::Cell::new(None);
        let input = egui::RawInput {
            events: vec![egui::Event::Copy],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| copied.set(Some(w.copied(ui))));
        assert_eq!(copied.take(), Some(None));

        // Each window finds on its own: another has no last query.
        frame(&ctx, &mut other, Vec::new());
        frame(&ctx, &mut other, vec![ctrl(Key::F)]);
        assert_eq!(other.find.query, "");
        assert_eq!(w.find.last_query, "four");
    }

    fn has_go_to_focus(ctx: &egui::Context, w: &BlameWindow) -> bool {
        ctx.memory(|m| m.has_focus(w.go_to_id()))
    }

    /// Ctrl+G, a frame for the popup to size itself, then `text` typed into its field.
    fn go_to(ctx: &egui::Context, w: &mut BlameWindow, text: &str) {
        frame(ctx, w, vec![crate::keys::go_to_line().event()]);
        frame(ctx, w, Vec::new());
        frame(ctx, w, vec![egui::Event::Text(text.into())]);
    }

    /// [`window`] with 300 lines from A, the history pane hidden.
    fn long_window() -> BlameWindow {
        let mut w = window();
        w.show_history = false;
        let out: String = (1..=300).map(|i| entry(A, i, i, 100, "", "a")).collect();
        let repo = w.repo.clone();
        w.loaded(Ready::new(
            Blame::parse(out.as_bytes()).unwrap(),
            &repo,
            "",
            &Engine::InProcess,
            &Cancel::new(),
        ));
        w
    }

    #[test]
    fn ctrl_g_chooses_the_line_typed_whole_with_its_commit_and_row() {
        let ctx = egui::Context::default();
        let mut w = listed_window();
        frame(&ctx, &mut w, Vec::new());
        go_to(&ctx, &mut w, "2");
        assert!(w.go_to.open);
        assert!(has_go_to_focus(&ctx, &w));
        assert_eq!(w.typed_line(), Some(1));
        frame(&ctx, &mut w, vec![key(Key::Enter)]);
        assert!(!w.go_to.open);
        assert!(!has_go_to_focus(&ctx, &w));
        assert_eq!(span(&w), Some((1, 1)));
        assert_eq!(w.selection.and_then(|s| s.chosen_chars()), None);
        assert_eq!(w.chosen_commit, Some(Some(oid(B))));
        assert_eq!(w.history_list.selected, Some(0));
        // In view already: the text stays where it is.
        frame(&ctx, &mut w, Vec::new());
        assert_eq!(w.scroll_y, 0.0);

        // Again: the popup opens empty, and the next line goes to A's row.
        go_to(&ctx, &mut w, "4");
        assert_eq!(w.go_to.text, "4");
        frame(&ctx, &mut w, vec![key(Key::Enter)]);
        assert_eq!(span(&w), Some((3, 3)));
        assert_eq!(w.chosen_commit, Some(Some(oid(A))));
        assert_eq!(w.history_list.selected, Some(2));
    }

    #[test]
    fn a_line_out_of_view_is_scrolled_a_third_of_the_way_down_and_one_in_view_is_not() {
        let ctx = egui::Context::default();
        let mut w = long_window();
        frame(&ctx, &mut w, Vec::new());
        go_to(&ctx, &mut w, "250");
        frame(&ctx, &mut w, vec![key(Key::Enter)]);
        frame(&ctx, &mut w, Vec::new());
        frame(&ctx, &mut w, Vec::new());
        assert_eq!(span(&w), Some((249, 249)));
        let row_h = Metrics::new(&ctx, 300, w.repo.abbrev_len).row_h;
        let height = 700.0 - TOOLBAR_HEIGHT - HEADER_HEIGHT - INFO_BAR_HEIGHT;
        let above = (height / row_h / 3.0).floor() as usize;
        assert_eq!(w.top, 249 - above);

        // A line in view: chosen, without scrolling.
        let (top, scroll) = (w.top, w.scroll_y);
        go_to(&ctx, &mut w, &(top + 3).to_string());
        frame(&ctx, &mut w, vec![key(Key::Enter)]);
        frame(&ctx, &mut w, Vec::new());
        assert_eq!(span(&w), Some((top + 2, top + 2)));
        assert_eq!(w.scroll_y, scroll);
    }

    #[test]
    fn a_number_naming_no_line_is_marked_and_enter_does_nothing() {
        let ctx = egui::Context::default();
        let mut w = window();
        frame(&ctx, &mut w, Vec::new());
        click(&ctx, &mut w, 0, Modifiers::NONE);
        for typed in ["5", "0", "x"] {
            go_to(&ctx, &mut w, typed);
            assert_eq!(w.go_to.text, typed);
            assert_eq!(w.typed_line(), None);
            frame(&ctx, &mut w, vec![key(Key::Enter)]);
            frame(&ctx, &mut w, Vec::new());
            assert!(w.go_to.open);
            assert!(has_go_to_focus(&ctx, &w), "the field keeps the focus");
            assert_eq!(span(&w), Some((0, 0)));
        }
        // Nothing typed is nothing gone to either.
        frame(&ctx, &mut w, vec![key(Key::Backspace)]);
        assert_eq!(w.go_to.text, "");
        frame(&ctx, &mut w, vec![key(Key::Enter)]);
        assert!(w.go_to.open);
    }

    #[test]
    fn esc_closes_the_go_to_popup_and_only_then_the_window() {
        let ctx = egui::Context::default();
        let mut w = window();
        frame(&ctx, &mut w, Vec::new());
        go_to(&ctx, &mut w, "3");
        frame(&ctx, &mut w, vec![key(Key::Escape)]);
        assert!(!w.go_to.open);
        assert!(!w.closed);
        assert!(!has_go_to_focus(&ctx, &w));
        assert_eq!(span(&w), None);
        frame(&ctx, &mut w, vec![crate::keys::close_window().event()]);
        assert!(w.closed);

        // Not before there are lines to go to.
        let mut w = window();
        w.load = Load::Failed("no".into());
        frame(&ctx, &mut w, vec![crate::keys::go_to_line().event()]);
        assert!(!w.go_to.open);
    }

    #[test]
    fn hovering_a_rows_subject_fetches_its_whole_message_and_elsewhere_does_not() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(|s| s.interaction.tooltip_delay = 0.0);
        let mut w = listed_window();
        frame(&ctx, &mut w, Vec::new());
        // Over row 2's author: its own tooltip, nothing fetched.
        let author = pos2(780.0, row_at(2).y);
        frame(&ctx, &mut w, vec![egui::Event::PointerMoved(author)]);
        for _ in 0..3 {
            frame(&ctx, &mut w, Vec::new());
        }
        assert!(!w.details.0.cache.contains_key(&oid(A)));
        // Over row 0's subject: B's message is asked for.
        frame(&ctx, &mut w, vec![egui::Event::PointerMoved(row_at(0))]);
        for _ in 0..3 {
            frame(&ctx, &mut w, Vec::new());
        }
        assert!(w.details.0.cache.contains_key(&oid(B)));
        assert_eq!(w.details.0.cache.len(), 1);
    }

    #[test]
    fn a_subject_tip_shows_the_subject_until_the_whole_message_is_there() {
        let w = listed_window();
        let row = &w.history().unwrap().history.rows[0];
        let fetching = MessageTip::new(row, None);
        assert_eq!(fetching.message, "s200");
        assert_eq!(fetching.author, "A B <a@b>");
        assert_eq!((fetching.truncated, fetching.error), (false, None));

        let details = |message: &str| {
            Ok(CommitDetails {
                message: message.to_owned(),
                author_date: "2026-09-27 08:14:19 +0200".into(),
                committer_name: "C".into(),
                committer_email: "c@d".into(),
                committer_date: "2026-09-27 08:14:19 +0200".into(),
                notes: Vec::new(),
            })
        };
        let fetched = details("s200\n\nWhy it changed,\n  and how.\n");
        let tip = MessageTip::new(row, Some(&fetched));
        assert_eq!(tip.message, "s200\n\nWhy it changed,\n  and how.");
        assert_eq!(tip.date, "2026-09-27 08:14:19 +0200");
        assert!(!tip.truncated);

        // A long message is cut, and says so.
        let long: String = (1..=50).map(|i| format!("line {i}\n")).collect();
        let fetched = details(&long);
        let tip = MessageTip::new(row, Some(&fetched));
        assert_eq!(tip.message.lines().count(), MESSAGE_TIP_MAX_LINES);
        assert!(tip.truncated);

        // Git failed: the subject, and why.
        let failed = Err("no such commit".to_owned());
        let tip = MessageTip::new(row, Some(&failed));
        assert_eq!(tip.message, "s200");
        assert_eq!(tip.error, Some("no such commit"));
    }
}
