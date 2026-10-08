//! The log window: the commits of a log query one per row, the selected commit's details, and
//! the files it changed. A window of its own (an immediate viewport, like the settings), one at
//! a time; Show log on other nodes replaces its contents. Decided in #27, #28 and #29; the
//! prototype is on the branch `prototype/log-window`.
//!
//! The commit list has a graph column of lanes, as TortoiseGit's log does ([`LogGraph`]), and
//! the header has TortoiseGit's walk options ([`LogOptions`]); both chosen with the prototype on
//! the branch `prototype/log-graph`.
//!
//! The window is handed a [`LogQuery`] and knows nothing about the revision graph. Its three panes are
//! separate functions that a [`LogLayout`] arranges; the layout is picked in the header or in
//! the settings, and it and the dividers of each layout are saved with the settings.
//!
//! Find (`Ctrl+F`), in the middle of the header, looks for text in the subjects and for the
//! start of a hash ([`parterre_core::log::find`]). Going to a place selects its commit, as the
//! main window's find does, so the details follow; Enter and `F3` go to the next, round the
//! end. Every place is highlighted in the list. Esc in the find field leaves it; elsewhere it
//! closes the window.
//!
//! Ctrl+click adds commits to the selection, Shift+click selects a range and Ctrl+A every
//! commit. The menu of a selected one compares the commit selected first with the one clicked
//! last, as the graph's Compare revisions does (#127), and cherry-picks the selection (#202).
//! The details and files follow the one clicked last; the mark for comparison is left alone.

use std::sync::Arc;

use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{
    self, Color32, CornerRadius, CursorIcon, FontId, Galley, Id, Key, Margin, Modifiers, Rangef,
    Rect, RichText, ScrollArea, Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2,
};
use parterre_core::blame::BlameSpec;
use parterre_core::columns::ColumnWidths;
use parterre_core::file_diff::{FileDiffSpec, Rev};
use parterre_core::find;
use parterre_core::glyphs::{self, Glyph};
use parterre_core::log::{self, LogOptions, LogOrder, LogQuery};
use parterre_core::log_graph::LogGraph;
use parterre_core::log_layout::LogLayout;
use parterre_core::revgraph::GraphOptions;
use parterre_core::text::{find_urls, thousands};
use parterre_core::{Commit, CommitIx, GitRef, Label, Oid, RefKind, Repo, Worktree};

use super::commit_table::{CommitList, CommitTable, Row, Select};
use super::compare_window::CompareRequest;
use super::file_table::{DiffQueue, FileTable, Lister, Listing};
use super::{Details, ParterreApp};
use crate::keys;
use crate::settings::LogWindowSettings;
use crate::text_size;
use crate::theme::{Palette, text_on};
use crate::widgets;

/// Height of a table's column headings.
pub(super) const HEADING: f32 = 26.0;
/// Thickness of the draggable dividers between panes.
pub(super) const DIVIDER: f32 = 6.0;
pub(super) const CELL_PAD: f32 = 8.0;
/// How long a copy button shows a check mark.
const COPIED_SECONDS: f64 = 1.2;

fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("log")
}

/// The icon of a layout in the pickers: the arrangement of its panes.
pub fn layout_glyph(layout: LogLayout) -> Glyph {
    match layout {
        LogLayout::Stacked => glyphs::LAYOUT_STACKED,
        LogLayout::SideBySide => glyphs::LAYOUT_SIDE_BY_SIDE,
        LogLayout::DetailsBelow => glyphs::LAYOUT_DETAILS_BELOW,
        LogLayout::FilesRight => glyphs::LAYOUT_FILES_RIGHT,
    }
}

/// The log window's state that outlives its contents: the changed files' sort and filter
/// (kept as you move between commits and logs), and caches. The layout and its dividers are
/// in the settings ([`LogWindowSettings`]).
#[derive(Debug, Default)]
pub struct LogWindow {
    /// What the window shows; `None` while it is closed.
    view: Option<LogView>,
    table: FileTable,
    files: Lister<Oid, Listing>,
    /// The size the window opened with. The viewport builder must not change while the window
    /// is open, or egui would resize it.
    size: Vec2,
    /// The theme last given to the window's title bar.
    title_theme: Option<egui::SystemTheme>,
    /// What was copied last, and when (for the check mark on its copy button).
    copied: Option<(Copied, f64)>,
    /// How many logs were opened, to give each its own scroll positions.
    opened: u64,
    /// Diff windows to open, for the app to take.
    diffs: DiffQueue<(Arc<Repo>, FileDiffSpec)>,
    /// Marks and comparisons asked for, for the app to take.
    requests: Vec<CompareRequest>,
    branch_requests: Vec<super::branches::Request>,
    /// Blame windows asked for, for the app to take.
    blames: Vec<(Arc<Repo>, BlameSpec)>,
    /// Find in the list, kept when Show log replaces the contents.
    find: Find,
}

/// Find in the log (Ctrl+F): the query and the rows it occurs in. The place gone to is the
/// selected row, when it is one of them.
#[derive(Debug, Default)]
struct Find {
    query: String,
    /// The last query searched for, which Ctrl+F offers again after Esc cleared the field.
    last: String,
    matches: Vec<usize>,
    /// Focus the field and select the query in the next frame.
    focus: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Copied {
    Hash,
    Email,
}

/// The commits of one log query.
#[derive(Debug)]
struct LogView {
    /// Tells logs apart, so that a new one starts scrolled to the top. Kept by F5.
    id: u64,
    repo: Arc<Repo>,
    /// [`Repo::refs_by_commit`] of `repo`.
    refs: Vec<Vec<usize>>,
    query: LogQuery,
    /// The walk options the list was made with.
    options: LogOptions,
    /// Safety warnings pin both membership and the snapshot, even after the last ref is gone.
    exact: Option<Vec<CommitIx>>,
    commits: Vec<CommitIx>,
    /// The graph column's lanes, a row for each of `commits`.
    graph: LogGraph,
    /// The selected row (an index into `commits`) and the scroll position.
    list: CommitList,
}

impl LogView {
    fn new(id: u64, repo: Arc<Repo>, query: LogQuery, options: LogOptions) -> LogView {
        let list = query.list(&repo, &options);
        LogView {
            id,
            refs: repo.refs_by_commit(),
            repo,
            query,
            options,
            exact: None,
            list: {
                let mut first = CommitList::default();
                first.select((!list.commits.is_empty()).then_some(0));
                first
            },
            graph: LogGraph::new(&list),
            commits: list.commits,
        }
    }

    fn selected_commit(&self) -> Option<CommitIx> {
        self.commits.get(self.list.selected?).copied()
    }

    /// Re-runs the query on a newly loaded snapshot, keeping the selected commit if it is still
    /// listed. Commits of the query that are gone from the snapshot are dropped from it.
    fn reload(&mut self, repo: Arc<Repo>) {
        if self.exact.is_some() {
            return;
        }
        self.rebuild(repo, self.options);
    }

    /// After a revert: walks from the new commit `to` where the query walked from the branch's
    /// old tip `from`, and selects it. False when the query doesn't walk from `from`.
    fn follow(&mut self, from: Oid, to: Oid) -> bool {
        if self.exact.is_some() {
            return false;
        }
        let (Some(from), Some(to)) = (self.repo.lookup(&from), self.repo.lookup(&to)) else {
            return false;
        };
        let Some(tip) = self.query.tips.iter().position(|&t| t == from) else {
            return false;
        };
        self.query.tips[tip] = to;
        self.rebuild(self.repo.clone(), self.options);
        let row = self.commits.iter().position(|&c| c == to);
        self.list.select(row);
        self.list.reveal = true;
        row.is_some()
    }

    /// Re-runs the query with other walk options, keeping the selected commit in view if it is
    /// still listed.
    fn set_options(&mut self, options: LogOptions) {
        if self.exact.is_some() {
            self.options = options;
            return;
        }
        self.rebuild(self.repo.clone(), options);
        self.list.reveal = true;
    }

    fn pin_exact(&mut self, commits: Vec<CommitIx>) {
        let wanted: std::collections::HashSet<_> = commits.iter().copied().collect();
        let options = LogOptions {
            order: self.options.order,
            ..LogOptions::default()
        };
        let all = self.query.list(&self.repo, &options);
        let mut list = parterre_core::log::LogList {
            commits: Vec::new(),
            parents: Vec::new(),
            outside: Vec::new(),
        };
        for (row, &c) in all.commits.iter().enumerate() {
            if wanted.contains(&c) {
                list.commits.push(c);
                list.parents.push(
                    all.parents[row]
                        .iter()
                        .copied()
                        .filter(|p| wanted.contains(p))
                        .collect(),
                );
                list.outside
                    .push(all.outside[row] || all.parents[row].iter().any(|p| !wanted.contains(p)));
            }
        }
        self.commits = list.commits.clone();
        self.graph = LogGraph::new(&list);
        self.list.select((!self.commits.is_empty()).then_some(0));
        self.exact = Some(commits);
    }

    /// Re-runs the query on `repo` with `options`, keeping the selected commit if it is still
    /// listed, and the scroll position.
    fn rebuild(&mut self, repo: Arc<Repo>, options: LogOptions) {
        let oid = |i: usize| self.commits.get(i).map(|&c| self.repo.commit(c).oid);
        let selected = self.list.selected.and_then(oid);
        let anchor = self.list.anchor.and_then(oid);
        let many: Vec<Oid> = self.list.many.iter().filter_map(|&i| oid(i)).collect();
        let map = |commits: &[CommitIx]| -> Vec<CommitIx> {
            commits
                .iter()
                .filter_map(|&c| repo.lookup(&self.repo.commit(c).oid))
                .collect()
        };
        let mut query = self.query.clone();
        query.tips = map(&self.query.tips);
        query.exclude = map(&self.query.exclude);
        let (scroll, height) = (self.list.scroll, self.list.height);
        let widths = std::mem::take(&mut self.list.widths);
        *self = LogView::new(self.id, repo, query, options);
        (self.list.scroll, self.list.height) = (scroll, height);
        self.list.widths = widths;
        // The selection, by commit, where its commits are still listed.
        let rows: std::collections::HashMap<Oid, usize> = self
            .commits
            .iter()
            .enumerate()
            .map(|(i, &c)| (self.repo.commit(c).oid, i))
            .collect();
        if let Some(&i) = selected.and_then(|o| rows.get(&o)) {
            self.list.select(Some(i));
            self.list.reveal = false;
            self.list.anchor = anchor.and_then(|o| rows.get(&o).copied()).or(Some(i));
            self.list
                .many
                .extend(many.iter().filter_map(|o| rows.get(o)));
        }
    }
}

/// The three panes a layout arranges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Commits,
    Details,
    Files,
}

