//! Blame windows: a file with the commit that last changed each of its lines, in a window of
//! its own (an immediate viewport, like the diff windows). Several can be open at once; they
//! close with the repository. The model is [`parterre_core::blame`].
//!
//! A gutter on the left names each run of lines from one commit (hash, author, date), shaded by
//! the commit's age among the file's commits as TortoiseGitBlame shades lines. Clicking a line
//! chooses it and its commit, whose lines are all highlighted. A drag over the text chooses
//! characters for copying, as in the diff windows (a double-click a word); a drag over the
//! commit column or the line numbers chooses whole lines; Shift+click extends either. Choosing
//! more keeps the commit of the line it started on, and the chosen commit stays chosen until
//! another is. An overview strip on the right marks the chosen commit's lines in the whole file
//! and shows the part in view; the text never scrolls by itself when a commit is chosen, the
//! strip scrolls it. The bar at the bottom describes the commit under the pointer or chosen. A
//! line's menu goes on from there, as TortoiseGitBlame's does: blame the version before its
//! commit (in a window of its own, the line chosen at its place there), show its commit's
//! change to the file in a diff window, or show the log from its commit. The toolbar says
//! whether whitespace changes and moved lines count, and whether the history pane shows,
//! remembered for the next window.
//!
//! The history pane below the text lists the commits that changed the file
//! ([`parterre_core::file_history`], listed by git alongside the blame) in the log window's
//! commit table, as decided in #111, #112 and #113. Choosing works both ways: a line selects
//! its commit's row, a row (or `Up`/`Down`) chooses its commit and first line, without
//! scrolling the text. The Hash cell carries the gutter's age colour; rows of commits that own
//! no lines are greyed out. Hovering a row's subject shows the commit's whole message, author
//! and date, fetched from git the first time (the subject alone until then). A row's menu
//! blames the file at that commit, shows its change to the file, or shows the log from it. The
//! divider above the pane is remembered for the next window.
//!
//! Find (`Ctrl+F`, #114) looks for text in the lines as in the file, ignoring case as the main
//! window's find does, in each window on its own. Every place found is highlighted and marked
//! in the overview strip, the one gone to stronger; Enter and `F3` go to the next, round the
//! end, scrolling it into view. Finding never changes the chosen commit, as in
//! TortoiseGitBlame. Esc in the find field leaves it; elsewhere it closes the window.
//!
//! Go to line (`Ctrl+G`), as in TortoiseGitBlame, asks for a line number in a popup under the
//! find field, with the lines there are. Enter chooses that line whole, and so its commit and
//! row, scrolling it into view; Esc closes the popup alone.

use std::collections::HashMap;
use std::sync::{Arc, mpsc};

use super::syntax;
use eframe::egui::text::{CCursor, LayoutJob, TextFormat};
use eframe::egui::{
    self, Color32, FontId, Id, Key, Modifiers, PopupCloseBehavior, Rect, RectAlign, RichText,
    ScrollArea, Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2,
};
use parterre_core::blame::{Blame, BlameOptions, BlameSpec, Moves, Origin};
use parterre_core::changed_files::FileStatus;
use parterre_core::file_diff::{FileDiffSpec, Rev, Version, display_column, raw_offset};
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

use super::commit_table::{CommitList, CommitTable, ROW, Row, Select};
use super::diff_window::{
    Colors, OVERVIEW, SCROLLBAR, colors, hscrollbar, message, overview_background, overview_scale,
    overview_scroll, overview_view, reveal,
};
use super::log_window::{self, Bar, DIVIDER, HEADING, cell, divider};
use super::{Details, ParterreApp};
use crate::keys;
use crate::settings::{BlameWindowSettings, Settings};
use crate::text_size;
use crate::theme::Palette;
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
/// The history pane is at least its headings and a row tall, and leaves the text this much.
const MIN_HISTORY: f32 = HEADING + ROW;
const MIN_TEXT: f32 = 80.0;

/// What a blame window asks the app for.
#[derive(Debug)]
pub enum BlameRequest {
    /// A blame window for the file at another revision, with a line (from 0) chosen if given.
    Blame(Arc<Repo>, BlameSpec, Option<usize>),
    /// A diff window: a line's commit's change to the file, at the line (from 0, in the new
    /// version).
    Diff(Arc<Repo>, FileDiffSpec, usize),
    /// The log window, from a line's commit.
    Log(Oid),
    /// Ctrl+F5: fetch, from this window.
    Fetch(egui::ViewportId),
}

/// The open blame windows.
#[derive(Debug, Default)]
pub struct BlameWindows {
    windows: Vec<BlameWindow>,
    /// How many were opened, to give each its own viewport id.
    opened: u64,
    requests: Vec<BlameRequest>,
    /// How files are coloured by syntax (#209).
    engine: Engine,
}

impl BlameWindows {
    pub fn new(engine: Engine) -> BlameWindows {
        BlameWindows {
            engine,
            ..BlameWindows::default()
        }
    }

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
        let mut w = BlameWindow::new(self.opened, repo, spec, settings, &self.engine);
        w.pending_line = line;
        w.load(ctx);
        self.windows.push(w);
    }

    /// Closes every blame window (the repository they belong to is closing).
    pub fn close_all(&mut self) {
        self.windows.clear();
        self.requests.clear();
    }

    /// True while git is still blaming, or listing a file's history, for any window.
    pub fn is_loading(&self) -> bool {
        self.windows
            .iter()
            .any(|w| matches!(w.load, Load::Loading(_)) || matches!(w.listing, Listing::Running(_)))
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
    /// The syntax spans of every line (#209), on the display text, when the file's language
    /// is known.
    syntax: Option<Vec<Spans>>,
}

impl Ready {
    fn new(blame: Blame, repo: &Repo, path: &str, engine: &Engine, cancel: &Cancel) -> Ready {
        // The spans of every line, moved onto the display text once.
        let syntax = highlight::language_of(path).map(|language| {
            let text: Vec<&str> = blame.lines.iter().map(|l| l.raw.as_str()).collect();
            let raw = engine.highlight(language, &text.join("\n"), cancel);
            blame
                .lines
                .iter()
                .enumerate()
                .map(|(i, l)| syntax::moved(&l.raw, raw.get(i).map_or(&[][..], Vec::as_slice)))
                .collect::<Vec<Spans>>()
        });
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
            syntax,
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

/// The history pane's rows, or how far listing them got.
#[derive(Debug)]
enum Listing {
    /// git lists the file's history on a worker thread.
    Running(mpsc::Receiver<Result<FileLog, String>>),
    /// Listed; the rows are made once the blame is there too.
    Listed(FileLog),
    Ready(Box<History>),
    Failed(String),
}

/// The history pane's rows, and what the pane shows for each.
#[derive(Debug)]
struct History {
    history: FileHistory,
    graph: LogGraph,
    /// Per row: the refs at its commit in the snapshot (indices into its refs), in the order
    /// the log shows them.
    refs: Vec<Vec<usize>>,
    /// Per row: its commit's age as the gutter shades it, if it owns lines.
    ages: Vec<Option<f32>>,
    /// Per row: the first line it owns.
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

    /// The path of the file at row `i`'s commit: where the log ran, or for a row only the
    /// blame names, the path its first line had there.
    fn path<'a>(&'a self, i: usize, spec: &'a BlameSpec) -> &'a str {
        match &self.history.rows[i].source {
            Source::WorkingTree => &spec.path,
            Source::Log => &self.history.path,
            Source::Blame { paths } => paths.first().unwrap_or(&self.history.path),
        }
    }
}

/// What the history pane needs from the app: how ref badges look, and which refs show.
struct Env<'a> {
    palette: Palette,
    graph: &'a GraphOptions,
}

/// What a row's menu (or a double-click) asked for, by row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowAction {
    Blame(usize),
    ShowChanges(usize),
    ShowLog(usize),
    CopyHash(usize),
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

/// "To the end of the line", as a column.
const LINE_END: usize = usize::MAX;

/// What is chosen for copying, from `anchor` to `head`, each a line (an index into the lines)
/// and a column (a character of the line's display text). A press on the commit column or the
/// line numbers chooses whole lines (`lines`), as does choosing a line on its own; a press on
/// the text chooses characters, and a click without a drag its line, whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Selection {
    anchor: (usize, usize),
    head: (usize, usize),
    lines: bool,
}

