use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use eframe::egui::text::{LayoutJob, TextFormat, TextWrapping};
use eframe::egui::{
    self, Color32, FontId, Id, Key, Margin, Modifiers, Rangef, Rect, RichText, ScrollArea, Sense,
    Stroke, Ui, UiBuilder, Vec2, pos2, vec2,
};
use parterre_core::blame::BlameSpec;
use parterre_core::columns::ColumnWidths;
use parterre_core::file_diff::{FileDiffSpec, Rev};
use parterre_core::find::{self, Step};
use parterre_core::glyphs::{self, Glyph};
use parterre_core::log::{self, LogList, LogOptions, LogOrder, LogQuery};
use parterre_core::log_graph::LogGraph;
use parterre_core::log_layout::LogLayout;
use parterre_core::revgraph::GraphOptions;
use parterre_core::text::{find_urls, plural, thousands};
use parterre_core::{Commit, CommitIx, Oid, Repo};

use super::commit_table::{CommitList, CommitTable, Row, Select};
use super::compare_window::CompareRequest;
use super::divider::{Bar, DIVIDER_THICKNESS, divider};
use super::file_table::{DiffQueue, FileTable, Lister, Listing};
use super::table_style::{Colors, badge_widget, badges, colors};
use super::text_view::{find_field_left, find_field_width, match_count};
use super::viewport::{self, Viewport};
use super::{Details, ParterreApp};
use crate::keys;
use crate::settings::LogWindowSettings;
use crate::text_size;
use crate::theme::Palette;
use crate::usage::Screen;
use crate::widgets;

const HEADER_HEIGHT: f32 = 40.0;
const HEADER_PAD: f32 = 8.0;
/// The [`widgets::trailing`] row of the header's tools.
const TOOLS_ROW: &str = "log-tools";
const COPIED_CHECK_SECONDS: f64 = 1.2;
/// A copy button's height.
const FIELD_HEIGHT: f32 = 20.0;
const FIELD_LABELS: [&str; 7] = [
    "Commit",
    "Refs",
    "Merge",
    "Author",
    "Date",
    "Committer",
    "Committed",
];

fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("log")
}

fn layout_glyph(layout: LogLayout) -> Glyph {
    match layout {
        LogLayout::Stacked => glyphs::LAYOUT_STACKED,
        LogLayout::SideBySide => glyphs::LAYOUT_SIDE_BY_SIDE,
        LogLayout::DetailsBelow => glyphs::LAYOUT_DETAILS_BELOW,
        LogLayout::FilesRight => glyphs::LAYOUT_FILES_RIGHT,
    }
}

#[derive(Debug, Default)]
pub struct LogWindow {
    view: Option<LogView>,
    file_table: FileTable,
    changed_files: Lister<Oid, Listing>,
    /// Fixed while the window is open: egui resizes a window whose viewport builder changes.
    size: Vec2,
    viewport: Viewport,
    copied_at: Option<(Copied, f64)>,
    logs_opened: u64,
    diffs: DiffQueue<(Arc<Repo>, FileDiffSpec)>,
    compare_requests: Vec<CompareRequest>,
    branch_requests: Vec<super::branches::Request>,
    blame_requests: Vec<(Arc<Repo>, BlameSpec)>,
    find: Find,
}

#[derive(Debug, Default)]
struct Find {
    query: String,
    last_query: String,
    matches: Vec<usize>,
    focus_next_frame: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Copied {
    Hash,
    Email,
}

#[derive(Debug)]
struct LogView {
    /// A new log gets a new one, and so starts scrolled to the top; F5 keeps it.
    id: u64,
    repo: Arc<Repo>,
    refs_by_commit: Vec<Vec<usize>>,
    query: LogQuery,
    options: LogOptions,
    /// Safety warnings' logs keep their commits and snapshot, even after the last ref is gone.
    pinned: bool,
    commits: Vec<CommitIx>,
    graph: LogGraph,
    list: CommitList,
}

impl LogView {
    fn new(id: u64, repo: Arc<Repo>, query: LogQuery, options: LogOptions) -> LogView {
        let list = query.list(&repo, &options);
        let mut view = LogView {
            id,
            refs_by_commit: repo.refs_by_commit(),
            repo,
            query,
            options,
            pinned: false,
            graph: LogGraph::new(&list),
            commits: list.commits,
            list: CommitList::default(),
        };
        view.select_first();
        view
    }