/// What the panes need from the app besides the log window's own state.
struct Env<'a> {
    details: &'a mut Details,
    /// The open worktree, where git is asked about commits. A pinned view keeps an older
    /// snapshot, perhaps of a worktree since deleted; any worktree of the repository answers.
    open: Option<std::path::PathBuf>,
    palette: Palette,
    graph: &'a GraphOptions,
    /// The layout and the dividers.
    settings: &'a mut LogWindowSettings,
    /// The commit marked for comparison, and its name.
    marked: Option<&'a (Oid, String)>,
    branches: Option<&'a parterre_core::branches::Catalog>,
    branch_busy: bool,
}

/// Where a layout puts the panes and dividers in the window body.
#[derive(Clone, Copy, Debug)]
struct Arrangement {
    /// Commits, details, changed files.
    panes: [Rect; 3],
    bars: [Bar; 2],
}

/// A draggable divider, and how a pointer position turns into its fraction in
/// [`Dividers`](parterre_core::log_layout::Dividers).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Bar {
    pub(super) rect: Rect,
    /// Dragged sideways: a vertical bar between panes side by side.
    pub(super) vertical: bool,
    /// Where the bar's middle is (along x for a vertical bar, else y) at fraction 0, and the
    /// room its fraction is of.
    pub(super) origin: f32,
    pub(super) room: f32,
}

impl Bar {
    /// The fraction that puts the bar's middle at `at`.
    fn fraction(&self, at: f32) -> f32 {
        (at - self.origin) / self.room
    }
}

/// A range cut in two by a divider at `fraction` of the room the two parts share.
struct Cut {
    first: Rangef,
    bar: Rangef,
    second: Rangef,
    origin: f32,
    room: f32,
}

fn cut(range: Rangef, fraction: f32) -> Cut {
    let room = (range.span() - DIVIDER).max(1.0);
    let at = range.min + (room * fraction).round();
    Cut {
        first: Rangef::new(range.min, at),
        bar: Rangef::new(at, at + DIVIDER),
        second: Rangef::new(at + DIVIDER, range.max.max(at + DIVIDER)),
        origin: range.min + DIVIDER / 2.0,
        room,
    }
}

/// Where `layout`, with its dividers at `fractions`, puts the panes and dividers in `body`.
fn arrange(layout: LogLayout, [a, b]: [f32; 2], body: Rect) -> Arrangement {
    let rect = Rect::from_x_y_ranges;
    let (xs, ys) = (body.x_range(), body.y_range());
    let bar = |rect: Rect, vertical: bool, c: &Cut| Bar {
        rect,
        vertical,
        origin: c.origin,
        room: c.room,
    };
    match layout {
        LogLayout::Stacked => {
            // Both dividers share the height, so the files' share is what the others leave.
            let room = (body.height() - 2.0 * DIVIDER).max(1.0);
            let y1 = body.top() + (room * a).round();
            let y2 = y1 + DIVIDER + (room * b).round();
            let stacked_bar = |y: f32, origin: f32| Bar {
                rect: rect(xs, Rangef::new(y, y + DIVIDER)),
                vertical: false,
                origin,
                room,
            };
            Arrangement {
                panes: [
                    rect(xs, Rangef::new(body.top(), y1)),
                    rect(xs, Rangef::new(y1 + DIVIDER, y2)),
                    rect(
                        xs,
                        Rangef::new(y2 + DIVIDER, body.bottom().max(y2 + DIVIDER)),
                    ),
                ],
                bars: [
                    stacked_bar(y1, body.top() + DIVIDER / 2.0),
                    stacked_bar(y2, body.top() + 1.5 * DIVIDER),
                ],
            }
        }
        LogLayout::SideBySide => {
            let across = cut(xs, a);
            let right = cut(ys, b);
            Arrangement {
                panes: [
                    rect(across.first, ys),
                    rect(across.second, right.first),
                    rect(across.second, right.second),
                ],
                bars: [
                    bar(rect(across.bar, ys), true, &across),
                    bar(rect(across.second, right.bar), false, &right),
                ],
            }
        }
        LogLayout::DetailsBelow => {
            let down = cut(ys, a);
            let below = cut(xs, b);
            Arrangement {
                panes: [
                    rect(xs, down.first),
                    rect(below.first, down.second),
                    rect(below.second, down.second),
                ],
                bars: [
                    bar(rect(xs, down.bar), false, &down),
                    bar(rect(below.bar, down.second), true, &below),
                ],
            }
        }
        LogLayout::FilesRight => {
            let across = cut(xs, a);
            let left = cut(ys, b);
            Arrangement {
                panes: [
                    rect(across.first, left.first),
                    rect(across.first, left.second),
                    rect(across.second, ys),
                ],
                bars: [
                    bar(rect(across.bar, ys), true, &across),
                    bar(rect(across.first, left.bar), false, &left),
                ],
            }
        }
    }
}

/// Colours of the log window beyond egui's visuals, after the prototype.
pub(super) struct Colors {
    /// Background of the panes (the chrome around them is the panel colour).
    pub(super) pane: Color32,
    pub(super) stripe: Color32,
    pub(super) hover: Color32,
    pub(super) line: Color32,
    pub(super) selected_bg: Color32,
    pub(super) selected_fg: Color32,
    /// The range label.
    pub(super) link: Color32,
    pub(super) added: Color32,
    pub(super) removed: Color32,
    pub(super) renamed: Color32,
    /// A modified file's `M`.
    pub(super) modified: Color32,
    /// Behind where a find query is.
    pub(super) found: Color32,
    /// The graph column's lanes, in turn.
    pub(super) lanes: [Color32; 8],
}

pub(super) fn colors(ui: &Ui) -> Colors {
    let t = widgets::tones(ui);
    if ui.visuals().dark_mode {
        Colors {
            pane: Color32::from_gray(22),
            stripe: Color32::from_white_alpha(5),
            hover: Color32::from_white_alpha(13),
            line: Color32::from_white_alpha(23),
            selected_bg: t.on_bg,
            selected_fg: Color32::from_rgb(0xcf, 0xe5, 0xff),
            link: t.on_fg,
            added: Color32::from_rgb(0x7b, 0xd8, 0x8f),
            removed: Color32::from_rgb(0xff, 0x8a, 0x80),
            renamed: Color32::from_rgb(0xd1, 0xa5, 0xff),
            modified: Color32::from_rgb(0xe0, 0xa8, 0x40),
            found: Color32::from_rgba_unmultiplied(0xd0, 0x9a, 0x1c, 80),
            lanes: [
                Color32::from_rgb(0xef, 0x53, 0x50),
                Color32::from_rgb(0x42, 0xa5, 0xf5),
                Color32::from_rgb(0x66, 0xbb, 0x6a),
                Color32::from_rgb(0xff, 0xa7, 0x26),
                Color32::from_rgb(0xab, 0x47, 0xbc),
                Color32::from_rgb(0x26, 0xc6, 0xda),
                Color32::from_rgb(0xec, 0x40, 0x7a),
                Color32::from_rgb(0xa1, 0x88, 0x7f),
            ],
        }
    } else {
        Colors {
            pane: Color32::WHITE,
            stripe: Color32::from_black_alpha(5),
            hover: Color32::from_black_alpha(11),
            line: Color32::from_black_alpha(26),
            selected_bg: t.on_bg,
            selected_fg: Color32::from_rgb(0x0b, 0x3d, 0x7a),
            link: t.on_fg,
            added: Color32::from_rgb(0x2e, 0x7d, 0x32),
            removed: Color32::from_rgb(0xc6, 0x28, 0x28),
            renamed: Color32::from_rgb(0x6a, 0x1b, 0x9a),
            modified: Color32::from_rgb(0xa8, 0x6a, 0x00),
            found: Color32::from_rgba_unmultiplied(0xff, 0xcc, 0x33, 130),
            lanes: [
                Color32::from_rgb(0xd3, 0x2f, 0x2f),
                Color32::from_rgb(0x19, 0x76, 0xd2),
                Color32::from_rgb(0x38, 0x8e, 0x3c),
                Color32::from_rgb(0xf5, 0x7c, 0x00),
                Color32::from_rgb(0x7b, 0x1f, 0xa2),
                Color32::from_rgb(0x00, 0x83, 0x8f),
                Color32::from_rgb(0xc2, 0x18, 0x5b),
                Color32::from_rgb(0x5d, 0x40, 0x37),
            ],
        }
    }
}

impl LogWindow {
    /// Shows `query` on `repo`, in place of what the window showed before.
    fn open(&mut self, repo: Arc<Repo>, query: LogQuery, options: LogOptions, size: Vec2) {
        // The column widths picked last as long as the window is open.
        let widths = match self.view.take() {
            Some(view) => view.list.widths,
            None => {
                self.size = size;
                self.table.widths = ColumnWidths::default();
                ColumnWidths::default()
            }
        };
        self.opened += 1;
        let mut view = LogView::new(self.opened, repo, query, options);
        view.list.widths = widths;
        self.view = Some(view);
        self.refind();
    }