impl Selection {
    /// Lines `a` to `b`, whole.
    fn lines(a: usize, b: usize) -> Selection {
        Selection {
            anchor: (a, 0),
            head: (b, 0),
            lines: true,
        }
    }

    /// The first and last line it touches.
    fn span(&self) -> (usize, usize) {
        let (a, b) = (self.anchor.0, self.head.0);
        (a.min(b), a.max(b))
    }

    fn touches(&self, i: usize) -> bool {
        let (a, b) = self.span();
        (a..=b).contains(&i)
    }

    /// Where the chosen characters start and end, in order, unless whole lines are chosen.
    fn chars(&self) -> Option<((usize, usize), (usize, usize))> {
        if self.lines || self.anchor == self.head {
            None
        } else if self.anchor < self.head {
            Some((self.anchor, self.head))
        } else {
            Some((self.head, self.anchor))
        }
    }

    /// The display columns chosen in line `i`, when characters are (the end may be
    /// [`LINE_END`]).
    fn columns(&self, i: usize) -> Option<std::ops::Range<usize>> {
        let (a, b) = self.chars()?;
        if i < a.0 || i > b.0 {
            return None;
        }
        let start = if i == a.0 { a.1 } else { 0 };
        let end = if i == b.0 { b.1 } else { LINE_END };
        Some(start..end)
    }
}

/// Find in the text (Ctrl+F), in this window alone: the query, where it occurs, and the place
/// gone to. Finding never changes the chosen commit.
#[derive(Debug, Default)]
struct Find {
    query: String,
    /// The last query searched for, which Ctrl+F offers again after Esc cleared the field.
    last: String,
    /// Every place the query occurs: a line and display columns in it.
    matches: Vec<find::Match>,
    /// The place gone to (an index into `matches`).
    current: Option<usize>,
    /// Focus the field and select the query in the next frame.
    focus: bool,
    /// Scroll the current place into view in the next frame.
    reveal: bool,
}

/// Go to line (Ctrl+G): the popup asking for a line number, in this window alone. What was
/// typed isn't kept for the next time.
#[derive(Debug, Default)]
struct GoTo {
    open: bool,
    text: String,
    /// Focus the field once the popup is drawn for real (its first frame only sizes it).
    focus: bool,
}

