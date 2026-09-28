//! The commit table of the log window and of the blame window's history pane: a graph column of
//! lanes ([`LogGraph`]), then the short hash, the subject with ref badges, the author and the
//! date, one commit per row. Virtualised; the rows are painted directly. Each window says what a
//! row shows ([`Row`]) and what its menu offers; the table keeps the selection in view. The
//! columns can be resized by dragging the borders between their headings.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Galley, Id, Rect, ScrollArea, Sense, Stroke, Ui, pos2,
    vec2,
};
use parterre_core::GitRef;
use parterre_core::columns::{ColumnWidths, Layout};
use parterre_core::log::Found;
use parterre_core::log_graph::{GraphRow, LogGraph};

use super::column_borders::{self, Columns};
use super::log_window::{CELL_PAD, Colors, HEADING, badge, cell, heading_background};
use crate::theme::Palette;
use crate::widgets;

/// Height of a commit row.
pub const ROW: f32 = 24.0;
/// Width of a lane of the graph column, and the margin on either side of the lanes.
const GRAPH_LANE: f32 = 11.0;
const GRAPH_PAD: f32 = 4.0;
/// The graph column grows to this many lanes, and only while the subject keeps this much room;
/// lanes further right are cut off.
const GRAPH_MAX_LANES: usize = 24;
const GRAPH_SUBJECT_ROOM: f32 = 220.0;
const AUTHOR_WIDTH: f32 = 170.0;
const DATE_WIDTH: f32 = 128.0;
/// A commit list narrower than this (beside another pane) gets narrower author and date
/// columns, as in the log window prototype's layouts B and D.
const NARROW_LIST: f32 = 720.0;
const NARROW_AUTHOR_WIDTH: f32 = 128.0;
const NARROW_DATE_WIDTH: f32 = 118.0;
/// The columns: graph, hash, subject, author, date. The subject takes what the others leave,
/// and no less than this.
const SUBJECT: usize = 2;
const SUBJECT_MIN: f32 = 80.0;

/// The selected row of a commit table and its scroll position.
#[derive(Debug, Default)]
pub struct CommitList {
    /// The row clicked last, which the window shows the details of.
    pub selected: Option<usize>,
    /// A second selected row, selected before `selected` (with Ctrl+ or Shift+click, in tables
    /// that allow a pair).
    pub other: Option<usize>,
    /// Scroll the selected row into view in the next frame (only if it is out of view).
    pub reveal: bool,
    /// The scroll offset and the height of the rows in the last frame, for keeping the
    /// selection in view and for paging.
    pub scroll: f32,
    pub height: f32,
    /// The column widths the user picked.
    pub widths: ColumnWidths,
}

impl CommitList {
    /// Selects row `i` (or none), alone, and keeps it in view.
    pub fn select(&mut self, i: Option<usize>) {
        self.selected = i;
        self.other = None;
        self.reveal = true;
    }

    /// The two selected rows, in the order they were selected.
    pub fn pair(&self) -> Option<(usize, usize)> {
        Some((self.other?, self.selected?))
    }

    pub fn is_selected(&self, i: usize) -> bool {
        self.selected == Some(i) || self.other == Some(i)
    }

    /// A click on row `i`. A plain click selects it alone. With `add` (Ctrl or Shift held) it
    /// is selected besides the row selected before, making a pair; a third row takes the place
    /// of the earlier of the two, and one of a pair clicked again leaves just the other. A
    /// right-click (`secondary`) keeps the selection if it is on a selected row, so that its
    /// menu can compare the pair.
    pub fn click(&mut self, i: usize, add: bool, secondary: bool) {
        if secondary {
            if !self.is_selected(i) {
                (self.selected, self.other) = (Some(i), None);
            }
            return;
        }
        match self.selected {
            _ if !add => (self.selected, self.other) = (Some(i), None),
            Some(s) if s == i => {
                if let Some(o) = self.other.take() {
                    self.selected = Some(o);
                }
            }
            _ if self.other == Some(i) => self.other = None,
            s => (self.other, self.selected) = (s, Some(i)),
        }
    }

    /// The rows a page up or down moves: those in view, less one.
    pub fn page(&self) -> usize {
        ((self.height / ROW).floor() as usize)
            .saturating_sub(1)
            .max(1)
    }
}