    pub fn is_open(&self) -> bool {
        self.view.is_some()
    }

    /// Closes the window and forgets the changed files listed for the repository it showed.
    pub fn close(&mut self) {
        self.view = None;
        self.files = Lister::default();
        self.table.clear_selection();
        self.diffs.cancel();
    }

    /// The diff windows asked for since the last call.
    pub fn take_diff_requests(&mut self) -> Vec<(Arc<Repo>, FileDiffSpec)> {
        self.diffs.take()
    }

    /// The blame windows asked for since the last call.
    pub fn take_blame_requests(&mut self) -> Vec<(Arc<Repo>, BlameSpec)> {
        std::mem::take(&mut self.blames)
    }

    /// The marks and comparisons asked for since the last call.
    pub fn take_compare_requests(&mut self) -> Vec<CompareRequest> {
        std::mem::take(&mut self.requests)
    }

    pub fn take_branch_requests(&mut self) -> Vec<super::branches::Request> {
        std::mem::take(&mut self.branch_requests)
    }

    /// After F5: re-runs the query on the new snapshot.
    pub fn reload(&mut self, repo: &Arc<Repo>) {
        if let Some(view) = &mut self.view {
            view.reload(repo.clone());
        }
        self.refind();
    }

    /// After a revert, on the reloaded snapshot: the log that walked from the branch's old tip
    /// `from` walks from the new commit `to`, selected. False when there's no such log.
    pub fn follow(&mut self, from: Oid, to: Oid) -> bool {
        let followed = self.view.as_mut().is_some_and(|v| v.follow(from, to));
        if followed {
            self.refind();
        }
        followed
    }

    /// The window's title: `<repo> – Log`.
    fn title(&self) -> String {
        let name = self
            .view
            .as_ref()
            .map(|v| v.repo.display_name())
            .unwrap_or_default();
        format!("{name} – Log")
    }

    fn find_id() -> Id {
        Id::new("log-find")
    }

    /// Ctrl+F: focuses the find field with the last query, selected so that typing replaces
    /// it.
    fn open_find(&mut self) {
        if self.find.query.is_empty() && !self.find.last.is_empty() {
            self.find.query = self.find.last.clone();
            self.query_changed();
        }
        self.find.focus = true;
    }

    /// Esc in the find field, or its clear button: empties it, and leaves it.
    fn close_find(&mut self, ctx: &egui::Context) {
        self.find.query.clear();
        self.refind();
        ctx.memory_mut(|m| m.surrender_focus(Self::find_id()));
    }

    /// Finds the query in the list.
    fn refind(&mut self) {
        self.find.matches = match &self.view {
            Some(v) => log::find(&v.repo, &v.commits, &self.find.query),
            None => Vec::new(),
        };
    }

    /// The place gone to: the selected row, if the query is in it (an index into the matches).
    fn find_current(&self) -> Option<usize> {
        let selected = self.view.as_ref()?.list.selected?;
        self.find.matches.binary_search(&selected).ok()
    }

    /// The query was typed: selects the first row it is in from the selected one on (which
    /// stays selected while it still matches), round the end.
    fn query_changed(&mut self) {
        self.refind();
        if !self.find.query.trim().is_empty() {
            self.find.last = self.find.query.clone();
        }
        let Some(view) = &mut self.view else { return };
        let from = view.list.selected.unwrap_or(0);
        if let Some(k) = find::first_from(&self.find.matches, from) {
            view.list.select(Some(self.find.matches[k]));
        }
    }

    /// Enter, F3 (`forward`), Shift+Enter, Shift+F3: selects the next or previous row the
    /// query is in, round the ends.
    fn find_step(&mut self, forward: bool) {
        let current = self.find_current();
        let Some(view) = &mut self.view else { return };
        let from = view.list.selected.unwrap_or(0);
        if let Some(k) = find::step(&self.find.matches, current, forward, from) {
            view.list.select(Some(self.find.matches[k]));
        }
    }