#[derive(Debug)]
struct BlameWindow {
    id: u64,
    repo: Arc<Repo>,
    spec: BlameSpec,
    /// The size the window opened with (the viewport builder must not change while it is open).
    size: Vec2,
    load: Load,
    /// The history pane's rows, listed alongside the blame.
    listing: Listing,
    /// Stops the listing (when the window closes or blames again).
    cancel: Cancel,
    /// Stops the syntax colouring's child process likewise; its own handle, since a handle
    /// holds one child at a time and the listing's git command has this one.
    colour: Cancel,
    /// How the file is coloured by syntax (#209).
    engine: Engine,
    /// The history pane's selected row and scroll position.
    list: CommitList,
    /// The whole messages of the history pane's commits, fetched from git the first time a
    /// row's subject is hovered.
    details: Details,
    /// The history pane is shown, this tall (with its headings).
    show_history: bool,
    history_height: f32,
    /// The options chosen in the toolbar, and those of the last load started.
    options: BlameOptions,
    requested: BlameOptions,
    /// Lines or characters chosen, for copying.
    selection: Option<Selection>,
    /// The chosen commit, whose lines are highlighted (`None` inside is uncommitted). Set with
    /// the line a choice of lines starts at, or on its own ([`BlameWindow::choose_commit`]).
    chosen: Option<Option<Oid>>,
    /// The mouse is choosing lines or characters.
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
    /// The first line drawn in the last frame.
    top: usize,
    find: Find,
    go_to: GoTo,
    /// Scroll this line into view in the next frame, if it is out of view.
    reveal_line: Option<usize>,
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
        engine: &Engine,
    ) -> BlameWindow {
        let [w, h] = settings.size;
        let options = BlameOptions {
            ignore_whitespace: settings.ignore_whitespace,
            moves: settings.moves,
        };
        BlameWindow {
            engine: engine.clone(),
            id,
            repo,
            spec,
            size: vec2(w, h),
            load: Load::Failed(String::new()),
            listing: Listing::Failed(String::new()),
            cancel: Cancel::new(),
            colour: Cancel::new(),
            list: CommitList::default(),
            details: Details::default(),
            show_history: settings.show_history,
            history_height: settings.history_height,
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
            top: 0,
            find: Find::default(),
            go_to: GoTo::default(),
            reveal_line: None,
            focus: false,
            closed: false,
            title_theme: None,
        }
    }

    /// Blames `spec` with the current options, and lists the file's history, on worker
    /// threads. A listing still running is stopped.
    fn load(&mut self, ctx: &egui::Context) {
        // New handles for this load; the ones before stop their listing and colouring.
        self.cancel.cancel();
        self.cancel = Cancel::new();
        self.colour.cancel();
        self.colour = Cancel::new();
        let (tx, rx) = mpsc::channel();
        let git = Git::new(&self.repo.path);
        let (spec, options, repo) = (self.spec.clone(), self.options, self.repo.clone());
        let (engine, cancel) = (self.engine.clone(), self.colour.clone());
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = git
                .blame(&spec, options)
                .map(|blame| Ready::new(blame, &repo, &spec.path, &engine, &cancel))
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
            repaint.request_repaint();
        });
        self.requested = options;
        self.load = Load::Loading(rx);
        self.dragging = false;

        let (tx, rx) = mpsc::channel();
        let (git, spec, cancel) = (
            Git::new(&self.repo.path),
            self.spec.clone(),
            self.cancel.clone(),
        );
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = git.file_log(&spec, &cancel).map_err(|e| e.to_string());
            let _ = tx.send(result);
            repaint.request_repaint();
        });
        self.listing = Listing::Running(rx);
    }

    /// Takes the workers' results when they are there, and blames again if the options
    /// changed.
    fn poll(&mut self, ctx: &egui::Context) {
        if let Load::Loading(rx) = &self.load
            && let Ok(result) = rx.try_recv()
        {
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
        if self.options != self.requested {
            // Keep the place: the lines stay, only their commits may change.
            self.pending_scroll = Some(self.scroll);
            self.load(ctx);
        }
    }

    /// Shows a blame, keeping the chosen lines if the file still has them. Whether the chosen
    /// commit stays is up to the history ([`BlameWindow::settle`]).
    fn loaded(&mut self, ready: Ready) {
        if self
            .selection
            .is_some_and(|s| s.span().1 >= ready.blame.lines.len())
        {
            self.selection = None;
        }
        self.load = Load::Ready(Box::new(ready));
        // The lines are the same but for a working tree's; the place gone to stays if it can.
        let current = self.find.current;
        self.refind();
        self.find.current = current.filter(|&c| c < self.find.matches.len());
    }

    /// Makes the history pane's rows once the blame and the listing are both there, and
    /// selects the chosen commit's row. The chosen commit stays (a reload) while it is still
    /// listed, or if the listing failed, while it still owns lines.
    fn settle(&mut self) {
        let Load::Ready(ready) = &self.load else {
            return;
        };
        let keep = match &mut self.listing {
            listing @ Listing::Listed(_) => {
                let Listing::Listed(log) =
                    std::mem::replace(listing, Listing::Failed(String::new()))
                else {
                    unreachable!()
                };
                let history = History::new(log, ready, &self.repo);
                let row = self.chosen.and_then(|c| history.history.row_of(c));
                self.list.select(row);
                *listing = Listing::Ready(Box::new(history));
                row.is_some()
            }
            Listing::Failed(_) => self
                .chosen
                .is_some_and(|c| ready.blame.origins.iter().any(|o| o.commit == c)),
            Listing::Running(_) | Listing::Ready(_) => return,
        };
        if !keep {
            self.chosen = None;
        }
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

    fn history(&self) -> Option<&History> {
        match &self.listing {
            Listing::Ready(h) => Some(h),
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

    /// Chooses line `i` and its commit, and selects the commit's row in the history pane.
    fn choose_line(&mut self, i: usize) {
        self.selection = Some(Selection::lines(i, i));
        if let Some(commit) = self.origin(i).map(|o| o.commit) {
            self.chosen = Some(commit);
            if let Some(h) = self.history() {
                let row = h.history.row_of(commit);
                self.list.select(row);
            }
        }
    }

    /// Chooses row `i` of the history pane: selects it, keeping it in view, and chooses its
    /// commit ([`BlameWindow::choose_commit`]).
    fn choose_row(&mut self, i: usize) {
        let Some(commit) = self
            .history()
            .and_then(|h| h.history.rows.get(i))
            .map(|r| r.commit)
        else {
            return;
        };
        self.list.select(Some(i));
        self.choose_commit(commit);
    }

    /// `Up`/`Down`: chooses the row above (newer, `by` < 0) or below the selected one, or the
    /// first row if none is selected.
    fn step(&mut self, by: isize) {
        let Some(n) = self.history().map(|h| h.history.rows.len()) else {
            return;
        };
        if n == 0 {
            return;
        }
        let i = match self.list.selected {
            Some(i) => i.saturating_add_signed(by).min(n - 1),
            None => 0,
        };
        self.choose_row(i);
    }

    /// Chooses `commit` (`None` for the uncommitted lines) and its first line, or no line if it
    /// owns none in this version. The text stays where it is.
    fn choose_commit(&mut self, commit: Option<Oid>) {
        self.chosen = Some(commit);
        self.selection = self.ready().and_then(|r| {
            let first = r
                .blame
                .lines
                .iter()
                .position(|l| r.blame.origins[l.origin].commit == commit)?;
            Some(Selection::lines(first, first))
        });
    }

    /// What Ctrl+C copies: the chosen characters as in the file (tabs kept), a line per line
    /// they span, ending with a newline if they run to the end of the last; otherwise the
    /// chosen lines ([`BlameWindow::lines_text`]).
    fn selected_text(&self) -> Option<String> {
        let s = self.selection?;
        let Some((_, end)) = s.chars() else {
            return self.lines_text();
        };
        let lines = &self.ready()?.blame.lines;
        let (a, b) = s.span();
        let mut parts = Vec::new();
        for (i, line) in lines.iter().enumerate().take(b + 1).skip(a) {
            let cols = s.columns(i)?;
            let start = raw_offset(&line.raw, cols.start);
            let end = if cols.end == LINE_END {
                line.raw.len()
            } else {
                raw_offset(&line.raw, cols.end)
            };
            parts.push(&line.raw[start..end.max(start)]);
        }
        let mut text = parts.join("\n");
        if end.1 == LINE_END {
            text.push('\n');
        }
        Some(text)
    }

    /// Every line the choice touches, whole, as in the file, each ending with a newline.
    fn lines_text(&self) -> Option<String> {
        let (a, b) = self.selection?.span();
        let lines = &self.ready()?.blame.lines;
        let b = b.min(lines.len().checked_sub(1)?);
        let mut text = String::new();
        for line in lines.get(a..=b)? {
            text.push_str(&line.raw);
            text.push('\n');
        }
        Some(text)
    }

    fn select_all(&mut self) {
        if let Some(n) = self.ready().map(|r| r.blame.lines.len())
            && n > 0
        {
            self.selection = Some(Selection::lines(0, n - 1));
        }
    }

    /// The single line of the chosen characters, if they are on one line.
    fn selected_in_line(&self) -> Option<String> {
        let (a, b) = self.selection?.chars()?;
        (a.0 == b.0 && b.1 != LINE_END)
            .then(|| self.selected_text())
            .flatten()
    }

    fn find_id(&self) -> Id {
        Id::new(("blame-find", self.id))
    }

    /// Ctrl+F: focuses the find field with the chosen characters, if they are on one line, or
    /// else the last query, selected so that typing replaces it.
    fn open_find(&mut self) {
        let query = self
            .selected_in_line()
            .unwrap_or_else(|| self.find.last.clone());
        if query != self.find.query {
            self.find.query = query;
            self.query_changed();
        }
        self.find.focus = true;
    }

    /// Esc in the find field, or its clear button: empties it, and leaves it.
    fn close_find(&mut self, ctx: &egui::Context) {
        self.find.query.clear();
        self.refind();
        ctx.memory_mut(|m| m.surrender_focus(self.find_id()));
    }

    /// Finds the query in the lines, as in the file, with nothing gone to yet.
    fn refind(&mut self) {
        self.find.current = None;
        self.find.matches = match self.ready() {
            Some(r) => {
                let lines = &r.blame.lines;
                find::find(lines.iter().map(|l| &l.raw), &self.find.query)
                    .into_iter()
                    .map(|m| {
                        let raw = &lines[m.line].raw;
                        let columns =
                            display_column(raw, m.range.start)..display_column(raw, m.range.end);
                        find::Match {
                            line: m.line,
                            range: columns,
                        }
                    })
                    .collect()
            }
            None => Vec::new(),
        };
    }

    /// The query was typed: goes to its first place at or after the top of the view.
    fn query_changed(&mut self) {
        self.refind();
        if !self.find.query.is_empty() {
            self.find.last = self.find.query.clone();
        }
        self.find.current = find::first_from(&self.find.matches, self.top);
        self.find.reveal = true;
    }

    /// Enter, F3 (`forward`), Shift+Enter, Shift+F3: goes to the next or previous place,
    /// round the ends.
    fn find_step(&mut self, forward: bool) {
        self.find.current = find::step(&self.find.matches, self.find.current, forward, self.top);
        self.find.reveal = true;
    }

    fn go_to_id(&self) -> Id {
        Id::new(("blame-go-to", self.id))
    }

    /// Ctrl+G: opens the go-to-line popup with an empty field, once there are lines to go to.
    fn open_go_to(&mut self) {
        if self.ready().is_some_and(|r| !r.blame.lines.is_empty()) {
            self.go_to = GoTo {
                open: true,
                text: String::new(),
                focus: true,
            };
        }
    }

    /// Esc, a click outside the popup, or going: closes the popup, and leaves its field.
    fn close_go_to(&mut self, ctx: &egui::Context) {
        self.go_to.open = false;
        ctx.memory_mut(|m| m.surrender_focus(self.go_to_id()));
    }

    /// The line (from 0) the go-to field names, if it names one of the file's.
    fn typed_line(&self) -> Option<usize> {
        line_number(&self.go_to.text, self.ready()?.blame.lines.len())
    }

    /// Enter in the go-to field: chooses the line it names, whole, and so its commit and row,
    /// scrolls it into view and closes the popup. False, doing nothing, if it names no line.
    fn go_to_typed_line(&mut self, ctx: &egui::Context) -> bool {
        let Some(i) = self.typed_line() else {
            return false;
        };
        self.choose_line(i);
        self.reveal_line = Some(i);
        self.close_go_to(ctx);
        true
    }

    /// Asks for a blame of the version before line `i`'s commit, in a window of its own, with
    /// the line's place in that version chosen, near where it came in. This window stays as it
    /// is.
    fn blame_previous(&self, i: usize, requests: &mut Vec<BlameRequest>) {
        let Some(spec) = self.origin(i).and_then(Origin::previous_blame) else {
            return;
        };
        if let Some(line) = self.ready().map(|r| r.blame.lines[i].orig_line as usize) {
            requests.push(BlameRequest::Blame(self.repo.clone(), spec, Some(line)));
        }
    }

    /// Ctrl+F finds, F3 and Shift+F3 (⌘G and ⇧⌘G on macOS) go to the next and previous place,
    /// and Esc in the find field leaves it; Ctrl+G (⌃G on macOS) asks for a line to go to, and
    /// Esc then closes the popup; elsewhere Esc (⌘W on macOS) closes. Ctrl+A chooses every line; `Up`/`Down` step through the history
    /// pane wherever the pointer is, while it shows. While the find field or the popup has the
    /// focus, other keys are its own.
    fn handle_keys(&mut self, ui: &Ui) {
        let id = self.find_id();
        // egui drops the focus on Esc before the frame starts.
        let in_find = ui.memory(|m| m.has_focus(id) || m.had_focus_last_frame(id));
        // The previous place first: egui takes F3 for Shift+F3 too, and ⌘G for ⇧⌘G.
        let (previous, next) = ui.input_mut(|i| {
            (
                keys::find_previous().consume(i),
                keys::find_next().consume(i),
            )
        });
        if previous || next {
            self.find_step(next);
        }
        if ui.input_mut(|i| keys::FIND.consume(i)) {
            self.open_find();
        }
        if ui.input_mut(|i| keys::go_to_line().consume(i)) {
            self.open_go_to();
        }
        if self.go_to.open {
            // Never the window.
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
                self.step(by);
            }
        }
        if ui.input_mut(|i| keys::close_window().consume(i)) {
            self.closed = true;
        }
    }

    /// Where the text, the divider and the history pane go in `above` (what the toolbar,
    /// header and bar leave): the pane at the bottom as tall as asked, less if the text would
    /// get less than [`MIN_TEXT`].
    fn arrange(&self, above: Rect) -> (Rect, Option<(Rect, Rect)>) {
        if !self.show_history {
            return (above, None);
        }
        let height = self.history_height.min(above.height() - DIVIDER - MIN_TEXT);
        let top = above.bottom() - height.max(0.0);
        let bar = Rect::from_x_y_ranges(above.x_range(), top - DIVIDER..=top);
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
        let above = Rect::from_min_max(rest.min, pos2(rest.right(), rest.bottom() - INFO));
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
            // The divider: dragging it sets the pane's height, for the next window too.
            let bar = Bar {
                rect: bar,
                vertical: false,
                origin: 0.0,
                room: 1.0,
            };
            if let Some(middle) = divider(ui, Id::new(("blame-divider", self.id)), &bar) {
                let most = (above.height() - DIVIDER - MIN_TEXT).max(MIN_HISTORY);
                let height = above.bottom() - (middle + DIVIDER / 2.0);
                self.history_height = height.clamp(MIN_HISTORY, most);
                settings.history_height = self.history_height;
            }
        }
        self.info_bar(ui, info, hovered, &c);
        ui.allocate_rect(rest, Sense::hover());
        requests
    }

    /// Whether whitespace changes and moved lines count, as icon segments; on the right,
    /// whether the history pane shows.
    fn toolbar(&mut self, ui: &mut Ui, settings: &mut BlameWindowSettings, syntax: &mut bool) {
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

        let moves = Moves::ALL.map(|m| (m, moves_glyph(m)));
        let picked = widgets::segmented(ui, self.options.moves, &moves, |m, r| {
            let title = format!("Moved lines: {}", m.label().to_lowercase());
            widgets::tip_explained(r, &title, "", m.description())
        });
        if let Some(moves) = picked {
            self.options.moves = moves;
            settings.moves = moves;
        }
        ui.add_space(14.0);
        widgets::syntax_button(ui, syntax);
        if self.spec.reads_working_tree() {
            ui.add_space(14.0);
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

            // Find, in the middle of what is left, as in the main window.
            let room = ui.available_width();
            ui.allocate_ui_with_layout(
                vec2(room, widgets::BUTTON),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    // Narrow windows squeeze the field rather than the tools.
                    let width = (room - 16.0).clamp(60.0, 380.0);
                    ui.add_space(((room - width) / 2.0).max(0.0));
                    let left = ui.cursor().left();
                    self.find_field(ui, width);
                    let rect = ui.min_rect();
                    Rect::from_x_y_ranges(left..=rect.right(), rect.y_range())
                },
            )
            .inner
        });
        self.go_to_popup(ui, find.inner);
    }

    /// The go-to-line popup, centred under `under` (where the find field is), while it is
    /// open.
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

    /// The popup's field, marked while it names none of the `n` lines, and the lines there
    /// are. Enter goes to the line, if it names one.
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

    /// The find field, with how many places the query was found at.
    fn find_field(&mut self, ui: &mut Ui, width: f32) {
        let n = self.find.matches.len();
        let count = match (self.find.current, n) {
            (_, 0) => "No matches".to_owned(),
            (Some(c), n) => format!("{} of {n}", c + 1),
            (None, 1) => "1 match".to_owned(),
            (None, n) => format!("{n} matches"),
        };
        let focus = std::mem::take(&mut self.find.focus);
        let find = widgets::Find {
            id: self.find_id(),
            width,
            hint: "Find in the file",
            count: &count,
            keys: &keys::find_field_keys(),
            focus,
            select: focus,
        };
        let found = widgets::find_field(ui, &find, &mut self.find.query);
        if found.changed {
            self.query_changed();
        }
        if found.cleared {
            self.close_find(ui.ctx());
        }
        if found.next || found.previous {
            self.find_step(found.next);
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
        syntax_on: bool,
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
        let Metrics {
            row_h,
            char_w,
            hash_w,
            gutter,
            numbers,
        } = Metrics::new(ui.ctx(), n, self.repo.abbrev_len);
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
            let commit = blame.origins[blame.lines[line].origin].commit;
            self.selection = Some(Selection::lines(line, line));
            self.chosen = Some(commit);
            if let Listing::Ready(h) = &self.listing {
                self.list.select(h.history.row_of(commit));
            }
            let above = (area.height() / row_h / 3.0).floor();
            self.scroll_to = Some(((line as f32 - above) * row_h).max(0.0));
        } else if let Some(offset) = self.pending_scroll.take() {
            self.scroll_to = Some(offset);
        }
        // A line gone to: into view, a third of the way down if it was out of it.
        if let Some(line) = self.reveal_line.take() {
            let line = line.min(n - 1);
            if let Some(offset) = reveal(line, self.scroll, area.height(), row_h) {
                self.scroll_to = Some(offset);
            }
        }
        // The place found gone to: the same, and sideways too.
        if std::mem::take(&mut self.find.reveal)
            && let Some(m) = self.find.current.and_then(|c| self.find.matches.get(c))
        {
            if let Some(offset) = reveal(m.line, self.scroll, area.height(), row_h) {
                self.scroll_to = Some(offset);
            }
            let (x0, x1) = (m.range.start as f32 * char_w, m.range.end as f32 * char_w);
            if x0 < self.hoff || x1 > self.hoff + text_w - char_w {
                self.hoff = if x1 < text_w - char_w { 0.0 } else { x0 - 80.0 };
            }
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

        let selection = self.selection;
        let highlighted = self.chosen;
        let (matches, current) = (&self.find.matches, self.find.current);
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let pressed = ui.input(|i| i.pointer.primary_pressed());
        let (hoff, abbrev) = (self.hoff, self.repo.abbrev_len);
        let mut hovered = None;
        // The line and column under the pointer.
        let mut hover_at = None;
        // Where the primary button went down: line, column, and whether on the commit column
        // or the line numbers.
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
                // Whole lines are chosen as before; chosen characters are drawn over the text.
                let whole = selection.is_some_and(|s| s.chars().is_none() && s.touches(i));
                if whole {
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
                // The line in its syntax colours, when it has any.
                let spans = ready.syntax.as_ref().filter(|_| syntax_on);
                let g = match spans.and_then(|s| s.get(i)) {
                    Some(spans) if !spans.is_empty() => {
                        let dark = ui.visuals().dark_mode;
                        let mut job = LayoutJob::default();
                        for (piece, kind, _) in syntax::sections(&line.text, &[], spans) {
                            let format = syntax::text_format(
                                kind,
                                font.clone(),
                                dark,
                                c.text,
                                Color32::TRANSPARENT,
                            );
                            job.append(&line.text[piece], 0.0, format);
                        }
                        painter.layout_job(job)
                    }
                    _ => painter.layout_no_wrap(line.text.clone(), font.clone(), c.text),
                };
                let at = pos2(text_x - hoff, y - g.size().y / 2.0);
                let text = painter.with_clip_rect(clip);
                let x = |col| at.x + g.pos_from_cursor(CCursor::new(col)).min.x;
                // Every place found in the line, the one gone to stronger.
                let from = matches.partition_point(|m| m.line < i);
                for (k, m) in matches.iter().enumerate().skip(from) {
                    if m.line != i {
                        break;
                    }
                    let fill = if current == Some(k) {
                        c.found_current
                    } else {
                        c.found
                    };
                    let place =
                        Rect::from_x_y_ranges(x(m.range.start)..=x(m.range.end), rect.y_range());
                    text.rect_filled(place, 0.0, fill);
                }
                if let Some(cols) = selection.and_then(|s| s.columns(i)) {
                    let x1 = if cols.end == LINE_END {
                        // Past the end, a little, to show the line break is included.
                        at.x + g.size().x + row_h / 2.0
                    } else {
                        x(cols.end)
                    };
                    let chosen = Rect::from_x_y_ranges(x(cols.start)..=x1, rect.y_range());
                    text.rect_filled(chosen, 0.0, c.selection);
                }
                text.galley(at, g.clone(), c.text);

                if let Some(p) = pointer.filter(|p| rect.y_range().contains(p.y)) {
                    hovered = Some(i);
                    let col = if p.x <= at.x {
                        0
                    } else {
                        g.cursor_from_pos(p - at).index.0
                    };
                    hover_at = Some((i, col));
                    let on_gutter = p.x < text_x - PAD;
                    if pressed && response.hovered() {
                        press = Some((i, col, on_gutter));
                    }
                    if !on_gutter && response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                    }
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
                            crate::usage::menu(ui.ctx(), crate::usage::Menu::Blame);
                            ui.set_min_width(crate::menu::MIN_WIDTH);
                            let chosen = selection.is_some_and(|s| {
                                let (a, b) = s.span();
                                a != b && s.touches(i)
                            });
                            let in_repo = ready.in_repo[line.origin];
                            if let Some(a) = line_menu(ui, i, origin, in_repo, chosen) {
                                action = Some(a);
                            }
                        });
                    });
            }
        });
        self.scroll = out.state.offset.y;
        self.top = first.unwrap_or(0);

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
        // The lines the query was found in, over the chosen commit's, in a colour of their own.
        let scale = overview_scale(strip, n, row_h);
        let mut marked = None;
        for m in &self.find.matches {
            if marked == Some(m.line) {
                continue;
            }
            marked = Some(m.line);
            let y0 = m.line as f32 * scale;
            let mark = Rect::from_min_max(
                pos2(strip.left() + 3.0, strip.top() + y0),
                pos2(strip.right() - 2.0, strip.top() + y0 + scale.max(2.0)),
            );
            ui.painter().rect_filled(mark, 0.0, c.found_mark);
        }
        overview_view(ui, strip, view, c);
        let id = egui::Id::new(("blame-overview", self.id));
        if let Some(offset) = overview_scroll(ui, id, strip, n, row_h, view) {
            self.scroll_to = Some(offset);
        }

        self.select(ui, press, hover_at, secondary, (first, last), area, row_h);

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

    /// Chooses with the mouse, from what the rows saw this frame (`press` and `hover` as line,
    /// column, and for a press whether on the commit column or the line numbers; `drawn`, the
    /// first and last row drawn). A press on the text chooses its line and commit, and a drag
    /// from there characters; a press on the commit column or the line numbers chooses lines.
    /// The commit stays the one of the line the choosing started on. Shift extends; a drag
    /// scrolls past the edges; a double-click on the text takes a word. A right-click outside
    /// the chosen lines chooses its line and commit.
    #[allow(clippy::too_many_arguments)]
    fn select(
        &mut self,
        ui: &Ui,
        press: Option<(usize, usize, bool)>,
        hover: Option<(usize, usize)>,
        secondary: Option<usize>,
        (first, last): (Option<usize>, Option<usize>),
        area: Rect,
        row_h: f32,
    ) {
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
        if let Some((i, col, on_gutter)) = press {
            match self.selection {
                Some(s) if shift => {
                    let lines = s.lines || on_gutter;
                    let head = (i, if lines { 0 } else { col });
                    self.selection = Some(Selection { head, lines, ..s });
                }
                _ => {
                    self.choose_line(i);
                    if !on_gutter {
                        self.selection = Some(Selection {
                            anchor: (i, col),
                            head: (i, col),
                            lines: false,
                        });
                    }
                }
            }
            self.dragging = true;
        }
        // Not only after a press: a quick double-click can end in the frame it started.
        if double
            && let Some((i, col)) = hover
            && self.selection.is_some_and(|s| !s.lines && s.touches(i))
            && let Some(word) = self
                .ready()
                .and_then(|r| word_at(&r.blame.lines[i].text, col))
        {
            self.selection = Some(Selection {
                anchor: (i, word.start),
                head: (i, word.end),
                lines: false,
            });
            self.dragging = false;
        } else if press.is_none()
            && let Some(i) = secondary
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
        let pointer = ui.input(|i| i.pointer.interact_pos());
        if let Some((i, col)) = hover {
            s.head = (i, if s.lines { 0 } else { col });
        } else if let Some(p) = pointer {
            // Past the top or bottom: take the line beyond those drawn, and scroll on.
            if p.y < area.top() {
                if let Some(f) = first {
                    s.head = (f.saturating_sub(1), 0);
                }
                self.scroll_to = Some(self.scroll - row_h);
            } else if p.y > area.bottom() {
                if let Some(l) = last {
                    s.head = ((l + 1).min(n - 1), if s.lines { 0 } else { LINE_END });
                }
                self.scroll_to = Some(self.scroll + row_h);
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

    /// The history pane in `rect`: the commits that changed the file in the log window's commit
    /// table, or how far listing them got. A click on a row chooses it, a double-click shows
    /// its change, a right-click chooses it and opens its menu.
    fn history_pane(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        env: &Env,
        requests: &mut Vec<BlameRequest>,
    ) {
        let c = log_window::colors(ui);
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
        let ready = match &self.load {
            Load::Ready(r) => Some(&**r),
            _ => None,
        };
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
            &mut self.list,
            |i| match history {
                Some(h) => history_row(h, i, repo, env.graph, &bc),
                None => Row::default(),
            },
            |ui, i, _| {
                let (Some(h), Some(ready)) = (history, ready) else {
                    return;
                };
                let changes = row_changes(h, i, ready, repo, spec).is_some();
                if let Some(a) = history_menu(ui, i, &h.history.rows[i], changes) {
                    action = Some(a);
                }
            },
            Some(&mut subject_tip),
        );
        if let Some((text, color)) = status {
            let below = Rect::from_min_max(pos2(rect.left(), rect.top() + HEADING), rect.max);
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

    /// What Ctrl+C copies from the text, unless the find field has the focus (then it copies
    /// from the field).
    fn copied(&self, ui: &Ui) -> Option<String> {
        let copy = ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        let in_find = ui.memory(|m| m.has_focus(self.find_id()));
        (copy && !in_find).then(|| self.selected_text()).flatten()
    }

    /// Shows the window; sets `closed` when it was closed. Ctrl+wheel and Ctrl+plus, minus
    /// and 0 change `text_size`.
    fn show(
        &mut self,
        ctx: &egui::Context,
        settings: &mut Settings,
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
        crate::usage::screen(ctx, id.0, crate::usage::Screen::Blame);
        if self.title_theme.is_none() && !ctx.embed_viewports() {
            ctx.request_repaint();
        }
        if std::mem::take(&mut self.focus) && !ctx.embed_viewports() {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        }
        let mut requests = Vec::new();
        ctx.show_viewport_immediate(id, builder, |ui, class| {
            super::commands::window_begin(ui);
            self.poll(ui.ctx());
            if class != egui::ViewportClass::EmbeddedWindow {
                if self.title_theme != window_theme {
                    self.title_theme = window_theme;
                    if let Some(theme) = window_theme {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::SetTheme(theme));
                    }
                }
                // Reload blames again, and fetch, as in the main window.
                let (close, size, (reload, fetch)) = ui.input_mut(|i| {
                    (
                        i.viewport().close_requested(),
                        i.viewport().inner_rect.map(|r| r.size()),
                        crate::keys::reload_and_fetch(i),
                    )
                });
                if let Some(size) = size
                    && size.x > 0.0
                    && size.y > 0.0
                {
                    settings.blame_window.size = [size.x, size.y];
                }
                if reload {
                    self.reload(ui.ctx());
                }
                if fetch {
                    requests.push(BlameRequest::Fetch(id));
                }
                // Keys go to the main window too when the window is embedded in it.
                self.handle_keys(ui);
                text_size::read_input(ui, &mut settings.text_size, true);
                if close {
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
            egui::CentralPanel::default()
                .frame(egui::Frame::central_panel(&ui.ctx().global_style()).inner_margin(0))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    requests = self.contents(
                        ui,
                        &mut settings.blame_window,
                        &mut settings.syntax_colour,
                        &env,
                    );
                });
        });
        requests
    }
}

impl Drop for BlameWindow {
    /// Stops the listing and the colouring when the window closes (and with it the
    /// repository).
    fn drop(&mut self) {
        self.cancel.cancel();
        self.colour.cancel();
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

/// What row `i` of the history pane shows: the working tree changes, or a commit with its refs
/// (those `graph` shows), the gutter's age colour behind the hash, the path the file had for a
/// row only the blame names, and a mark if the snapshot doesn't have it. Greyed out if it owns
/// no lines.
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
        refs: super::log_window::badges(repo, &h.refs[i], row.snapshot, graph),
        tag: (row.commit.is_some() && row.snapshot.is_none()).then_some("not in the graph"),
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
    const FILE: u32 = 0o100644;
    let spec = FileDiffSpec {
        modes: [if old.is_some() { FILE } else { 0 }, FILE],
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

/// The menu of a row of the history pane. The working tree changes offer only *Show changes*;
/// *Show log* needs the commit in the snapshot.
fn history_menu(ui: &mut Ui, i: usize, row: &HistoryRow, changes: bool) -> Option<RowAction> {
    let mut action = None;
    let mut item = |ui: &mut Ui, enabled: bool, text: &str, why: &str, a: RowAction| {
        let r = ui
            .add_enabled(enabled, egui::Button::new(text))
            .on_disabled_hover_text(why);
        if r.clicked() {
            action = Some(a);
            ui.close();
        }
    };
    if row.commit.is_none() {
        let why = "Nothing is committed yet to compare with";
        item(ui, changes, "Show changes", why, RowAction::ShowChanges(i));
        return action;
    }
    item(ui, true, "Blame this revision", "", RowAction::Blame(i));
    let why = "The history before this commit isn't in this repository";
    item(ui, changes, "Show changes", why, RowAction::ShowChanges(i));
    let why = "Not in the graph: the commit isn't among the loaded history";
    item(
        ui,
        row.snapshot.is_some(),
        "Show log",
        why,
        RowAction::ShowLog(i),
    );
    crate::menu::separator(ui);
    item(ui, true, "Copy hash", "", RowAction::CopyHash(i));
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

/// Sizes of a blame's rows and columns, from the text size and the number of lines.
struct Metrics {
    row_h: f32,
    /// The width of a character of the text.
    char_w: f32,
    /// The widths of the hash column, the whole gutter, and the line numbers.
    hash_w: f32,
    gutter: f32,
    numbers: f32,
}

impl Metrics {
    fn new(ctx: &egui::Context, lines: usize, abbrev: usize) -> Metrics {
        let font = FontId::monospace(FONT_SIZE);
        let row_h = ctx.fonts_mut(|f| f.row_height(&font)).ceil() + 3.0;
        let char_w = ctx.fonts_mut(|f| f.glyph_width(&font, '0'));
        let hash_font = FontId::monospace(12.0);
        let hash_w = ctx.fonts_mut(|f| f.glyph_width(&hash_font, '0')) * abbrev as f32 + 2.0 * PAD;
        let digits = (lines.max(1) as f32).log10().floor() + 1.0;
        Metrics {
            row_h,
            char_w,
            hash_w,
            gutter: hash_w + AUTHOR + DATE,
            numbers: digits * char_w + 2.0 * PAD,
        }
    }
}

/// The toolbar's icon for a choice of moved lines.
fn moves_glyph(moves: Moves) -> glyphs::Glyph {
    match moves {
        Moves::Off => glyphs::MOVES_OFF,
        Moves::WithinFile => glyphs::MOVES_WITHIN_FILE,
        Moves::AcrossFiles => glyphs::MOVES_ACROSS_FILES,
    }
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

/// A tooltip shows this much of a commit message at most, as the graph's tooltips do.
const TIP_LINES: usize = 40;
const TIP_CHARS: usize = 4000;

/// What the tooltip over a row's subject shows: the commit's whole message, author and date,
/// as the log window's details show them. Until git has given the message (or if it failed),
/// the subject alone, with the date as the log shows it.
#[derive(Debug, PartialEq, Eq)]
struct MessageTip<'a> {
    /// `None` for the working tree changes.
    commit: Option<Oid>,
    author: String,
    date: &'a str,
    /// At most [`TIP_LINES`] lines and [`TIP_CHARS`] characters of it, and whether there was
    /// more.
    message: String,
    cut: bool,
    /// Why git could not give the message.
    error: Option<&'a str>,
}

impl<'a> MessageTip<'a> {
    /// For `row`, with what git gave for its commit so far (`None` while fetching).
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
        let mut shown = lines[..lines.len().min(TIP_LINES)].join("\n");
        let mut cut = lines.len() > TIP_LINES;
        if let Some((at, _)) = shown.char_indices().nth(TIP_CHARS) {
            shown.truncate(at);
            cut = true;
        }
        MessageTip {
            commit: row.commit,
            author: format!("{} <{}>", row.author_name, row.author_email),
            date,
            message: shown,
            cut,
            error,
        }
    }
}

/// The tooltip over a row's subject ([`MessageTip`]): author, date and hash, then the message
/// monospaced as the log window's details show it, the subject in the strong colour.
fn message_tip(ui: &mut Ui, tip: &MessageTip, abbrev: usize) {
    let Some(oid) = tip.commit else {
        ui.label("Changes in the working tree, not committed yet.");
        return;
    };
    ui.set_max_width(560.0);
    ui.label(&tip.author);
    ui.weak(format!("{}   {}", tip.date, oid.short(abbrev.max(12))));
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
    if tip.cut {
        ui.weak("…");
    }
    if let Some(e) = tip.error {
        ui.add_space(6.0);
        ui.colored_label(ui.visuals().error_fg_color, e);
    }
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
        requests.extend(self.blames.take_requests());
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

    /// Opens the blame of `path` at `rev` (a ref or hash prefix, or `WORKING_TREE`), with
    /// line `line` (from 1) chosen if given, for a script's `open blame:REV:PATH[:LINE]`.
    pub(super) fn open_named_blame(
        &mut self,
        repo: &Arc<Repo>,
        spec: &str,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let (rev, path) = spec.split_once(':').ok_or("expected COMMIT:PATH")?;
        let (path, line) = match path.rsplit_once(':') {
            Some((p, l)) if l.parse::<usize>().is_ok() => (p, l.parse::<usize>().ok()),
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
    use super::*;
    use parterre_core::file_history::LogCommit;
    use parterre_core::repo::Head;

    const A: &str = "c4275f7dbe7e820e3bb9a21c7d1cc1317657f2d4";
    const B: &str = "8e09b4551acb469f9df4ce58895b77e2e7e4190e";
    /// A commit between A and B that changed the file, none of whose lines remain.
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

    /// Runs one frame of the window's contents with `events`.
    fn frame(
        ctx: &egui::Context,
        w: &mut BlameWindow,
        events: Vec<egui::Event>,
    ) -> Vec<BlameRequest> {
        frame_with(ctx, w, &mut BlameWindowSettings::default(), events)
    }

    /// [`frame`], keeping what the window saves in `settings`.
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
        let top = 700.0 - INFO - BlameWindowSettings::default().history_height;
        pos2(500.0, top + HEADING + (i as f32 + 0.5) * ROW)
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
        let font = FontId::monospace(FONT_SIZE);
        let row_h = ctx.fonts_mut(|f| f.row_height(&font)).ceil() + 3.0;
        pos2(700.0, TOOLBAR + HEADER + (i as f32 + 0.5) * row_h)
    }

    /// Where column `col` of line `i`'s text starts, in `w`.
    fn char_at(ctx: &egui::Context, w: &BlameWindow, i: usize, col: usize) -> egui::Pos2 {
        let n = w.ready().unwrap().blame.lines.len();
        let m = Metrics::new(ctx, n, w.repo.abbrev_len);
        let x = m.gutter + m.numbers + PAD + col as f32 * m.char_w;
        pos2(x, at(ctx, i).y)
    }

    /// The middle of line `i`'s number, in `w`.
    fn number_at(ctx: &egui::Context, w: &BlameWindow, i: usize) -> egui::Pos2 {
        let n = w.ready().unwrap().blame.lines.len();
        let m = Metrics::new(ctx, n, w.repo.abbrev_len);
        pos2(m.gutter + m.numbers / 2.0, at(ctx, i).y)
    }

    /// The first and last line chosen.
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
        assert_eq!(w.chosen, Some(Some(Oid::from_hex(A).unwrap())));

        // A drag over the line numbers chooses whole lines.
        let (from, to) = (number_at(&ctx, &w, 1), number_at(&ctx, &w, 3));
        drag(&ctx, &mut w, from, to, Modifiers::NONE);
        assert_eq!(span(&w), Some((1, 3)));
        assert_eq!(w.selection.and_then(|s| s.chars()), None);
        // The drag started on B's line: B stays chosen over A's lines.
        assert_eq!(w.chosen, Some(Some(Oid::from_hex(B).unwrap())));
        // Copied as in the file: tabs kept, a newline after each line.
        assert_eq!(w.selected_text().as_deref(), Some("two\tx\nthree\nfour\n"));

        // So does one over the commit column.
        let (from, to) = (pos2(20.0, at(&ctx, 0).y), pos2(20.0, at(&ctx, 2).y));
        drag(&ctx, &mut w, from, to, Modifiers::NONE);
        assert_eq!(span(&w), Some((0, 2)));
        assert_eq!(w.selected_text().as_deref(), Some("one\ntwo\tx\nthree\n"));
        assert_eq!(w.chosen, Some(Some(Oid::from_hex(A).unwrap())));
    }

    #[test]
    fn a_drag_over_the_text_chooses_characters_and_copies_just_those() {
        let ctx = egui::Context::default();
        let mut w = window();
        frame(&ctx, &mut w, Vec::new());
        // From "tw|o    x" (the tab shown as spaces) to "thr|ee".
        let (from, to) = (char_at(&ctx, &w, 1, 2), char_at(&ctx, &w, 2, 3));
        drag(&ctx, &mut w, from, to, Modifiers::NONE);
        assert_eq!(w.selection.and_then(|s| s.chars()), Some(((1, 2), (2, 3))));
        // Copied as in the file, the tab kept.
        assert_eq!(w.selected_text().as_deref(), Some("o\tx\nthr"));
        // The chosen commit is that of the line the drag started on.
        assert_eq!(w.chosen, Some(Some(oid(B))));
        // The line menu's Copy lines still copies them whole.
        let mut requests = Vec::new();
        w.act(LineAction::CopyLines, &ctx, &mut requests);
        assert_eq!(w.lines_text().as_deref(), Some("two\tx\nthree\n"));

        // Backwards, within a line; then Shift+click on the text extends it.
        let (from, to) = (char_at(&ctx, &w, 3, 3), char_at(&ctx, &w, 3, 1));
        drag(&ctx, &mut w, from, to, Modifiers::NONE);
        assert_eq!(w.selected_text().as_deref(), Some("ou"));
        assert_eq!(w.chosen, Some(Some(oid(A))));
        let p = char_at(&ctx, &w, 2, 0);
        drag(&ctx, &mut w, p, p, Modifiers::SHIFT);
        assert_eq!(w.selected_text().as_deref(), Some("three\nfou"));
        assert_eq!(w.chosen, Some(Some(oid(A))));

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
        assert_eq!(w.selection.and_then(|s| s.chars()), None);
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
        assert_eq!(w.chosen, Some(Some(oid(B))));

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
        assert_eq!(w.chosen, Some(Some(oid(A))));
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
        // Shift+click on a line number chooses whole lines, from the text's line too.
        let p = number_at(&ctx, &w, 3);
        drag(&ctx, &mut w, p, p, Modifiers::SHIFT);
        assert_eq!(span(&w), Some((1, 3)));
        assert_eq!(w.selection.and_then(|s| s.chars()), None);
        assert_eq!(w.chosen, Some(Some(Oid::from_hex(B).unwrap())));

        // A right-click inside the chosen lines keeps them; outside, it chooses its line.
        let p = at(&ctx, 2);
        frame(
            &ctx,
            &mut w,
            vec![egui::Event::PointerMoved(p), button(p, true, true)],
        );
        frame(&ctx, &mut w, vec![button(p, false, true)]);
        assert_eq!(span(&w), Some((1, 3)));
        assert_eq!(w.chosen, Some(Some(Oid::from_hex(B).unwrap())));
        let p = at(&ctx, 0);
        frame(
            &ctx,
            &mut w,
            vec![egui::Event::PointerMoved(p), button(p, true, true)],
        );
        frame(&ctx, &mut w, vec![button(p, false, true)]);
        assert_eq!(span(&w), Some((0, 0)));
        assert_eq!(w.chosen, Some(Some(Oid::from_hex(A).unwrap())));
    }

    #[test]
    fn choosing_a_commit_chooses_its_first_line_without_scrolling() {
        let mut w = window();
        w.choose_commit(Some(Oid::from_hex(B).unwrap()));
        assert_eq!(span(&w), Some((1, 1)));
        assert_eq!(w.chosen, Some(Some(Oid::from_hex(B).unwrap())));
        assert_eq!(w.scroll_to, None);

        // A commit that owns no lines here is chosen, with no line.
        let gone = Oid::from_hex("0123456789abcdef0123456789abcdef01234567").unwrap();
        w.choose_commit(Some(gone));
        assert_eq!(span(&w), None);
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
            opened: 1,
            requests: Vec::new(),
            engine: Engine::InProcess,
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
        assert_eq!(w.chosen, Some(Some(oid(GONE))));
        assert_eq!(w.list.selected, Some(1));

        // Gone from the history, and lines 3 and 4 from the file.
        let out = [
            entry(A, 1, 1, 100, "", "one"),
            entry(A, 2, 2, 100, "", "two"),
        ]
        .concat();
        w.selection = Some(Selection::lines(1, 3));
        w.loaded(Ready::new(
            Blame::parse(out.as_bytes()).unwrap(),
            &repo,
            "",
            &Engine::InProcess,
            &Cancel::new(),
        ));
        w.listing = log(vec![logged(A, 100, &[])]);
        w.settle();
        assert_eq!(w.chosen, None);
        assert_eq!(span(&w), None);
        assert_eq!(w.list.selected, None);

        // Without a history, the chosen commit stays while it owns lines.
        w.choose_line(0);
        w.listing = Listing::Failed("no".into());
        w.settle();
        assert_eq!(w.chosen, Some(Some(oid(A))));
        w.chosen = Some(Some(oid(B)));
        w.settle();
        assert_eq!(w.chosen, None);
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
        assert_eq!(span(&w), Some((250, 250)));
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
        assert_eq!(w.list.selected, Some(2));
        assert_eq!(w.chosen, Some(Some(oid(A))));
        assert_eq!(span(&w), Some((0, 0)));
        assert_eq!(w.scroll, 0.0);

        // A commit that owns no lines: chosen, with no line, and greyed out without an age.
        let p = row_at(1);
        frame(
            &ctx,
            &mut w,
            vec![egui::Event::PointerMoved(p), button(p, true, false)],
        );
        frame(&ctx, &mut w, vec![button(p, false, false)]);
        assert_eq!(w.chosen, Some(Some(oid(GONE))));
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
        assert_eq!(w.chosen, Some(Some(oid(B))));
        assert_eq!(w.list.selected, Some(0));
        click(&ctx, &mut w, 3, Modifiers::NONE);
        assert_eq!(w.list.selected, Some(2));
    }

    #[test]
    fn up_and_down_step_through_the_rows_wherever_the_pointer_is() {
        let ctx = egui::Context::default();
        let mut w = listed_window();
        frame(&ctx, &mut w, Vec::new());
        // Over the text.
        let over_text = egui::Event::PointerMoved(at(&ctx, 1));
        frame(&ctx, &mut w, vec![over_text, key(Key::ArrowDown)]);
        assert_eq!(w.list.selected, Some(0));
        assert_eq!(w.chosen, Some(Some(oid(B))));
        frame(&ctx, &mut w, vec![key(Key::ArrowDown)]);
        assert_eq!((w.list.selected, span(&w)), (Some(1), None));
        frame(&ctx, &mut w, vec![key(Key::ArrowDown)]);
        frame(&ctx, &mut w, vec![key(Key::ArrowDown)]);
        assert_eq!(w.list.selected, Some(2));
        assert_eq!(w.chosen, Some(Some(oid(A))));
        assert_eq!(span(&w), Some((0, 0)));
        frame(&ctx, &mut w, vec![key(Key::ArrowUp)]);
        assert_eq!(w.chosen, Some(Some(oid(GONE))));
        assert_eq!(w.scroll, 0.0);

        // Not while the pane is hidden.
        w.show_history = false;
        frame(&ctx, &mut w, vec![key(Key::ArrowUp)]);
        assert_eq!(w.list.selected, Some(1));
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
        let middle = 700.0 - INFO - before - DIVIDER / 2.0;
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
        let first = w.cancel.clone();
        w.reload(&ctx);
        assert!(first.is_cancelled());
        assert!(matches!(w.listing, Listing::Running(_)));
        let second = w.cancel.clone();
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
        clicked(&ctx, &mut w, &mut settings, pos2(177.0, TOOLBAR / 2.0));
        assert_eq!(w.options.moves, Moves::AcrossFiles);
        assert_eq!(settings.moves, Moves::AcrossFiles);

        // The history icon, at the right end.
        assert!(w.show_history);
        clicked(&ctx, &mut w, &mut settings, pos2(975.0, TOOLBAR / 2.0));
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
        let p = pos2(211.0, TOOLBAR / 2.0);
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

    /// The places found, as line and the text found there.
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
        let chosen = w.chosen;
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
        assert_eq!(w.chosen, chosen);
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
        assert_eq!(w.find.last, "four");
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
        assert_eq!(w.selection.and_then(|s| s.chars()), None);
        assert_eq!(w.chosen, Some(Some(oid(B))));
        assert_eq!(w.list.selected, Some(0));
        // In view already: the text stays where it is.
        frame(&ctx, &mut w, Vec::new());
        assert_eq!(w.scroll, 0.0);

        // Again: the popup opens empty, and the next line goes to A's row.
        go_to(&ctx, &mut w, "4");
        assert_eq!(w.go_to.text, "4");
        frame(&ctx, &mut w, vec![key(Key::Enter)]);
        assert_eq!(span(&w), Some((3, 3)));
        assert_eq!(w.chosen, Some(Some(oid(A))));
        assert_eq!(w.list.selected, Some(2));
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
        let height = 700.0 - TOOLBAR - HEADER - INFO;
        let above = (height / row_h / 3.0).floor() as usize;
        assert_eq!(w.top, 249 - above);

        // A line in view: chosen, without scrolling.
        let (top, scroll) = (w.top, w.scroll);
        go_to(&ctx, &mut w, &(top + 3).to_string());
        frame(&ctx, &mut w, vec![key(Key::Enter)]);
        frame(&ctx, &mut w, Vec::new());
        assert_eq!(span(&w), Some((top + 2, top + 2)));
        assert_eq!(w.scroll, scroll);
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
        assert!(!w.details.cache.contains_key(&oid(A)));
        // Over row 0's subject: B's message is asked for.
        frame(&ctx, &mut w, vec![egui::Event::PointerMoved(row_at(0))]);
        for _ in 0..3 {
            frame(&ctx, &mut w, Vec::new());
        }
        assert!(w.details.cache.contains_key(&oid(B)));
        assert_eq!(w.details.cache.len(), 1);
    }

    #[test]
    fn a_subject_tip_shows_the_subject_until_the_whole_message_is_there() {
        let w = listed_window();
        let row = &w.history().unwrap().history.rows[0];
        let fetching = MessageTip::new(row, None);
        assert_eq!(fetching.message, "s200");
        assert_eq!(fetching.author, "A B <a@b>");
        assert_eq!((fetching.cut, fetching.error), (false, None));

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
        assert!(!tip.cut);

        // A long message is cut, and says so.
        let long: String = (1..=50).map(|i| format!("line {i}\n")).collect();
        let fetched = details(&long);
        let tip = MessageTip::new(row, Some(&fetched));
        assert_eq!(tip.message.lines().count(), TIP_LINES);
        assert!(tip.cut);

        // Git failed: the subject, and why.
        let failed = Err("no such commit".to_owned());
        let tip = MessageTip::new(row, Some(&failed));
        assert_eq!(tip.message, "s200");
        assert_eq!(tip.error, Some("no such commit"));
    }
}