/// What a row shows.
#[derive(Default)]
pub struct Row<'a> {
    /// The short hash; empty for a row that isn't a commit.
    pub hash: String,
    /// Behind the hash (the blame's age shading).
    pub hash_fill: Option<Color32>,
    /// Marked for comparison: a ribbon before the refs.
    pub marked: bool,
    /// The ref badges before the subject, in order.
    pub refs: Vec<&'a GitRef>,
    /// An outlined tag before the subject, in the weak colour ("not in the graph").
    pub tag: Option<&'a str>,
    pub subject: &'a str,
    /// Weak text after the subject.
    pub note: Option<String>,
    pub author: &'a str,
    /// Shown with the author over the author's cell.
    pub author_email: &'a str,
    pub date: &'a str,
    /// Drawn in fainter colours throughout.
    pub greyed: bool,
    /// Where a find query is in the hash and the subject, highlighted.
    pub found: Found,
}

/// A commit table: how many rows, their graph column, and where its state is kept.
pub struct CommitTable<'a> {
    /// Tells tables apart, and lists shown in one table: each has its own scroll position.
    pub id: Id,
    pub rows: usize,
    /// A row for each of the `rows`.
    pub graph: &'a LogGraph,
    pub abbrev_len: usize,
    pub palette: &'a Palette,
    /// Ctrl+ and Shift+click select a second row ([`CommitList::other`]).
    pub pairs: bool,
}

/// Fills the tooltip over row `i`'s subject.
pub type SubjectTip<'a> = dyn FnMut(&mut Ui, usize) + 'a;

/// What a click on a row asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clicks {
    /// A click with either button (see [`CommitList::click`]).
    pub clicked: Option<usize>,
    pub double_clicked: Option<usize>,
}

fn columns(layout: &Layout) -> Columns<'_> {
    Columns {
        layout,
        flex: SUBJECT,
        flex_min: SUBJECT_MIN,
    }
}