    /// The find field, with how many rows the query was found in.
    fn find_field(&mut self, ui: &mut Ui, width: f32) {
        let n = self.find.matches.len();
        let count = match (self.find_current(), n) {
            (_, 0) => "No matches".to_owned(),
            (Some(c), n) => format!("{} of {n}", c + 1),
            (None, 1) => "1 match".to_owned(),
            (None, n) => format!("{n} matches"),
        };
        let focus = std::mem::take(&mut self.find.focus);
        let find = widgets::Find {
            id: Self::find_id(),
            width,
            hint: "Find subjects, hashes",
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

    /// Ctrl+F finds, F3 and Shift+F3 (⌘G and ⇧⌘G on macOS) go to the next and previous place,
    /// and Esc in the find field leaves it; elsewhere Esc (⌘W on macOS) closes. The arrow keys, Page Up/Down, Home and End move
    /// the selection, only while no text field has the keyboard.
    fn handle_keys(&mut self, ui: &Ui) {
        let id = Self::find_id();
        // egui drops the focus on Esc before the frame starts.
        let in_find = ui.memory(|m| m.has_focus(id) || m.had_focus_last_frame(id));
        if ui.input_mut(|i| keys::FIND.consume(i)) {
            self.open_find();
        }
        // The previous one first: egui takes F3 for Shift+F3 too.
        let (previous, next) = ui.input_mut(|i| {
            (
                keys::find_previous().consume(i),
                keys::find_next().consume(i),
            )
        });
        if previous || next {
            self.find_step(next);
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
        let Some(view) = &mut self.view else { return };
        let n = view.commits.len();
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
            view.list.select_all(n);
        }
        let page = view.list.page();
        let target = ui.input_mut(|i| {
            let mut key = |k: Key| i.consume_key(Modifiers::NONE, k);
            let current = view.list.selected.unwrap_or(0);
            if key(Key::ArrowDown) {
                Some(current + 1)
            } else if key(Key::ArrowUp) {
                Some(current.saturating_sub(1))
            } else if key(Key::PageDown) {
                Some(current + page)
            } else if key(Key::PageUp) {
                Some(current.saturating_sub(page))
            } else if key(Key::Home) {
                Some(0)
            } else if key(Key::End) {
                Some(usize::MAX)
            } else {
                None
            }
        });
        if let Some(target) = target
            && n > 0
        {
            view.list.select(Some(target.min(n - 1)));
        }
        if ui.input_mut(|i| keys::close_window().consume(i)) {
            self.view = None;
        }
    }

    /// The header and the panes, in the chosen layout.
    fn contents(&mut self, ui: &mut Ui, env: &mut Env) {
        let c = colors(ui);
        self.header(ui, &c, env);
        if let Some(view) = &mut self.view
            && view.options != env.settings.options
        {
            view.set_options(env.settings.options);
            self.refind();
        }
        let body = ui.available_rect_before_wrap();
        self.body(ui, body, env);
        self.diffs.confirm_many(ui, Id::new("log-many-diffs"));
    }

    /// The range at the top left, as TortoiseGit shows it; find in the middle; on the right the
    /// commit count, the walk options, the layout picker and the button that resets the
    /// layout's dividers.
    fn header(&mut self, ui: &mut Ui, c: &Colors, env: &mut Env) {
        let Some(view) = &self.view else { return };
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
        let mut tools = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(8.0, 0.0)))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        tools.spacing_mut().item_spacing.x = 4.0;
        layout_tools(&mut tools, env.settings);
        tools.add_space(12.0);
        tools.add_enabled_ui(view.exact.is_none(), |ui| {
            walk_tools(ui, &mut env.settings.options)
        });
        if let (Some(&from), Some(&to)) = (view.query.exclude.first(), view.query.tips.first()) {
            tools.add_space(8.0);
            let compare = widgets::tip_explained(
                widgets::text_button(&mut tools, "Compare files"),
                "Compare files",
                "",
                "List the files that differ between the two ends of the range, and open their \
                 diffs.",
            );
            if compare.clicked() {
                let oid = |c: CommitIx| view.repo.commit(c).oid;
                self.requests
                    .push(CompareRequest::Compare(oid(from), oid(to)));
            }
        }
        let tools_left = tools.min_rect().left();
        let painter = ui.painter();
        painter.hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1.0, c.line),
        );
        let mut label = view
            .query
            .label(&view.repo, &view.refs, |r| env.graph.shows(r.kind));
        if view.exact.is_some() {
            label.from = None;
            let count = view.commits.len();
            label.to = format!(
                "{count} commit{} at risk",
                if count == 1 { "" } else { "s" }
            );
        }
        let font = FontId::proportional(13.5);
        let weak = ui.visuals().weak_text_color();
        let mut job = LayoutJob::default();
        let format = |color| TextFormat::simple(font.clone(), color);
        if let Some(from) = &label.from {
            job.append(from, 0.0, format(c.link));
            job.append("..", 2.0, format(weak));
            job.append(&label.to, 2.0, format(c.link));
        } else {
            job.append(&label.to, 0.0, format(c.link));
        }
        let n = view.commits.len();
        let count = format!("{} commit{}", thousands(n), if n == 1 { "" } else { "s" });
        let count = painter.layout_no_wrap(count, FontId::proportional(13.0), weak);
        let count_left = tools_left - 14.0 - count.size().x;
        // Find, centred in the window if it fits between the range (given some room) and the
        // count; else in the middle of the room there is, squeezed.
        let room = Rangef::new(rect.left() + 12.0 + 140.0, count_left - 14.0);
        let width = (room.span() - 16.0).clamp(60.0, 380.0);
        let (lo, hi) = (room.min + 8.0, room.max - 8.0 - width);
        let find_left = if lo <= hi {
            (rect.center().x - width / 2.0).clamp(lo, hi)
        } else {
            room.center() - width / 2.0
        };
        job.wrap = TextWrapping {
            max_width: (find_left - rect.left() - 36.0).max(0.0),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let galley = painter.layout_job(job);
        painter.galley(
            pos2(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0),
            galley,
            weak,
        );
        painter.galley(
            pos2(count_left, rect.center().y - count.size().y / 2.0),
            count,
            weak,
        );
        let at = Rect::from_center_size(
            pos2(find_left + width / 2.0, rect.center().y),
            vec2(width, widgets::BUTTON),
        );
        let layout = egui::Layout::left_to_right(egui::Align::Center);
        ui.scope_builder(UiBuilder::new().max_rect(at).layout(layout), |ui| {
            self.find_field(ui, width);
        });
    }

    /// The panes where the layout puts them, and the dividers between them.
    fn body(&mut self, ui: &mut Ui, body: Rect, env: &mut Env) {
        let layout = env.settings.layout;
        let arrangement = arrange(layout, env.settings.dividers.of(layout), body);
        let panes = [Pane::Commits, Pane::Details, Pane::Files];
        for (pane, rect) in panes.into_iter().zip(arrangement.panes) {
            self.pane(ui, pane, rect, env);
        }
        for (which, bar) in arrangement.bars.iter().enumerate() {
            let id = Id::new(("log-divider", layout, which));
            if let Some(at) = divider(ui, id, bar) {
                env.settings.dividers.set(layout, which, bar.fraction(at));
            }
        }
        ui.allocate_rect(body, Sense::hover());
    }

    /// Draws `pane` in `rect`.
    fn pane(&mut self, ui: &mut Ui, pane: Pane, rect: Rect, env: &mut Env) {
        let c = colors(ui);
        ui.painter().rect_filled(rect, 0.0, c.pane);
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .id_salt(("log-pane", pane as u8)),
        );
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        match pane {
            Pane::Commits => self.commits_pane(&mut child, env, &c),
            Pane::Details => self.details_pane(&mut child, env, &c),
            Pane::Files => self.files_pane(&mut child, env, &c),
        }
    }

    /// The commit list: the graph, short hash, ref badges and subject, author, date.
    fn commits_pane(&mut self, ui: &mut Ui, env: &mut Env, c: &Colors) {
        let Some(view) = &mut self.view else { return };
        let LogView {
            id,
            repo,
            refs,
            commits,
            graph,
            list,
            ..
        } = view;
        let table = CommitTable {
            id: Id::new(("log-commits", *id)),
            rows: commits.len(),
            graph,
            abbrev_len: repo.abbrev_len,
            palette: &env.palette,
            select: Select::Many,
            icons: false,
        };
        let head = repo.head_commit().map(|c| repo.commit(c).oid);
        let marked = env.marked;
        let branches = env.branches;
        let branch_busy = env.branch_busy;
        let worktrees = env.graph.show_worktrees;
        let query = self.find.query.as_str();
        let mut request = None;
        table.show(
            ui,
            c,
            list,
            |i| {
                let commit = repo.commit(commits[i]);
                Row {
                    hash: commit.oid.short(repo.abbrev_len),
                    marked: marked.is_some_and(|(m, _)| *m == commit.oid),
                    refs: badges(repo, &refs[commits[i].ix()], Some(commits[i]), env.graph),
                    subject: &commit.subject,
                    author: &commit.author_name,
                    author_email: &commit.author_email,
                    date: &commit.author_date,
                    found: log::find_in(commit, query),
                    ..Row::default()
                }
            },
            |ui, i, list| {
                let commit = repo.commit(commits[i]);
                // The selected pair, if this row is one of them.
                let pair = list
                    .pair()
                    .filter(|_| list.is_selected(i))
                    .map(|(a, b)| (repo.commit(commits[a]).oid, repo.commit(commits[b]).oid));
                // The selected commits as listed, if this row is one of them.
                let selection: Vec<Oid> = if list.is_selected(i) && list.many.len() > 1 {
                    list.many
                        .iter()
                        .map(|&r| repo.commit(commits[r]).oid)
                        .collect()
                } else {
                    vec![commit.oid]
                };
                let env = MenuEnv {
                    marked,
                    head,
                    pair,
                    working_tree: repo.has_working_tree,
                };
                if let Some(r) = row_menu(ui, commit, &env) {
                    request = Some(r);
                }
                if let Some(r) = super::branches::row_node_menu(
                    ui,
                    repo,
                    commit.oid,
                    &selection,
                    branches,
                    branch_busy,
                    worktrees,
                ) {
                    self.branch_requests.push(r);
                }
            },
            // The details pane shows the whole message.
            None,
        );
        if commits.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| ui.weak("No commits."));
        }
        self.requests.extend(request);
    }

    /// The selected commit as `git log` shows it: full hash, refs, a merge's parents, author and
    /// date, each with a copy button where it helps, the committer where it differs, then the
    /// full message and the notes. The text is selectable, and web links open in the browser.
    fn details_pane(&mut self, ui: &mut Ui, env: &mut Env, c: &Colors) {
        let Some(view) = &self.view else { return };
        let Some(ix) = view.selected_commit() else {
            return;
        };
        let commit = view.repo.commit(ix);
        let now = ui.input(|i| i.time);
        let copied = self
            .copied
            .filter(|&(_, at)| now - at < COPIED_SECONDS)
            .map(|(what, _)| what);
        if copied.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
        }
        let mut copy = None;
        let mut jump = None;
        let ctx = ui.ctx().clone();
        let path = env.open.clone().unwrap_or_else(|| view.repo.path.clone());
        let details = env.details.get(&path, commit.oid, &ctx);
        let loaded = details.and_then(|d| d.as_ref().ok());
        let refs = badges(&view.repo, &view.refs[ix.ix()], Some(ix), env.graph);
        ScrollArea::vertical()
            .id_salt(("log-details", commit.oid))
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 4.0;
                        let label_width = field_label_width(ui);
                        field(ui, label_width, "Commit", |ui| {
                            ui.label(RichText::new(commit.oid.to_hex()).monospace());
                            if widgets::copy_button(ui, copied == Some(Copied::Hash), c.added)
                                .on_hover_text("Copy the full hash")
                                .clicked()
                            {
                                ui.ctx().copy_text(commit.oid.to_hex());
                                copy = Some(Copied::Hash);
                            }
                        });
                        if !refs.is_empty() {
                            field(ui, label_width, "Refs", |ui| {
                                for r in &refs {
                                    badge_widget(ui, r, &env.palette);
                                }
                            });
                        }
                        if commit.parents.len() > 1 {
                            field(ui, label_width, "Merge", |ui| {
                                for &p in &commit.parents {
                                    if let Some(row) = parent_link(ui, view, p) {
                                        jump = Some(row);
                                    }
                                }
                            });
                        }
                        field(ui, label_width, "Author", |ui| {
                            ui.label(format!("{} <{}>", commit.author_name, commit.author_email));
                            if widgets::copy_button(ui, copied == Some(Copied::Email), c.added)
                                .on_hover_text("Copy the email address")
                                .clicked()
                            {
                                ui.ctx().copy_text(commit.author_email.clone());
                                copy = Some(Copied::Email);
                            }
                        });
                        // To the minute in local time until git has said more.
                        let date = loaded.map_or(&commit.author_date, |d| &d.author_date);
                        field(ui, label_width, "Date", |ui| ui.label(date));
                        // The committer only where it differs from the author.
                        if let Some(d) = loaded {
                            let committer = (d.committer_name.as_str(), d.committer_email.as_str());
                            if committer != (&commit.author_name, &commit.author_email) {
                                field(ui, label_width, "Committer", |ui| {
                                    ui.label(format!("{} <{}>", committer.0, committer.1))
                                });
                            }
                            if d.committer_date != d.author_date {
                                field(ui, label_width, "Committed", |ui| {
                                    ui.label(&d.committer_date)
                                });
                            }
                        }
                        ui.add_space(10.0);
                        match details {
                            Some(Ok(d)) => {
                                message_ui(ui, &d.message, true);
                                for note in &d.notes {
                                    ui.add_space(10.0);
                                    ui.weak(&note.heading);
                                    ui.add_space(2.0);
                                    message_ui(ui, &note.text, false);
                                }
                            }
                            Some(Err(e)) => {
                                message_ui(ui, &commit.subject, true);
                                ui.add_space(6.0);
                                ui.colored_label(ui.visuals().error_fg_color, e);
                            }
                            None => {
                                message_ui(ui, &commit.subject, true);
                                ui.spinner();
                            }
                        }
                    });
            });
        if let Some(what) = copy {
            self.copied = Some((what, now));
        }
        if let Some(row) = jump
            && let Some(view) = &mut self.view
        {
            view.list.select(Some(row));
        }
    }

    /// The selected commit's changed files: a filter and a sortable table.
    fn files_pane(&mut self, ui: &mut Ui, env: &mut Env, c: &Colors) {
        let Some(view) = &self.view else { return };
        let Some(ix) = view.selected_commit() else {
            return;
        };
        let commit = view.repo.commit(ix);
        let merge = commit.parents.len() > 1;
        let ctx = ui.ctx().clone();
        let path = env.open.clone().unwrap_or_else(|| view.repo.path.clone());
        let files = self.files.get(&path, commit.oid, &ctx, |git, oid| {
            git.changed_files(oid).map_err(|e| e.to_string())
        });
        let weak = ui.visuals().weak_text_color();
        let action = self
            .table
            .show(ui, c, "log", Id::new(commit.oid), files, |ui| {
                if merge {
                    ui.label(
                        RichText::new("Merge: compared with its first parent")
                            .size(12.0)
                            .color(weak),
                    );
                }
            });
        if let Some(f) = action.blame {
            let spec = BlameSpec {
                rev: Rev::Commit(commit.oid),
                path: f.path.clone(),
            };
            self.blames.push((view.repo.clone(), spec));
        }
        let parent = commit.parents.first().map(|&p| view.repo.commit(p).oid);
        let open = action
            .open
            .into_iter()
            .map(|f| {
                let spec = FileDiffSpec::of_commit(commit.oid, parent, f);
                (view.repo.clone(), spec)
            })
            .collect();
        self.diffs.push(open);
    }
}