    fn select_first(&mut self) {
        self.list.select((!self.commits.is_empty()).then_some(0));
    }

    fn selected_commit(&self) -> Option<CommitIx> {
        self.commits.get(self.list.selected?).copied()
    }

    fn reload(&mut self, repo: Arc<Repo>) {
        if !self.pinned {
            self.rebuild(repo, self.options);
        }
    }

    fn follow(&mut self, from: Oid, to: Oid) -> bool {
        if self.pinned {
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
        self.list.reveal_selected = true;
        row.is_some()
    }

    fn set_options(&mut self, options: LogOptions) {
        if self.pinned {
            self.options = options;
            return;
        }
        self.rebuild(self.repo.clone(), options);
        self.list.reveal_selected = true;
    }

    fn pin(&mut self, commits: &[CommitIx]) {
        let wanted: HashSet<CommitIx> = commits.iter().copied().collect();
        // Every commit wanted, whatever filters the user saved.
        let options = LogOptions {
            order: self.options.order,
            ..LogOptions::default()
        };
        let all = self.query.list(&self.repo, &options);
        let mut list = LogList::default();
        let rows = all.commits.iter().zip(&all.parents).zip(&all.outside);
        for ((&commit, parents), &outside) in rows {
            if !wanted.contains(&commit) {
                continue;
            }
            list.commits.push(commit);
            list.parents.push(
                parents
                    .iter()
                    .copied()
                    .filter(|p| wanted.contains(p))
                    .collect(),
            );
            list.outside
                .push(outside || parents.iter().any(|p| !wanted.contains(p)));
        }
        self.graph = LogGraph::new(&list);
        self.commits = list.commits;
        self.select_first();
        self.pinned = true;
    }

    /// Keeps the selection where its commits are still listed, and the scroll position.
    fn rebuild(&mut self, repo: Arc<Repo>, options: LogOptions) {
        let oid = |row: usize| self.commits.get(row).map(|&c| self.repo.commit(c).oid);
        let selected = self.list.selected.and_then(oid);
        let anchor = self.list.anchor.and_then(oid);
        let many: Vec<Oid> = self.list.many.iter().filter_map(|&row| oid(row)).collect();
        let in_new_snapshot = |commits: &[CommitIx]| -> Vec<CommitIx> {
            commits
                .iter()
                .filter_map(|&c| repo.lookup(&self.repo.commit(c).oid))
                .collect()
        };
        let mut query = self.query.clone();
        query.tips = in_new_snapshot(&self.query.tips);
        query.exclude = in_new_snapshot(&self.query.exclude);
        let old_list = std::mem::take(&mut self.list);
        *self = LogView::new(self.id, repo, query, options);
        self.list.scroll_offset = old_list.scroll_offset;
        self.list.view_height = old_list.view_height;
        self.list.widths = old_list.widths;
        let rows: HashMap<Oid, usize> = self
            .commits
            .iter()
            .enumerate()
            .map(|(row, &c)| (self.repo.commit(c).oid, row))
            .collect();
        if let Some(&row) = selected.and_then(|o| rows.get(&o)) {
            self.list.select(Some(row));
            self.list.reveal_selected = false;
            self.list.anchor = anchor.and_then(|o| rows.get(&o).copied()).or(Some(row));
            self.list
                .many
                .extend(many.iter().filter_map(|o| rows.get(o)));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Commits,
    Details,
    Files,
}

impl Pane {
    const ALL: [Pane; 3] = [Pane::Commits, Pane::Details, Pane::Files];
}

struct Env<'a> {
    details: &'a mut Details,
    /// Where git is asked about commits: a pinned view's snapshot may be of a worktree since
    /// deleted, and any worktree of the repository answers.
    open_worktree: Option<&'a Path>,
    palette: Palette,
    graph: &'a GraphOptions,
    settings: &'a mut LogWindowSettings,
    /// The commit marked for comparison, and its name.
    marked: Option<&'a (Oid, String)>,
    branches: Option<&'a parterre_core::branches::Catalog>,
    branch_busy: bool,
}

#[derive(Clone, Copy, Debug)]
struct Arrangement {
    /// In [`Pane::ALL`] order.
    panes: [Rect; 3],
    dividers: [Bar; 2],
}

struct Cut {
    first: Rangef,
    bar: Rangef,
    second: Rangef,
    origin: f32,
    room: f32,
}

/// `range` cut in two by a divider at `fraction` of the room the two parts share.
fn cut(range: Rangef, fraction: f32) -> Cut {
    let room = (range.span() - DIVIDER_THICKNESS).max(1.0);
    let at = range.min + (room * fraction).round();
    let after = at + DIVIDER_THICKNESS;
    Cut {
        first: Rangef::new(range.min, at),
        bar: Rangef::new(at, after),
        second: Rangef::new(after, range.max.max(after)),
        origin: range.min + DIVIDER_THICKNESS / 2.0,
        room,
    }
}

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
            let room = (body.height() - 2.0 * DIVIDER_THICKNESS).max(1.0);
            let y1 = body.top() + (room * a).round();
            let y2 = y1 + DIVIDER_THICKNESS + (room * b).round();
            let stacked_bar = |y: f32, origin: f32| Bar {
                rect: rect(xs, Rangef::new(y, y + DIVIDER_THICKNESS)),
                vertical: false,
                origin,
                room,
            };
            let below = |y: f32| y + DIVIDER_THICKNESS;
            Arrangement {
                panes: [
                    rect(xs, Rangef::new(body.top(), y1)),
                    rect(xs, Rangef::new(below(y1), y2)),
                    rect(xs, Rangef::new(below(y2), body.bottom().max(below(y2)))),
                ],
                dividers: [
                    stacked_bar(y1, body.top() + DIVIDER_THICKNESS / 2.0),
                    stacked_bar(y2, body.top() + 1.5 * DIVIDER_THICKNESS),
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
                dividers: [
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
                dividers: [
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
                dividers: [
                    bar(rect(across.bar, ys), true, &across),
                    bar(rect(across.first, left.bar), false, &left),
                ],
            }
        }
    }
}

impl LogWindow {
    fn open(&mut self, repo: Arc<Repo>, query: LogQuery, options: LogOptions, size: Vec2) {
        // Column widths last as long as the window is open.
        let widths = match self.view.take() {
            Some(view) => view.list.widths,
            None => {
                self.size = size;
                self.file_table.widths = ColumnWidths::default();
                ColumnWidths::default()
            }
        };
        self.logs_opened += 1;
        let mut view = LogView::new(self.logs_opened, repo, query, options);
        view.list.widths = widths;
        self.view = Some(view);
        self.refind();
    }

    fn pin(&mut self, commits: &[CommitIx]) {
        if let Some(view) = &mut self.view {
            view.pin(commits);
        }
        self.refind();
    }

    pub fn is_open(&self) -> bool {
        self.view.is_some()
    }

    pub fn close(&mut self) {
        self.view = None;
        self.changed_files = Lister::default();
        self.file_table.clear_selection();
        self.diffs.cancel();
    }

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

    fn open_find(&mut self) {
        if self.find.query.is_empty() && !self.find.last_query.is_empty() {
            self.find.query = self.find.last_query.clone();
            self.query_changed();
        }
        self.find.focus_next_frame = true;
    }

    fn close_find(&mut self, ctx: &egui::Context) {
        self.find.query.clear();
        self.refind();
        ctx.memory_mut(|m| m.surrender_focus(Self::find_id()));
    }

    fn refind(&mut self) {
        self.find.matches = match &self.view {
            Some(v) => log::find(&v.repo, &v.commits, &self.find.query),
            None => Vec::new(),
        };
    }

    fn current_match(&self) -> Option<usize> {
        let selected = self.view.as_ref()?.list.selected?;
        self.find.matches.binary_search(&selected).ok()
    }

    fn query_changed(&mut self) {
        self.refind();
        if !self.find.query.trim().is_empty() {
            self.find.last_query = self.find.query.clone();
        }
        let Some(view) = &mut self.view else { return };
        let from = view.list.selected.unwrap_or(0);
        if let Some(k) = find::first_from(&self.find.matches, from) {
            view.list.select(Some(self.find.matches[k]));
        }
    }

    fn find_step(&mut self, step: Step) {
        let current = self.current_match();
        let Some(view) = &mut self.view else { return };
        let from = view.list.selected.unwrap_or(0);
        if let Some(k) = find::step(&self.find.matches, current, step, from) {
            view.list.select(Some(self.find.matches[k]));
        }
    }

    fn find_field(&mut self, ui: &mut Ui, width: f32) {
        let count = match_count(self.current_match(), self.find.matches.len());
        let focus = std::mem::take(&mut self.find.focus_next_frame);
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
        if found.next {
            self.find_step(Step::Next);
        } else if found.previous {
            self.find_step(Step::Previous);
        }
    }

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
        if next {
            self.find_step(Step::Next);
        } else if previous {
            self.find_step(Step::Previous);
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
        if ui.input_mut(|i| keys::CONTEXT_MENU.consume(i)) {
            view.list.open_selected_menu = true;
        }
        let page = view.list.rows_per_page();
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

    fn header(&mut self, ui: &mut Ui, c: &Colors, env: &mut Env) {
        let Some(view) = &self.view else { return };
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), HEADER_HEIGHT), Sense::hover());
        // The tools go in after find, so that Tab reads them in order: placed by their width.
        let mut tools = ui.new_child(
            UiBuilder::new()
                .max_rect(rect.shrink2(vec2(HEADER_PAD, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        tools.spacing_mut().item_spacing.x = 4.0;
        let tools_left = rect.right() - HEADER_PAD - widgets::trailing_width(&tools, TOOLS_ROW);
        let painter = ui.painter();
        painter.hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            Stroke::new(1.0, c.line),
        );
        let mut label = view.query.label(&view.repo, &view.refs_by_commit, |r| {
            env.graph.shows(r.kind)
        });
        if view.pinned {
            label.from = None;
            label.to = format!("{} at risk", plural(view.commits.len(), "commit"));
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
        // Find goes between the range (given some room) and the count.
        let room = Rangef::new(rect.left() + 12.0 + 140.0, count_left - 14.0);
        let width = find_field_width(room.span());
        let find_left = find_field_left(room, rect.center().x, width);
        job.wrap = TextWrapping::truncate_at_width((find_left - rect.left() - 36.0).max(0.0));
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
            vec2(width, widgets::BUTTON_SIZE),
        );
        let layout = egui::Layout::left_to_right(egui::Align::Center);
        ui.scope_builder(UiBuilder::new().max_rect(at).layout(layout), |ui| {
            self.find_field(ui, width);
        });
        let Some(view) = &self.view else { return };
        let requests = &mut self.compare_requests;
        widgets::trailing(&mut tools, TOOLS_ROW, |ui| {
            if let (Some(&from), Some(&to)) = (view.query.exclude.first(), view.query.tips.first())
            {
                let compare = widgets::tip_explained(
                    widgets::text_button(ui, "Compare files"),
                    "Compare files",
                    "",
                    "List the files that differ between the two ends of the range, and open \
                     their diffs.",
                );
                if compare.clicked() {
                    let oid = |c: CommitIx| view.repo.commit(c).oid;
                    requests.push(CompareRequest::Compare(oid(from), oid(to)));
                }
                ui.add_space(8.0);
            }
            ui.add_enabled_ui(!view.pinned, |ui| walk_tools(ui, &mut env.settings.options));
            ui.add_space(12.0);
            layout_tools(ui, env.settings);
        });
    }

    fn body(&mut self, ui: &mut Ui, body: Rect, env: &mut Env) {
        let layout = env.settings.layout;
        let arrangement = arrange(layout, env.settings.dividers.of(layout), body);
        for (pane, rect) in Pane::ALL.into_iter().zip(arrangement.panes) {
            self.pane(ui, pane, rect, env);
        }
        for (which, bar) in arrangement.dividers.iter().enumerate() {
            let id = Id::new(("log-divider", layout, which));
            if let Some(middle) = divider(ui, id, bar) {
                env.settings
                    .dividers
                    .set(layout, which, bar.fraction(middle));
            }
        }
        ui.allocate_rect(body, Sense::hover());
    }

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
            Pane::Files => self.files_pane(&mut child, env),
        }
    }

    fn commits_pane(&mut self, ui: &mut Ui, env: &mut Env, c: &Colors) {
        let Some(view) = &mut self.view else { return };
        let LogView {
            id,
            repo,
            refs_by_commit,
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
                    marked_for_comparison: marked.is_some_and(|(m, _)| *m == commit.oid),
                    refs: badges(
                        repo,
                        &refs_by_commit[commits[i].ix()],
                        Some(commits[i]),
                        env.graph,
                    ),
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
                let oid = |row: usize| repo.commit(commits[row]).oid;
                let pair = list
                    .selected_first_and_last()
                    .filter(|_| list.is_selected(i))
                    .map(|(first, last)| (oid(first), oid(last)));
                let selection: Vec<Oid> = if list.is_selected(i) && list.many.len() > 1 {
                    list.many.iter().map(|&row| oid(row)).collect()
                } else {
                    vec![commit.oid]
                };
                let menu_env = MenuEnv {
                    marked,
                    head,
                    pair,
                    working_tree: repo.has_working_tree,
                };
                if let Some(r) = row_menu(ui, commit, &menu_env) {
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
        self.compare_requests.extend(request);
    }

    fn details_pane(&mut self, ui: &mut Ui, env: &mut Env, c: &Colors) {
        let Some(view) = &self.view else { return };
        let Some(ix) = view.selected_commit() else {
            return;
        };
        let commit = view.repo.commit(ix);
        let now = ui.input(|i| i.time);
        let copied = self
            .copied_at
            .filter(|&(_, at)| now - at < COPIED_CHECK_SECONDS)
            .map(|(what, _)| what);
        if copied.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
        }
        let mut copy = None;
        let mut jump = None;
        let path = env.open_worktree.unwrap_or(&view.repo.path);
        let details = env.details.get(path, commit.oid, ui.ctx());
        let loaded = details.and_then(|d| d.as_ref().ok());
        let refs = badges(
            &view.repo,
            &view.refs_by_commit[ix.ix()],
            Some(ix),
            env.graph,
        );
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
                            if widgets::copy_icon_button(ui, copied == Some(Copied::Hash), c.added)
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
                            if widgets::copy_icon_button(ui, copied == Some(Copied::Email), c.added)
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
            self.copied_at = Some((what, now));
        }
        if let Some(row) = jump
            && let Some(view) = &mut self.view
        {
            view.list.select(Some(row));
        }
    }

    fn files_pane(&mut self, ui: &mut Ui, env: &mut Env) {
        let Some(view) = &self.view else { return };
        let Some(ix) = view.selected_commit() else {
            return;
        };
        let commit = view.repo.commit(ix);
        let merge = commit.parents.len() > 1;
        let path = env.open_worktree.unwrap_or(&view.repo.path);
        let files = self
            .changed_files
            .get(path, commit.oid, ui.ctx(), |git, oid| {
                git.changed_files(oid).map_err(|e| e.to_string())
            });
        let weak = ui.visuals().weak_text_color();
        let action = self
            .file_table
            .show(ui, "log", Id::new(commit.oid), files, |ui| {
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
            self.blame_requests.push((view.repo.clone(), spec));
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

struct MenuEnv<'a> {
    marked: Option<&'a (Oid, String)>,
    head: Option<Oid>,
    /// When the row is one of them: the commit selected first and the one clicked last.
    pair: Option<(Oid, Oid)>,
    working_tree: bool,
}

fn row_menu(ui: &mut Ui, commit: &Commit, env: &MenuEnv) -> Option<CompareRequest> {
    let MenuEnv {
        marked,
        head,
        pair,
        working_tree,
    } = *env;
    let oid = commit.oid;
    let mut request = None;
    // The mark is left alone, as by the graph's Compare revisions on two nodes.
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

fn layout_tools(ui: &mut Ui, settings: &mut LogWindowSettings) {
    if let Some(layout) = layout_picker(ui, settings.layout) {
        settings.layout = layout;
    }
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
}

fn walk_tools(ui: &mut Ui, options: &mut LogOptions) {
    walk_toggles(ui, options);
    ui.add_space(8.0);
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
}

fn walk_toggles(ui: &mut Ui, options: &mut LogOptions) {
    let toggles = [
        (
            &mut options.all_branches,
            glyphs::ALL_BRANCHES,
            "All branches",
            "Also list the commits of every branch, remote branch and tag, not only the \
             history the log was opened on.",
        ),
        (
            &mut options.first_parent,
            glyphs::FIRST_PARENT,
            "First parent only",
            "Follow only the first parent of each merge, so the commits it merged in are left \
             out.",
        ),
        (
            &mut options.no_merges,
            glyphs::NO_MERGES,
            "No merges",
            "Leave merge commits out.",
        ),
        (
            &mut options.branchings_only,
            glyphs::BRANCHINGS,
            "Branchings and merges only",
            "List only the commits where history branches or joins, and those with refs; the \
             ones in between are left out.",
        ),
    ];
    for (on, glyph, title, body) in toggles {
        let response = widgets::icon_button(ui, glyph, *on);
        if widgets::tip_explained(response, title, "", body).clicked() {
            *on = !*on;
        }
    }
}

pub(super) fn layout_picker(ui: &mut Ui, current: LogLayout) -> Option<LogLayout> {
    let items = LogLayout::ALL.map(|l| (l, layout_glyph(l)));
    widgets::segmented(ui, current, &items, |layout, response| {
        response.on_hover_text(layout.label())
    })
}

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

fn field<R>(ui: &mut Ui, label_width: f32, label: &str, add: impl FnOnce(&mut Ui) -> R) {
    debug_assert!(FIELD_LABELS.contains(&label));
    ui.horizontal_top(|ui| {
        let centred = egui::Layout::left_to_right(egui::Align::Center);
        ui.allocate_ui_with_layout(vec2(label_width, FIELD_HEIGHT), centred, |ui| {
            ui.set_min_size(vec2(label_width, FIELD_HEIGHT));
            ui.weak(label);
        });
        let width = ui.available_width();
        let wrapping = centred.with_main_wrap(true);
        ui.allocate_ui_with_layout(vec2(width, FIELD_HEIGHT), wrapping, |ui| {
            ui.set_min_height(FIELD_HEIGHT);
            add(ui);
        });
    });
}

/// A merge parent's short hash, as `git log`'s `Merge:` line has it: a link that returns the
/// parent's row when clicked, or weak text where the log doesn't list it.
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

/// Selectable, monospaced as in a terminal, with clickable `http(s)` links.
fn message_ui(ui: &mut Ui, message: &str, first_line_is_subject: bool) {
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
        let is_subject = first_line_is_subject && n == 0;
        let color = if is_subject { strong } else { text };
        if urls.is_empty() && !is_subject {
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
        let commits: Vec<CommitIx> = oids.iter().filter_map(|oid| repo.lookup(oid)).collect();
        let mut query = LogQuery::default();
        query.tips = commits.clone();
        self.open_log_query(repo, query);
        self.log.pin(&commits);
    }

    /// One node gives the node's log, two the range between them in the order they were
    /// selected; anything else does nothing.
    pub(super) fn show_log(&mut self, nodes: &[usize]) {
        let Some(scene) = &self.scene else { return };
        // The newest snapshot, which the scene on screen may not have caught up with yet.
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let commits: Vec<CommitIx> = nodes
            .iter()
            .filter_map(|&n| repo.lookup(&scene.node_oid(n)))
            .collect();
        if commits.len() != nodes.len() {
            return;
        }
        self.open_log(repo, &commits);
    }

    pub(super) fn open_log(&mut self, repo: Arc<Repo>, commits: &[CommitIx]) {
        if let Some(query) = LogQuery::for_selection(&repo, commits) {
            self.open_log_query(repo, query);
        }
    }

    fn open_log_query(&mut self, repo: Arc<Repo>, query: LogQuery) {
        let was_open = self.log.is_open();
        let [w, h] = self.settings.log_window.size;
        let options = self.settings.log_window.options;
        self.log.open(repo, query, options, vec2(w, h));
        if was_open {
            self.log.viewport.bring_to_front = true;
        }
    }

    pub(super) fn log_window(&mut self, ctx: &egui::Context) {
        if !self.log.is_open() {
            self.log.viewport.closed();
            return;
        }
        let id = viewport_id();
        let builder = viewport::builder(
            self.log.title(),
            self.log.size,
            [480.0, 360.0],
            &self.window_icon,
        );
        self.log.viewport.before_show(ctx, id, Screen::Log);
        ctx.show_viewport_immediate(id, builder, |ui, class| {
            if let Some(input) = self.log.viewport.begin(ui, class, self.window_theme) {
                input.keep_size(&mut self.settings.log_window.size);
                let (reload, fetch) = ui.input_mut(keys::reload_and_fetch);
                if reload {
                    self.reload();
                }
                if fetch {
                    self.fetch(ui.ctx(), id);
                }
                self.log.handle_keys(ui);
                text_size::read_input(ui, &mut self.settings.text_size, true);
                if input.close_requested {
                    self.log.view = None;
                }
            }
            if !self.log.is_open() {
                return;
            }
            let palette = Palette::new(ui.visuals().dark_mode, &self.settings.branch_colors);
            let mut env = Env {
                details: &mut self.details,
                open_worktree: self.repo.as_deref().map(|r| r.path.as_path()),
                palette,
                graph: &self.settings.graph,
                settings: &mut self.settings.log_window,
                marked: self.marked.as_ref(),
                branches: self.branches.catalog.as_deref(),
                branch_busy: self.branches.busy(),
            };
            let log = &mut self.log;
            viewport::central_panel(ui).show(ui, |ui| log.contents(ui, &mut env));
        });
        for request in std::mem::take(&mut self.log.compare_requests) {
            self.compare_request(request);
        }
        for request in std::mem::take(&mut self.log.branch_requests) {
            self.branches.request(ctx, request, id);
        }
        for (repo, spec) in self.log.diffs.take() {
            self.diffs.open(repo, spec, &self.settings.diff_window, ctx);
        }
        for (repo, spec) in std::mem::take(&mut self.log.blame_requests) {
            self.open_blame(repo, spec, None, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::commit_table::ROW_HEIGHT;
    use crate::app::table_style::HEADING_HEIGHT;
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
        view.pin(&wanted);
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
    fn a_pinned_log_finds_in_its_own_rows() {
        let mut w = window(&["tip", "at risk", "protected", "root"]);
        w.find.query = "root".into();
        w.refind();
        assert_eq!(w.find.matches, [3]);
        w.pin(&[CommitIx(1), CommitIx(3)]);
        assert_eq!(w.find.matches, [1]);
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
                .chain(a.dividers.map(|b| b.rect))
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
            for (which, bar) in a.dividers.iter().enumerate() {
                assert_eq!(bar.vertical, Dividers::splits_width(layout, which));
                let thickness = if bar.vertical {
                    bar.rect.width()
                } else {
                    bar.rect.height()
                };
                assert_eq!(thickness, DIVIDER_THICKNESS);
            }
        }
    }

    #[test]
    fn a_divider_dragged_by_its_middle_stays_under_the_pointer() {
        let body = Rect::from_min_size(pos2(0.0, 40.0), vec2(1000.0, 600.0));
        for layout in LogLayout::ALL {
            for which in 0..2 {
                let mut d = Dividers::default();
                let before = arrange(layout, d.of(layout), body).dividers[which];
                let along = |r: Rect| {
                    if before.vertical {
                        r.center().x
                    } else {
                        r.center().y
                    }
                };
                let target = along(before.rect) + 37.0;
                d.set(layout, which, before.fraction(target));
                let after = arrange(layout, d.of(layout), body).dividers[which];
                assert!(
                    (along(after.rect) - target).abs() <= 0.5,
                    "{layout:?} divider {which}: {} instead of {target}",
                    along(after.rect)
                );
            }
        }
    }

    /// A line of commits with these subjects, newest first, the first selected; commit `i`'s
    /// hash is `i` then `a`s.
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
                open_worktree: None,
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

    /// In the default layout the commits are at the top.
    fn row_y(row: usize) -> f32 {
        HEADER_HEIGHT + HEADING_HEIGHT + (row as f32 + 0.5) * ROW_HEIGHT
    }

    fn click_row(ctx: &egui::Context, w: &mut LogWindow, row: usize, modifiers: Modifiers) {
        let pos = pos2(300.0, row_y(row));
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
        let pair = |w: &LogWindow| w.view.as_ref().unwrap().list.selected_first_and_last();
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
        click_row(&ctx, &mut w, 4, Modifiers::SHIFT);
        assert_eq!((pair(&w), many(&w)), (Some((1, 4)), vec![1, 2, 3, 4]));
        assert_eq!(selected(&w), Some(4));
        // Taking out the last clicked goes back to the first.
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

    /// Right of where [`click_row`] clicks, so that the menu that opens is out of its way.
    fn right_click_row(ctx: &egui::Context, w: &mut LogWindow, row: usize) {
        let pos = pos2(700.0, row_y(row));
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

    /// With nothing else having the keyboard, the context-menu key (Shift+F10) opens the menu
    /// of the commit the arrow keys move; Esc closes the menu and not the window (which Esc
    /// closes on Windows and Linux, ⌘W on macOS).
    #[test]
    fn the_context_menu_key_opens_the_selected_commits_menu() {
        let ctx = egui::Context::default();
        ctx.add_plugin(crate::menu::ContextMenus);
        let mut w = window(&["one", "two"]);
        frame(&ctx, &mut w, Vec::new());
        click_row(&ctx, &mut w, 1, Modifiers::NONE);
        assert!(!egui::Popup::is_any_open(&ctx));
        frame(&ctx, &mut w, vec![key_with(Key::F10, Modifiers::SHIFT)]);
        frame(&ctx, &mut w, Vec::new());
        assert!(egui::Popup::is_any_open(&ctx), "the menu is open");
        frame(&ctx, &mut w, vec![key(Key::Escape)]);
        assert!(!egui::Popup::is_any_open(&ctx), "Esc closes the menu");
        assert!(w.is_open(), "but not the window");
        frame(&ctx, &mut w, vec![crate::keys::close_window().event()]);
        assert!(!w.is_open(), "which its own key closes then");
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