impl CommitTable<'_> {
    /// The column headings and, unless there are no rows, the rows as `row` describes them;
    /// `menu` fills a row's menu, given the selection, and `subject_tip`, if given, the tooltip
    /// over a row's subject. A click with either button selects the row (see
    /// [`CommitList::click`]). Returns the clicks. With no rows, the window
    /// can say why below the headings.
    pub fn show<'r>(
        &self,
        ui: &mut Ui,
        c: &Colors,
        list: &mut CommitList,
        row: impl Fn(usize) -> Row<'r>,
        mut menu: impl FnMut(&mut Ui, usize, &CommitList),
        mut subject_tip: Option<&mut SubjectTip>,
    ) -> Clicks {
        if self.graph.lanes == 0 {
            list.widths.reset(0, SUBJECT);
        }
        let defaults = self.default_widths(ui, c, &list.widths);
        let layout = |widths: &ColumnWidths, rect: Rect| {
            widths.layout(&defaults, SUBJECT, SUBJECT_MIN, rect.left(), rect.width())
        };
        let weak = ui.visuals().weak_text_color();
        let text = ui.visuals().text_color();
        let mono = FontId::monospace(12.0);
        let body = egui::TextStyle::Body.resolve(ui.style());
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADING), Sense::hover());
        heading_background(ui, head, c);
        // The borders; then the columns as dragged, in this frame.
        let before = layout(&list.widths, head);
        let active = column_borders::drag(ui, self.id, head, &columns(&before), &mut list.widths);
        let cols = layout(&list.widths, head);
        headings(ui, head, &cols);
        column_borders::paint(ui, head, &columns(&cols), active, c);
        if self.rows == 0 {
            return Clicks::default();
        }

        ui.spacing_mut().item_spacing.y = 0.0;
        let mut area = ScrollArea::vertical().id_salt(self.id).auto_shrink(false);
        if list.reveal
            && let Some(sel) = list.selected
        {
            list.reveal = false;
            let top = sel as f32 * ROW;
            // Before the rows were first shown, the room they will have.
            let height = if list.height > 0.0 {
                list.height
            } else {
                ui.available_height()
            };
            let height = height.max(ROW);
            if top < list.scroll {
                area = area.vertical_scroll_offset(top);
            } else if top + ROW > list.scroll + height {
                area = area.vertical_scroll_offset(top + ROW - height);
            }
        }
        let mut clicks = Clicks::default();
        let add = self.pairs && ui.input(|i| i.modifiers.command || i.modifiers.shift);
        let output = area.show_rows(ui, ROW, self.rows, |ui, range| {
            let graph = self.graph.rows(range.clone());
            for (i, graph) in range.zip(&graph) {
                let r = row(i);
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
                let selected = list.is_selected(i);
                let bg = if selected {
                    Some(c.selected_bg)
                } else if response.hovered() {
                    Some(c.hover)
                } else if i % 2 == 1 {
                    Some(c.stripe)
                } else {
                    None
                };
                if let Some(bg) = bg {
                    ui.painter().rect_filled(rect, 0.0, bg);
                }
                let (fg, fg_weak) = match (selected, r.greyed) {
                    (true, false) => (c.selected_fg, c.selected_fg),
                    (true, true) => {
                        let faint = c.selected_fg.gamma_multiply(0.6);
                        (faint, faint)
                    }
                    (false, false) => (text, weak),
                    (false, true) => {
                        let faint = weak.gamma_multiply(0.75);
                        (faint, faint)
                    }
                };
                // Hash, subject, author and date, after the graph.
                let cols = layout(&list.widths, rect);
                let x: [f32; 4] = std::array::from_fn(|k| cols.x[k + 1]);
                let w: [f32; 4] = std::array::from_fn(|k| cols.w[k + 1]);
                let y = rect.center().y;
                let painter = ui.painter();
                let graph_rect = Rect::from_min_size(rect.min, vec2(cols.w[0], ROW));
                paint_graph(painter, graph_rect, graph, c, bg);
                let put = |g: Arc<Galley>, x: f32, color| {
                    painter.galley(pos2(x, y - g.size().y / 2.0), g, color);
                };
                if let Some(fill) = r.hash_fill {
                    let cell_rect = Rect::from_x_y_ranges(x[0]..=x[0] + w[0], rect.y_range());
                    painter.rect_filled(cell_rect, 0.0, fill);
                }
                let g = cell(ui, &r.hash, mono.clone(), fg_weak, w[0]);
                let digits = r.found.hash.min(r.hash.chars().count());
                if digits > 0 {
                    paint_found(painter, &g, pos2(x[0] + CELL_PAD, y), 0..digits, c.found);
                }
                put(g, x[0] + CELL_PAD, fg_weak);

                // The mark, ref badges and tag, then the subject and the note in what is left.
                let mut left = x[1] + CELL_PAD;
                let right = x[1] + w[1] - CELL_PAD;
                if r.marked {
                    let ribbon = Rect::from_center_size(pos2(left + 5.0, y), vec2(10.0, 15.0));
                    widgets::paint_ribbon(painter, ribbon, self.palette.marked, Stroke::NONE);
                    left += 16.0;
                }
                for git_ref in &r.refs {
                    if left >= right {
                        break;
                    }
                    left += badge(ui, git_ref, self.palette, pos2(left, y), right - left) + 4.0;
                }
                if let Some(tag) = r.tag
                    && left < right
                {
                    left += outlined_tag(ui, tag, fg_weak, pos2(left, y), right - left) + 6.0;
                }
                if left < right {
                    let g = cell(ui, r.subject, body.clone(), fg, right - left);
                    let end = left + g.size().x;
                    for place in &r.found.subject {
                        let chars = |b: usize| r.subject[..b].chars().count();
                        let place = chars(place.start)..chars(place.end);
                        paint_found(painter, &g, pos2(left, y), place, c.found);
                    }
                    put(g, left, fg);
                    if let Some(note) = &r.note
                        && end + 6.0 < right
                    {
                        put(
                            cell(ui, note, body.clone(), fg_weak, right - end - 6.0),
                            end + 6.0,
                            fg_weak,
                        );
                    }
                }
                put(
                    cell(ui, r.author, body.clone(), fg, w[2] - 2.0 * CELL_PAD),
                    x[2] + CELL_PAD,
                    fg,
                );
                put(
                    cell(ui, r.date, body.clone(), fg_weak, w[3] - 2.0 * CELL_PAD),
                    x[3] + CELL_PAD,
                    fg_weak,
                );
                let author = Rect::from_x_y_ranges(x[2]..=x[2] + w[2], rect.y_range());
                let over_author = !r.author.is_empty()
                    && response.hover_pos().is_some_and(|p| author.contains(p));
                let subject = Rect::from_x_y_ranges(x[1]..=x[1] + w[1], rect.y_range());
                let over_subject = response.hover_pos().is_some_and(|p| subject.contains(p));
                let response = match &mut subject_tip {
                    _ if over_author => {
                        response.on_hover_text(format!("{} <{}>", r.author, r.author_email))
                    }
                    Some(tip) if over_subject => response.on_hover_ui(|ui| tip(ui, i)),
                    _ => response,
                };
                if response.clicked() || response.secondary_clicked() {
                    clicks.clicked = Some(i);
                    list.click(i, add, response.secondary_clicked());
                }
                if response.double_clicked() {
                    clicks.double_clicked = Some(i);
                }
                egui::Popup::context_menu(&response)
                    .style(crate::menu::style)
                    .show(|ui| {
                        crate::menu::fit_window(ui, |ui| {
                            ui.set_min_width(crate::menu::MIN_WIDTH);
                            menu(ui, i, list);
                        });
                    });
            }
        });
        list.scroll = output.state.offset.y;
        list.height = output.inner_rect.height();
        clicks
    }

    /// The columns' widths in the width available, as the layout has them before the user
    /// drags any: the graph only as wide as leaves the subject its room, given the other
    /// columns in `widths`. A table without lanes has no graph column.
    fn default_widths(&self, ui: &Ui, c: &Colors, widths: &ColumnWidths) -> [f32; 5] {
        let digit = ui
            .painter()
            .layout_no_wrap("0".into(), FontId::monospace(12.0), c.line)
            .size()
            .x;
        let hash = digit * self.abbrev_len as f32 + 2.0 * CELL_PAD + 2.0;
        let (author, date) = if ui.available_width() < NARROW_LIST {
            (NARROW_AUTHOR_WIDTH, NARROW_DATE_WIDTH)
        } else {
            (AUTHOR_WIDTH, DATE_WIDTH)
        };
        let graph = if self.graph.lanes == 0 {
            0.0
        } else {
            let lanes = self.graph.lanes.min(GRAPH_MAX_LANES) as f32 * GRAPH_LANE;
            let others = widths.get(1, hash) + widths.get(3, author) + widths.get(4, date);
            let room = ui.available_width() - others - GRAPH_SUBJECT_ROOM;
            lanes.min(room.max(3.0 * GRAPH_LANE)) + 2.0 * GRAPH_PAD
        };
        [graph, hash, 0.0, author, date]
    }
}