/// What a row's menu needs to know besides the commit.
struct MenuEnv<'a> {
    marked: Option<&'a (Oid, String)>,
    head: Option<Oid>,
    /// The commit selected first and the one clicked last, when the row is one of the
    /// selected.
    pair: Option<(Oid, Oid)>,
    working_tree: bool,
}

/// The menu of a commit row: comparing and marking, and copying. Says what was picked.
fn row_menu(ui: &mut Ui, commit: &Commit, env: &MenuEnv) -> Option<CompareRequest> {
    let MenuEnv {
        marked,
        head,
        pair,
        working_tree,
    } = *env;
    let oid = commit.oid;
    let mut request = None;
    // As the graph's Compare revisions on two nodes; the mark is left alone.
    let compare = ui
        .add_enabled(pair.is_some(), egui::Button::new("Compare revisions"))
        .on_disabled_hover_text(format!(
            "{} a second commit first",
            keys::with_command("click")
        ));
    if compare.clicked()
        && let Some((first, second)) = pair
    {
        request = Some(CompareRequest::Compare(first, second));
        ui.close();
    }
    let is_marked = marked.is_some_and(|(m, _)| *m == oid);
    let (text, mark) = if is_marked {
        ("Clear the mark", None)
    } else {
        ("Mark for comparison", Some(oid))
    };
    if ui.button(text).clicked() {
        request = Some(CompareRequest::Mark(mark));
        ui.close();
    }
    // Greyed out rather than left out, so the menu keeps its shape.
    let other = marked.filter(|(m, _)| *m != oid);
    let label = other.map_or("Compare with marked".to_owned(), |(_, name)| {
        format!("Compare with marked ({name})")
    });
    let why = if is_marked {
        "This is the marked commit"
    } else {
        "Mark a commit for comparison first"
    };
    let with_marked = ui
        .add_enabled(other.is_some(), egui::Button::new(label))
        .on_disabled_hover_text(why);
    if with_marked.clicked()
        && let Some(&(m, _)) = other
    {
        request = Some(CompareRequest::Compare(m, oid));
        ui.close();
    }
    let other_head = head.filter(|&h| h != oid);
    let with_head = ui
        .add_enabled(other_head.is_some(), egui::Button::new("Compare with HEAD"))
        .on_disabled_hover_text("This is HEAD");
    if with_head.clicked()
        && let Some(h) = other_head
    {
        request = Some(CompareRequest::Compare(oid, h));
        ui.close();
    }
    let with_working_tree = ui
        .add_enabled(working_tree, egui::Button::new("Compare with working tree"))
        .on_disabled_hover_text("A bare repository has no working tree");
    if with_working_tree.clicked() {
        request = Some(CompareRequest::WorkingTree(oid));
        ui.close();
    }
    crate::menu::separator(ui);
    if ui.button("Copy hash").clicked() {
        ui.ctx().copy_text(oid.to_hex());
        ui.close();
    }
    if ui.button("Copy subject").clicked() {
        ui.ctx().copy_text(commit.subject.clone());
        ui.close();
    }
    request
}

/// `text` on one line, cut with an ellipsis at `width`.
pub(super) fn cell(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping {
        max_width: width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.painter().layout_job(job)
}

/// A label before a commit's subject, as in the graph: a ref, or another worktree's detached
/// HEAD.
#[derive(Clone, Copy, Debug)]
pub(super) enum Badge<'a> {
    /// A ref, the other worktree that has it checked out, while worktrees are shown, and a
    /// local branch's ahead and behind counts, while upstreams are shown and it has any.
    Ref(&'a GitRef, Option<&'a Worktree>, Option<(usize, usize)>),
    Worktree(&'a Worktree),
}

impl Badge<'_> {
    fn text(&self) -> String {
        match self {
            Badge::Ref(r, Some(w), _) if r.kind == RefKind::DetachedHead => w.name(),
            Badge::Ref(r, _, _) => r.name.clone(),
            Badge::Worktree(w) => w.name(),
        }
    }

    /// A detached worktree's name, in italics as in the graph.
    fn is_detached_worktree(&self) -> bool {
        match self {
            Badge::Ref(r, w, _) => r.kind == RefKind::DetachedHead && w.is_some(),
            Badge::Worktree(_) => true,
        }
    }
}

/// Width of a worktree badge's folder glyph and the gap after it.
const BADGE_GLYPH: f32 = 11.0 + 3.0;

/// The badges of a commit of the snapshot (`None` for one outside it) with the refs `refs`
/// (indices into [`Repo::refs`], in display order): the refs `graph` shows and, while it
/// shows worktrees, the other worktrees there, worktrees first ([`Repo::labels`]).
pub(super) fn badges<'a>(
    repo: &'a Repo,
    refs: &[usize],
    commit: Option<CommitIx>,
    graph: &GraphOptions,
) -> Vec<Badge<'a>> {
    let shown = graph.show_worktrees;
    let refs: Vec<usize> = refs
        .iter()
        .copied()
        .filter(|&i| {
            let r = &repo.refs[i];
            graph.shows(r.kind) || (shown && repo.worktrees_on(&r.full_name).next().is_some())
        })
        .collect();
    let detached: Vec<usize> = match commit {
        Some(c) if shown => repo.detached_worktrees_at(c).collect(),
        _ => Vec::new(),
    };
    repo.labels(&refs, &detached, shown)
        .into_iter()
        .map(|label| match label {
            Label::Ref { index, worktree } => {
                let counts = repo
                    .upstream_of(index)
                    .filter(|_| graph.show_upstreams)
                    .map(|u| (u.ahead(), u.behind()))
                    .filter(|&(a, b)| a + b > 0);
                Badge::Ref(
                    &repo.refs[index],
                    worktree.map(|k| &repo.worktrees[k]),
                    counts,
                )
            }
            Label::Worktree(k) => Badge::Worktree(&repo.worktrees[k]),
        })
        .collect()
}

/// A badge in the graph's label colour, left-centred at `at` and at most `max_width` wide.
/// Returns its width.
pub(super) fn badge(
    ui: &Ui,
    badge: &Badge,
    palette: &Palette,
    at: egui::Pos2,
    max_width: f32,
) -> f32 {
    let worktree_fill = |w: &Worktree| {
        if w.missing {
            palette.missing_worktree
        } else {
            palette.worktree
        }
    };
    // As in the graph: a worktree's folder glyph (crossed out if it is gone) before its
    // name, in the worktree colour unless it is HEAD's, and a detached one in italics.
    let (fill, worktree) = match *badge {
        Badge::Ref(r, Some(w), _) if !r.is_head => (worktree_fill(w), Some(w)),
        Badge::Ref(r, w, _) => (palette.ref_fill(r.kind, r.is_head, &r.name), w),
        Badge::Worktree(w) => (worktree_fill(w), Some(w)),
    };
    let glyph = if worktree.is_some() { BADGE_GLYPH } else { 0.0 };
    // A local branch's ↑ahead ↓behind after its name.
    let counts = match *badge {
        Badge::Ref(_, _, counts) => counts,
        Badge::Worktree(_) => None,
    };
    let counts_width = counts.map_or(0.0, |c| crate::upstreams::counts_width(ui.painter(), c));
    let color = text_on(fill);
    let pad = 5.0;
    let mut job = LayoutJob::single_section(
        badge.text(),
        TextFormat {
            font_id: FontId::proportional(11.5),
            color,
            italics: badge.is_detached_worktree(),
            ..TextFormat::default()
        },
    );
    job.wrap = TextWrapping {
        max_width: (max_width - 2.0 * pad - glyph - counts_width).max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let g = ui.painter().layout_job(job);
    let size = vec2(g.size().x + 2.0 * pad + glyph + counts_width, 17.0);
    let rect = Rect::from_min_size(pos2(at.x, at.y - size.y / 2.0), size);
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(3),
        fill,
        Stroke::new(1.0, Color32::from_black_alpha(64)),
        egui::StrokeKind::Inside,
    );
    if let Some(w) = worktree {
        let icon = Rect::from_center_size(
            pos2(rect.left() + pad + 5.5, rect.center().y),
            Vec2::splat(11.0),
        );
        let folder = if w.missing {
            glyphs::FOLDER_GONE
        } else {
            glyphs::FOLDER
        };
        widgets::paint_glyph(painter, icon, folder, color);
    }
    painter.galley(
        pos2(
            rect.left() + pad + glyph,
            rect.center().y - g.size().y / 2.0,
        ),
        g,
        color,
    );
    if let Some(c) = counts {
        let at = pos2(rect.right() - pad - counts_width, rect.center().y);
        crate::upstreams::paint_counts(painter, at, c, color);
    }
    size.x
}

/// The background of column headings and bars: the panel colour with a line below.
pub(super) fn heading_background(ui: &Ui, rect: Rect, c: &Colors) {
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, ui.visuals().panel_fill);
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, c.line),
    );
}

/// A divider between two panes. While it is dragged, returns where its middle should go: the
/// pointer's position across the bar, less where on the bar it was grabbed.
pub(super) fn divider(ui: &mut Ui, id: Id, bar: &Bar) -> Option<f32> {
    let c = colors(ui);
    let rect = bar.rect;
    let cursor = if bar.vertical {
        CursorIcon::ResizeHorizontal
    } else {
        CursorIcon::ResizeVertical
    };
    let response = ui.interact(rect, id, Sense::drag()).on_hover_cursor(cursor);
    let active = response.hovered() || response.dragged();
    let painter = ui.painter();
    let fill = if active {
        widgets::tones(ui).accent
    } else {
        ui.visuals().panel_fill
    };
    painter.rect_filled(rect, 0.0, fill);
    let line = Stroke::new(1.0, c.line);
    if bar.vertical {
        painter.vline(rect.left() + 0.5, rect.y_range(), line);
        painter.vline(rect.right() - 0.5, rect.y_range(), line);
    } else {
        painter.hline(rect.x_range(), rect.top() + 0.5, line);
        painter.hline(rect.x_range(), rect.bottom() - 0.5, line);
    }
    let along = |p: egui::Pos2| if bar.vertical { p.x } else { p.y };
    let pointer = response.interact_pointer_pos().map(along)?;
    if response.drag_started() {
        let middle = along(rect.center());
        ui.data_mut(|d| d.insert_temp(id, pointer - middle));
    }
    if !response.dragged() {
        return None;
    }
    let grabbed = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
    Some(pointer - grabbed)
}

/// The layout picker and the reset button, laid out right to left.
fn layout_tools(ui: &mut Ui, settings: &mut LogWindowSettings) {
    let current = settings.layout;
    let reset = ui.add_enabled_ui(!settings.dividers.is_default(current), |ui| {
        widgets::icon_button(ui, glyphs::RESET, false)
    });
    let reset = widgets::tip_explained(
        reset.inner,
        "Reset layout",
        "",
        "Put the dividers of this layout back where they started.",
    );
    if reset.clicked() {
        settings.dividers.reset(current);
    }
    if let Some(layout) = layout_picker(ui, current) {
        settings.layout = layout;
    }
}

/// Which commits the log lists and in what order, as toggles and order segments. In a
/// right-to-left layout.
fn walk_tools(ui: &mut Ui, options: &mut LogOptions) {
    let order = [
        (LogOrder::Topological, glyphs::ORDER_TOPOLOGICAL),
        (LogOrder::Date, glyphs::ORDER_DATE),
    ];
    let chosen = widgets::segmented(ui, options.order, &order, |o, r| match o {
        LogOrder::Topological => widgets::tip_explained(
            r,
            "Topological order",
            "",
            "Each branch's commits together, parents after their children.",
        ),
        LogOrder::Date => widgets::tip_explained(
            r,
            "Date order",
            "",
            "Newest first, the branches' commits interleaved; parents still after their \
             children.",
        ),
    });
    if let Some(o) = chosen {
        options.order = o;
    }
    ui.add_space(8.0);
    let toggles = [
        (
            &mut options.branchings_only,
            glyphs::BRANCHINGS,
            "Branchings and merges only",
            "List only the commits where history branches or joins, and those with refs; the \
             ones in between are left out.",
        ),
        (
            &mut options.no_merges,
            glyphs::NO_MERGES,
            "No merges",
            "Leave merge commits out.",
        ),
        (
            &mut options.first_parent,
            glyphs::FIRST_PARENT,
            "First parent only",
            "Follow only the first parent of each merge, so the commits it merged in are left \
             out.",
        ),
        (
            &mut options.all_branches,
            glyphs::ALL_BRANCHES,
            "All branches",
            "Also list the commits of every branch, remote branch and tag, not only the \
             history the log was opened on.",
        ),
    ];
    for (on, glyph, title, body) in toggles {
        let response = widgets::icon_button(ui, glyph, *on);
        if widgets::tip_explained(response, title, "", body).clicked() {
            *on = !*on;
        }
    }
}

/// The four layouts as icon segments, each named in its tooltip. Returns the one clicked.
pub fn layout_picker(ui: &mut Ui, current: LogLayout) -> Option<LogLayout> {
    let items = LogLayout::ALL.map(|l| (l, layout_glyph(l)));
    widgets::segmented(ui, current, &items, |layout, response| {
        response.on_hover_text(layout.label())
    })
}

/// Height of a line of the selected commit's fields: that of a copy button.
const FIELD: f32 = 20.0;
/// The names of the selected commit's fields.
const FIELD_LABELS: [&str; 7] = [
    "Commit",
    "Refs",
    "Merge",
    "Author",
    "Date",
    "Committer",
    "Committed",
];

/// The width of the column of field names: the widest name and a gap.
fn field_label_width(ui: &Ui) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let widest = FIELD_LABELS
        .iter()
        .map(|&l| {
            ui.painter()
                .layout_no_wrap(l.to_owned(), font.clone(), Color32::PLACEHOLDER)
                .size()
                .x
        })
        .fold(0.0, f32::max);
    widest + 12.0
}

/// One of the selected commit's fields: its name in a column `label_width` wide, then what
/// `add` adds, wrapping onto more lines where it has to.
fn field<R>(ui: &mut Ui, label_width: f32, label: &str, add: impl FnOnce(&mut Ui) -> R) {
    debug_assert!(FIELD_LABELS.contains(&label));
    ui.horizontal_top(|ui| {
        let centred = egui::Layout::left_to_right(egui::Align::Center);
        ui.allocate_ui_with_layout(vec2(label_width, FIELD), centred, |ui| {
            ui.set_min_size(vec2(label_width, FIELD));
            ui.weak(label);
        });
        let width = ui.available_width();
        ui.allocate_ui_with_layout(vec2(width, FIELD), centred.with_main_wrap(true), |ui| {
            ui.set_min_height(FIELD);
            add(ui);
        });
    });
}

/// A badge (see [`badge`]) laid out in `ui`.
fn badge_widget(ui: &mut Ui, b: &Badge, palette: &Palette) {
    let text =
        ui.painter()
            .layout_no_wrap(b.text(), FontId::proportional(11.5), Color32::PLACEHOLDER);
    let glyph = if let Badge::Ref(_, Some(_), _) | Badge::Worktree(_) = b {
        BADGE_GLYPH
    } else {
        0.0
    };
    let counts = match *b {
        Badge::Ref(_, _, Some(c)) => crate::upstreams::counts_width(ui.painter(), c),
        _ => 0.0,
    };
    let (rect, response) = ui.allocate_exact_size(
        vec2(text.size().x + 11.0 + glyph + counts, 17.0),
        Sense::hover(),
    );
    badge(ui, b, palette, rect.left_center(), rect.width());
    if let Badge::Ref(_, Some(w), _) | Badge::Worktree(w) = b {
        response.on_hover_text(format!("Worktree {}", w.path.display()));
    }
}

/// A merge parent's short hash, as `git log`'s `Merge:` line has it. A link to the parent's
/// row if the log lists it: returns that row when clicked. Otherwise plain, weak text.
fn parent_link(ui: &mut Ui, view: &LogView, parent: CommitIx) -> Option<usize> {
    let text = RichText::new(view.repo.commit(parent).oid.short(view.repo.abbrev_len)).monospace();
    let Some(row) = view.commits.iter().position(|&c| c == parent) else {
        ui.label(text.color(ui.visuals().weak_text_color()))
            .on_hover_text("Not in this log");
        return None;
    };
    let link = ui
        .link(text.color(widgets::tones(ui).on_fg))
        .on_hover_text("Show this commit");
    link.clicked().then_some(row)
}

/// A commit message or note, monospaced as in a terminal; a message's subject in the strong
/// colour. The text is selectable; `http(s)` links are clickable.
fn message_ui(ui: &mut Ui, message: &str, subject: bool) {
    let font = FontId::monospace(12.5);
    let text = ui.visuals().text_color();
    let strong = ui.visuals().strong_text_color();
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    let label = |ui: &mut Ui, s: &str, color: Color32| {
        ui.add(egui::Label::new(RichText::new(s).font(font.clone()).color(color)).selectable(true));
    };
    // Lines without links are shown together, as one label.
    let mut plain = String::new();
    let flush = |ui: &mut Ui, plain: &mut String| {
        if !plain.is_empty() {
            label(ui, plain.trim_end_matches('\n'), text);
            plain.clear();
        }
    };
    for (n, line) in message.trim_end().lines().enumerate() {
        let urls = find_urls(line);
        let first = subject && n == 0;
        let color = if first { strong } else { text };
        if urls.is_empty() && !first {
            plain.push_str(if line.is_empty() { " " } else { line });
            plain.push('\n');
            continue;
        }
        flush(ui, &mut plain);
        if urls.is_empty() {
            label(ui, line, color);
            continue;
        }
        ui.horizontal_wrapped(|ui| {
            let mut at = 0;
            for url in urls {
                if url.start > at {
                    label(ui, &line[at..url.start], color);
                }
                let link = RichText::new(&line[url.clone()])
                    .font(font.clone())
                    .color(widgets::tones(ui).on_fg);
                ui.hyperlink_to(link, &line[url.clone()]);
                at = url.end;
            }
            if at < line.len() {
                label(ui, &line[at..], color);
            }
        });
    }
    flush(ui, &mut plain);
}

impl ParterreApp {
    pub(super) fn open_loss_log(&mut self, repo: Arc<Repo>, oids: &[Oid]) {
        let commits: Vec<_> = oids.iter().filter_map(|oid| repo.lookup(oid)).collect();
        let mut query = LogQuery::default();
        query.tips = commits.clone();
        let was_open = self.log.is_open();
        let [w, h] = self.settings.log_window.size;
        self.log
            .open(repo, query, self.settings.log_window.options, vec2(w, h));
        if let Some(view) = &mut self.log.view {
            view.pin_exact(commits);
        }
        if was_open {
            self.focus_log = true;
        }
    }

    /// Show log on graph nodes: one gives the node's log, two the range between them in the
    /// order they were selected; anything else does nothing.
    pub(super) fn show_log(&mut self, nodes: &[usize]) {
        let Some(scene) = &self.scene else { return };
        // The newest snapshot, which the scene on screen may not have caught up with yet.
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let commits: Vec<CommitIx> = nodes
            .iter()
            .filter_map(|&n| {
                let oid = scene.repo.commit(scene.graph.nodes[n].commit).oid;
                repo.lookup(&oid)
            })
            .collect();
        if commits.len() != nodes.len() {
            return;
        }
        self.open_log(repo, &commits);
    }