/// Highlights characters `chars` of `g`, drawn left-centred at `at`.
fn paint_found(
    painter: &egui::Painter,
    g: &Galley,
    at: egui::Pos2,
    chars: std::ops::Range<usize>,
    fill: Color32,
) {
    let x = |i| at.x + g.pos_from_cursor(egui::text::CCursor::new(i)).min.x;
    let (x0, x1) = (x(chars.start), x(chars.end));
    let y = at.y - g.size().y / 2.0;
    let place = Rect::from_x_y_ranges(x0..=x1, y..=y + g.size().y);
    painter.rect_filled(place, 2.0, fill);
}

/// The column headings in `head`: Graph (where it fits), Hash, Subject, Author, Date.
fn headings(ui: &Ui, head: Rect, cols: &Layout) {
    let weak = ui.visuals().weak_text_color();
    let x: [f32; 4] = std::array::from_fn(|k| cols.x[k + 1]);
    let w: [f32; 4] = std::array::from_fn(|k| cols.w[k + 1]);
    let title = ui
        .painter()
        .layout_no_wrap("Graph".into(), FontId::proportional(12.0), weak);
    if title.size().x + 2.0 * GRAPH_PAD <= cols.w[0] {
        ui.painter().galley(
            pos2(
                head.left() + GRAPH_PAD,
                head.center().y - title.size().y / 2.0,
            ),
            title,
            weak,
        );
    }
    for (i, title) in ["Hash", "Subject", "Author", "Date"]
        .into_iter()
        .enumerate()
    {
        let g = cell(
            ui,
            title,
            FontId::proportional(12.0),
            weak,
            w[i] - 2.0 * CELL_PAD,
        );
        ui.painter().galley(
            pos2(x[i] + CELL_PAD, head.center().y - g.size().y / 2.0),
            g,
            weak,
        );
    }
}

/// A tag outlined in `color`, left-centred at `at` and at most `max_width` wide, like a ref
/// badge without the fill. Returns its width.
fn outlined_tag(ui: &Ui, text: &str, color: Color32, at: egui::Pos2, max_width: f32) -> f32 {
    let pad = 5.0;
    let g = cell(
        ui,
        text,
        FontId::proportional(11.5),
        color,
        max_width - 2.0 * pad,
    );
    let size = vec2(g.size().x + 2.0 * pad, 17.0);
    let rect = Rect::from_min_size(pos2(at.x, at.y - size.y / 2.0), size);
    let painter = ui.painter();
    painter.rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(1.0, color),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        pos2(rect.left() + pad, rect.center().y - g.size().y / 2.0),
        g,
        color,
    );
    size.x
}