    /// Opens the log of `commits` (one or two, see [`LogQuery::for_selection`]).
    pub(super) fn open_log(&mut self, repo: Arc<Repo>, commits: &[CommitIx]) {
        let Some(query) = LogQuery::for_selection(&repo, commits) else {
            return;
        };
        let was_open = self.log.is_open();
        let [w, h] = self.settings.log_window.size;
        let options = self.settings.log_window.options;
        self.log.open(repo, query, options, vec2(w, h));
        if was_open {
            self.focus_log = true;
        }
    }

    /// The log window, while it is open. (Screenshot runs embed it in the main window.)
    pub(super) fn log_window(&mut self, ctx: &egui::Context) {
        if !self.log.is_open() {
            self.log.title_theme = None;
            return;
        }
        let builder = egui::ViewportBuilder::default()
            .with_title(self.log.title())
            .with_app_id(crate::settings::APP_ID)
            .with_icon(self.window_icon.clone())
            .with_inner_size(self.log.size)
            .with_min_inner_size([480.0, 360.0]);
        let id = viewport_id();
        crate::usage::screen(ctx, id.0, crate::usage::Screen::Log);
        if self.log.title_theme.is_none() && !ctx.embed_viewports() {
            ctx.request_repaint();
        }
        if std::mem::take(&mut self.focus_log) && !ctx.embed_viewports() {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
        }
        ctx.show_viewport_immediate(id, builder, |ui, class| {
            super::commands::window_begin(ui);
            let embedded = class == egui::ViewportClass::EmbeddedWindow;
            if !embedded {
                if self.log.title_theme != self.window_theme {
                    self.log.title_theme = self.window_theme;
                    if let Some(theme) = self.window_theme {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::SetTheme(theme));
                    }
                }
                // Reload and fetch, as in the main window.
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
                    self.settings.log_window.size = [size.x, size.y];
                }
                if reload {
                    self.reload();
                }
                if fetch {
                    self.fetch(ui.ctx(), id);
                }
                // Keys go to the main window too when the log is embedded in it.
                self.log.handle_keys(ui);
                text_size::read_input(ui, &mut self.settings.text_size, true);
                if close {
                    self.log.view = None;
                }
            }
            if !self.log.is_open() {
                return;
            }
            let palette = Palette::new(ui.visuals().dark_mode, &self.settings.branch_colors);
            let mut env = Env {
                details: &mut self.details,
                open: self.repo.as_ref().map(|r| r.path.clone()),
                palette,
                graph: &self.settings.graph,
                settings: &mut self.settings.log_window,
                marked: self.marked.as_ref(),
                branches: self.branches.catalog.as_deref(),
                branch_busy: self.branches.busy(),
            };
            let log = &mut self.log;
            egui::CentralPanel::default()
                .frame(egui::Frame::central_panel(&ui.ctx().global_style()).inner_margin(0))
                .show(ui, |ui| log.contents(ui, &mut env));
        });
        for request in self.log.take_compare_requests() {
            self.compare_request(request);
        }
        for request in self.log.take_branch_requests() {
            self.branches.request(ctx, request, viewport_id());
        }
        for (repo, spec) in self.log.take_diff_requests() {
            self.diffs.open(repo, spec, &self.settings.diff_window, ctx);
        }
        for (repo, spec) in self.log.take_blame_requests() {
            self.open_blame(repo, spec, None, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::commit_table::ROW;
    use parterre_core::log_layout::Dividers;

    #[test]
    fn loss_log_keeps_exact_membership_with_saved_filters_and_after_refs_disappear() {
        let mut w = window(&["tip", "at risk", "protected", "root"]);
        let view = w.view.as_mut().unwrap();
        let original = view.repo.clone();
        let wanted = vec![CommitIx(0), CommitIx(1)];
        view.options = LogOptions {
            all_branches: true,
            first_parent: true,
            no_merges: true,
            branchings_only: true,
            ..LogOptions::default()
        };
        view.pin_exact(wanted.clone());
        assert_eq!(view.commits, wanted);
        let empty = Arc::new(Repo::new(
            original.path.clone(),
            Vec::new(),
            Vec::new(),
            parterre_core::Head::Branch {
                name: "main".into(),
                target: None,
            },
        ));
        w.reload(&empty);
        let view = w.view.as_mut().unwrap();
        view.set_options(LogOptions::default());
        assert!(Arc::ptr_eq(&view.repo, &original));
        assert_eq!(view.commits, wanted);
        assert_eq!(view.graph.rows(0..wanted.len()).len(), wanted.len());
    }

    #[test]
    fn layouts_tile_the_body_with_their_panes_and_dividers() {
        let body = Rect::from_min_size(pos2(10.0, 50.0), vec2(1100.0, 700.0));
        let d = Dividers::default();
        for layout in LogLayout::ALL {
            let a = arrange(layout, d.of(layout), body);
            let rects: Vec<Rect> = a
                .panes
                .iter()
                .copied()
                .chain(a.bars.map(|b| b.rect))
                .collect();
            let area: f32 = rects.iter().map(|r| r.area()).sum();
            assert!(
                (area - body.area()).abs() < 1.0,
                "{layout:?} leaves gaps or overlaps"
            );
            for (i, r) in rects.iter().enumerate() {
                assert!(
                    body.expand(0.01).contains_rect(*r),
                    "{layout:?}: {r:?} outside"
                );
                for s in &rects[i + 1..] {
                    assert!(
                        r.intersect(*s).area() < 0.01,
                        "{layout:?}: {r:?} overlaps {s:?}"
                    );
                }
            }
            for (which, bar) in a.bars.iter().enumerate() {
                assert_eq!(bar.vertical, Dividers::splits_width(layout, which));
                let thickness = if bar.vertical {
                    bar.rect.width()
                } else {
                    bar.rect.height()
                };
                assert_eq!(thickness, DIVIDER);
            }
        }
    }

    #[test]
    fn a_divider_dragged_by_its_middle_stays_under_the_pointer() {
        let body = Rect::from_min_size(pos2(0.0, 40.0), vec2(1000.0, 600.0));
        for layout in LogLayout::ALL {
            for which in 0..2 {
                let mut d = Dividers::default();
                let before = arrange(layout, d.of(layout), body).bars[which];
                let along = |r: Rect| {
                    if before.vertical {
                        r.center().x
                    } else {
                        r.center().y
                    }
                };
                let target = along(before.rect) + 37.0;
                d.set(layout, which, before.fraction(target));
                let after = arrange(layout, d.of(layout), body).bars[which];
                assert!(
                    (along(after.rect) - target).abs() <= 0.5,
                    "{layout:?} divider {which}: {} instead of {target}",
                    along(after.rect)
                );
            }
        }
    }

    /// A log window showing a line of commits with these subjects, newest first, the first
    /// selected; commit `i`'s hash starts with `i` then `a`s.
    fn window(subjects: &[&str]) -> LogWindow {
        let n = subjects.len() as u32;
        let commits = subjects
            .iter()
            .enumerate()
            .map(|(i, subject)| Commit {
                oid: Oid::from_hex(&format!("{i:x}{:a<39}", "")).unwrap(),
                parents: if (i as u32) + 1 < n {
                    vec![CommitIx(i as u32 + 1)]
                } else {
                    Vec::new()
                },
                truncated: false,
                empty_tree: false,
                author_name: String::new(),
                author_email: String::new(),
                author_time: 100 - i as i64,
                author_date: String::new(),
                commit_time: 100 - i as i64,
                subject: (*subject).into(),
            })
            .collect();
        let repo = Repo::new(
            "/nowhere".into(),
            commits,
            Vec::new(),
            parterre_core::repo::Head::Detached(CommitIx(0)),
        );
        let mut w = LogWindow::default();
        let query = LogQuery::commit(CommitIx(0));
        w.open(
            Arc::new(repo),
            query,
            LogOptions::default(),
            vec2(1100.0, 760.0),
        );
        w
    }

    #[test]
    fn a_revert_moves_the_log_to_the_new_commit() {
        let mut w = window(&["tip", "change", "root"]);
        let old = w.view.as_ref().unwrap().repo.clone();
        let (from, change) = (old.commit(CommitIx(0)).oid, old.commit(CommitIx(1)).oid);
        // The snapshot after the revert: its commit first, on top of the old tip.
        let to = Oid::from_hex(&format!("{:f<40}", "")).unwrap();
        let mut commits = vec![Commit {
            oid: to,
            parents: vec![CommitIx(1)],
            subject: "Revert \"change\"".into(),
            ..old.commit(CommitIx(0)).clone()
        }];
        commits.extend(old.commits.iter().map(|c| Commit {
            parents: c.parents.iter().map(|p| CommitIx(p.0 + 1)).collect(),
            ..c.clone()
        }));
        let new = Arc::new(Repo::new(
            "/nowhere".into(),
            commits,
            Vec::new(),
            parterre_core::repo::Head::Detached(CommitIx(0)),
        ));
        // The change reverted was selected; the reload alone keeps it.
        w.view.as_mut().unwrap().list.select(Some(1));
        w.reload(&new);
        let view = w.view.as_ref().unwrap();
        assert_eq!(view.commits.len(), 3, "still from the old tip");
        assert!(!w.follow(change, to), "the log doesn't walk from there");
        assert!(w.follow(from, to));
        let view = w.view.as_ref().unwrap();
        assert_eq!(view.commits.len(), 4);
        assert_eq!(selected(&w), Some(0));
    }

    fn frame(ctx: &egui::Context, w: &mut LogWindow, events: Vec<egui::Event>) {
        frame_with(ctx, w, events, Modifiers::NONE);
    }

    /// A frame with `modifiers` held.
    fn frame_with(
        ctx: &egui::Context,
        w: &mut LogWindow,
        events: Vec<egui::Event>,
        modifiers: Modifiers,
    ) {
        let mut events = events;
        events.insert(0, egui::Event::ModifiersChanged(modifiers));
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1100.0, 760.0))),
            events,
            ..Default::default()
        };
        let (mut details, graph) = (Details::default(), GraphOptions::default());
        let mut settings = LogWindowSettings::default();
        ctx.run_ui(input, |ui| {
            let mut env = Env {
                details: &mut details,
                open: None,
                palette: Palette::new(false, &[]),
                graph: &graph,
                settings: &mut settings,
                marked: None,
                branches: None,
                branch_busy: false,
            };
            w.handle_keys(ui);
            if w.is_open() {
                w.contents(ui, &mut env);
            }
        })
        .textures_delta
        .clear();
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

    fn typed(text: &str) -> egui::Event {
        egui::Event::Text(text.into())
    }

    fn selected(w: &LogWindow) -> Option<usize> {
        w.view.as_ref()?.list.selected
    }

    /// Clicks commit row `row` (in the default layout the list is at the top) with `modifiers`.
    fn click_row(ctx: &egui::Context, w: &mut LogWindow, row: usize, modifiers: Modifiers) {
        let pos = pos2(300.0, 40.0 + HEADING + (row as f32 + 0.5) * ROW);
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        };
        frame_with(ctx, w, vec![egui::Event::PointerMoved(pos)], modifiers);
        frame_with(ctx, w, vec![button(true)], modifiers);
        frame_with(ctx, w, vec![button(false)], modifiers);
    }

    #[test]
    fn dragging_a_heading_border_resizes_the_column() {
        let ctx = egui::Context::default();
        let mut w = window(&["c", "b", "a"]);
        frame(&ctx, &mut w, Vec::new());
        frame(&ctx, &mut w, Vec::new());
        let picked = |w: &LogWindow, i| w.view.as_ref().unwrap().list.widths.picked(i);
        // The border between the hash and the subject sizes the hash.
        let id = Id::new(("log-commits", w.view.as_ref().unwrap().id)).with(("column-border", 1));
        let border = ctx.read_response(id).expect("the border is there").rect;
        let at = border.center();
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(&ctx, &mut w, vec![egui::Event::PointerMoved(at)]);
        frame(&ctx, &mut w, vec![button(at, true)]);
        for dx in [10.0, 20.0, 40.0] {
            frame(
                &ctx,
                &mut w,
                vec![egui::Event::PointerMoved(at + vec2(dx, 0.0))],
            );
        }
        frame(&ctx, &mut w, vec![button(at + vec2(40.0, 0.0), false)]);
        frame(&ctx, &mut w, Vec::new());
        let moved = ctx.read_response(id).unwrap().rect.center().x;
        assert!(
            (moved - (at.x + 40.0)).abs() < 0.5,
            "{moved} vs {}",
            at.x + 40.0
        );
        assert!(picked(&w, 1).is_some());
        assert_eq!(picked(&w, 0), None);
        // Another log in the open window keeps the width; a window opened again doesn't.
        let repo = w.view.as_ref().unwrap().repo.clone();
        w.open(
            repo.clone(),
            LogQuery::commit(CommitIx(1)),
            LogOptions::default(),
            vec2(1100.0, 760.0),
        );
        assert!(picked(&w, 1).is_some());
        w.view = None;
        w.open(
            repo,
            LogQuery::commit(CommitIx(1)),
            LogOptions::default(),
            vec2(1100.0, 760.0),
        );
        assert_eq!(picked(&w, 1), None);
    }

    #[test]
    fn ctrl_and_shift_click_select_many_and_compare_the_first_with_the_last() {
        let ctx = egui::Context::default();
        let mut w = window(&["e", "d", "c", "b", "a"]);
        frame(&ctx, &mut w, Vec::new());
        let pair = |w: &LogWindow| w.view.as_ref().unwrap().list.pair();
        let many = |w: &LogWindow| {
            let list = &w.view.as_ref().unwrap().list;
            list.many.iter().copied().collect::<Vec<_>>()
        };
        // The window opens with its first commit selected: Ctrl+click adds to it.
        click_row(&ctx, &mut w, 2, Modifiers::COMMAND);
        assert_eq!((pair(&w), many(&w)), (Some((0, 2)), vec![0, 2]));
        click_row(&ctx, &mut w, 1, Modifiers::NONE);
        click_row(&ctx, &mut w, 3, Modifiers::COMMAND);
        assert_eq!(pair(&w), Some((1, 3)));
        assert_eq!(selected(&w), Some(3), "the details follow the last clicked");
        // Shift+click: the range from the commit selected first, compared with the last.
        click_row(&ctx, &mut w, 4, Modifiers::SHIFT);
        assert_eq!((pair(&w), many(&w)), (Some((1, 4)), vec![1, 2, 3, 4]));
        assert_eq!(selected(&w), Some(4));
        // Ctrl+click takes one out; taking out the last clicked goes back to the first.
        click_row(&ctx, &mut w, 2, Modifiers::COMMAND);
        assert_eq!(many(&w), [1, 3, 4]);
        click_row(&ctx, &mut w, 4, Modifiers::COMMAND);
        assert_eq!((selected(&w), pair(&w)), (Some(1), None));
        // A right-click on a selected row keeps them; on another, selects it alone.
        right_click_row(&ctx, &mut w, 1);
        assert_eq!(many(&w), [1, 3]);
        right_click_row(&ctx, &mut w, 0);
        assert_eq!((many(&w), pair(&w)), (vec![0], None));
        // A plain click or a key leaves one.
        frame(&ctx, &mut w, vec![key_with(Key::A, Modifiers::COMMAND)]);
        assert_eq!(many(&w), [0, 1, 2, 3, 4]);
        frame(&ctx, &mut w, vec![key(Key::ArrowDown)]);
        assert_eq!((many(&w), selected(&w)), (vec![1], Some(1)));
        click_row(&ctx, &mut w, 4, Modifiers::NONE);
        assert_eq!((many(&w), pair(&w)), (vec![4], None));
    }

    /// Right-clicks commit row `row`, right of where [`click_row`] clicks, so that the menu
    /// that opens is out of its way.
    fn right_click_row(ctx: &egui::Context, w: &mut LogWindow, row: usize) {
        let pos = pos2(700.0, 40.0 + HEADING + (row as f32 + 0.5) * ROW);
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(ctx, w, vec![egui::Event::PointerMoved(pos)]);
        frame(ctx, w, vec![button(true)]);
        frame(ctx, w, vec![button(false)]);
    }

    fn has_find_focus(ctx: &egui::Context) -> bool {
        ctx.memory(|m| m.has_focus(LogWindow::find_id()))
    }

    #[test]
    fn ctrl_f_finds_subjects_and_hashes_and_selects_the_commits() {
        let ctx = egui::Context::default();
        let mut w = window(&[
            "Fix the parser",
            "Docs",
            "parser: faster",
            "More docs",
            "Parse",
        ]);
        frame(&ctx, &mut w, Vec::new());
        assert_eq!(selected(&w), Some(0));
        frame(&ctx, &mut w, vec![key_with(Key::F, Modifiers::COMMAND)]);
        assert!(has_find_focus(&ctx));
        // The selected row stays while it matches; the first after it is gone to otherwise.
        frame(&ctx, &mut w, vec![typed("PARSE")]);
        assert_eq!(w.find.matches, [0, 2, 4]);
        assert_eq!(selected(&w), Some(0));
        frame(&ctx, &mut w, vec![typed("r:")]);
        assert_eq!(w.find.matches, [2]);
        assert_eq!(selected(&w), Some(2));
        // Enter and F3 go on, round the end; Shift goes back.
        frame(&ctx, &mut w, vec![key(Key::Backspace), key(Key::Backspace)]);
        assert_eq!(w.find.matches, [0, 2, 4]);
        frame(&ctx, &mut w, vec![key(Key::Enter)]);
        assert_eq!(selected(&w), Some(4));
        assert!(has_find_focus(&ctx));
        frame(&ctx, &mut w, vec![crate::keys::find_next().event()]);
        assert_eq!(selected(&w), Some(0));
        let shift = Modifiers::SHIFT;
        let held = egui::Event::ModifiersChanged(shift);
        frame(
            &ctx,
            &mut w,
            vec![held, crate::keys::find_previous().event()],
        );
        frame(
            &ctx,
            &mut w,
            vec![egui::Event::ModifiersChanged(Modifiers::NONE)],
        );
        assert_eq!(selected(&w), Some(4));
        // From a row that isn't a place, F3 goes to the next one after it.
        w.view.as_mut().unwrap().list.select(Some(1));
        ctx.memory_mut(|m| m.surrender_focus(LogWindow::find_id()));
        frame(&ctx, &mut w, vec![crate::keys::find_next().event()]);
        assert_eq!(selected(&w), Some(2));

        // The start of a hash: commit 3's is "3aaa…".
        frame(&ctx, &mut w, vec![key_with(Key::F, Modifiers::COMMAND)]);
        frame(&ctx, &mut w, vec![typed("3AAA")]);
        assert_eq!(w.find.matches, [3]);
        assert_eq!(selected(&w), Some(3));
    }

    #[test]
    fn esc_in_the_find_field_leaves_it_and_only_then_closes_the_window() {
        let ctx = egui::Context::default();
        let mut w = window(&["one", "two"]);
        frame(&ctx, &mut w, Vec::new());
        frame(&ctx, &mut w, vec![key_with(Key::F, Modifiers::COMMAND)]);
        frame(&ctx, &mut w, vec![typed("o")]);
        assert_eq!(w.find.matches, [0, 1]);
        frame(&ctx, &mut w, vec![key(Key::Escape)]);
        assert!(w.is_open());
        assert!(!has_find_focus(&ctx));
        assert!(w.find.query.is_empty() && w.find.matches.is_empty());
        // Ctrl+F offers the last query again.
        frame(&ctx, &mut w, vec![key_with(Key::F, Modifiers::COMMAND)]);
        assert_eq!(w.find.query, "o");
        frame(&ctx, &mut w, vec![key(Key::Escape)]);
        frame(&ctx, &mut w, vec![crate::keys::close_window().event()]);
        assert!(!w.is_open());
    }
}