/// One row of the graph column in `rect`: each line in the colour of the lane it runs in,
/// then the commit's dot, a ring for a merge. `bg` is the row's background over the pane's.
fn paint_graph(
    painter: &egui::Painter,
    rect: Rect,
    row: &GraphRow,
    c: &Colors,
    bg: Option<Color32>,
) {
    if rect.width() <= 0.0 {
        return;
    }
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let x = |lane: usize| rect.left() + GRAPH_PAD + (lane as f32 + 0.5) * GRAPH_LANE;
    let color = |lane: usize| c.lanes[lane % c.lanes.len()];
    let (top, mid, bottom) = (rect.top(), rect.center().y, rect.bottom());
    let stroke = |lane| Stroke::new(1.6, color(lane));
    for line in &row.upper {
        let points = [pos2(x(line.from), top), pos2(x(line.to), mid)];
        painter.line_segment(points, stroke(line.from));
    }
    for line in &row.lower {
        let points = [pos2(x(line.from), mid), pos2(x(line.to), bottom)];
        painter.line_segment(points, stroke(line.to));
    }
    let centre = pos2(x(row.lane), mid);
    if centre.x > rect.right() - GRAPH_PAD {
        // The commit's lane is cut off: point to it from the edge.
        let tip = pos2(rect.right() - 1.0, mid);
        let arrow = vec![tip, tip + vec2(-5.0, -4.0), tip + vec2(-5.0, 4.0)];
        let shape = egui::Shape::convex_polygon(arrow, color(row.lane), Stroke::NONE);
        painter.add(shape);
        return;
    }
    if row.outside {
        // History that goes on outside the log; aside if a line in the log goes down too.
        let weak = Stroke::new(1.4, painter.ctx().global_style().visuals.weak_text_color());
        let aside = row.lower.iter().any(|l| l.from == row.lane);
        let end_x = centre.x + if aside { 0.6 * GRAPH_LANE } else { 0.0 };
        let points = [centre, pos2(end_x, bottom)];
        painter.extend(egui::Shape::dashed_line(&points, weak, 2.0, 2.0));
    }
    const R: f32 = 3.5;
    if row.merge {
        painter.circle_filled(centre, R, c.pane);
        if let Some(bg) = bg {
            painter.circle_filled(centre, R, bg);
        }
        painter.circle_stroke(centre, R, Stroke::new(1.8, color(row.lane)));
    } else {
        painter.circle_filled(centre, R, color(row.lane));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(selected: Option<usize>, other: Option<usize>) -> CommitList {
        CommitList {
            selected,
            other,
            ..CommitList::default()
        }
    }

    fn after(
        mut l: CommitList,
        i: usize,
        add: bool,
        secondary: bool,
    ) -> (Option<usize>, Option<usize>) {
        l.click(i, add, secondary);
        (l.selected, l.other)
    }

    #[test]
    fn a_plain_click_selects_one_row() {
        assert_eq!(
            after(list(Some(1), Some(4)), 2, false, false),
            (Some(2), None)
        );
        assert_eq!(after(list(None, None), 2, false, false), (Some(2), None));
    }

    #[test]
    fn an_added_click_makes_a_pair_in_the_order_selected() {
        let mut l = list(Some(1), None);
        l.click(5, true, false);
        assert_eq!(l.pair(), Some((1, 5)));
        // A third row takes the place of the earlier.
        l.click(3, true, false);
        assert_eq!(l.pair(), Some((5, 3)));
        assert_eq!(after(list(None, None), 2, true, false), (Some(2), None));
    }

    #[test]
    fn an_added_click_on_one_of_a_pair_leaves_the_other() {
        assert_eq!(
            after(list(Some(5), Some(1)), 5, true, false),
            (Some(1), None)
        );
        assert_eq!(
            after(list(Some(5), Some(1)), 1, true, false),
            (Some(5), None)
        );
        assert_eq!(after(list(Some(5), None), 5, true, false), (Some(5), None));
    }

    #[test]
    fn a_right_click_keeps_the_pair_it_is_on() {
        assert_eq!(
            after(list(Some(5), Some(1)), 1, false, true),
            (Some(5), Some(1))
        );
        assert_eq!(
            after(list(Some(5), Some(1)), 2, false, true),
            (Some(2), None)
        );
    }

    #[test]
    fn selecting_by_key_leaves_one_row() {
        let mut l = list(Some(5), Some(1));
        l.select(Some(6));
        assert_eq!((l.selected, l.other), (Some(6), None));
    }
}
