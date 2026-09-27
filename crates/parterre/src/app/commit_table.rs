//! The commit table of the log window and of the blame window's history pane: a graph column of
//! lanes ([`LogGraph`]), then the short hash, the subject with ref badges, the author and the
//! date, one commit per row. Virtualised; the rows are painted directly. Each window says what a
//! row shows ([`Row`]) and what its menu offers; the table keeps the selection in view.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Galley, Id, Rect, ScrollArea, Sense, Stroke, Ui, pos2,
    vec2,
};
use parterre_core::GitRef;
use parterre_core::log_graph::{GraphRow, LogGraph};

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

/// The selected row of a commit table and its scroll position.
#[derive(Debug, Default)]
pub struct CommitList {
    pub selected: Option<usize>,
    /// Scroll the selected row into view in the next frame (only if it is out of view).
    pub reveal: bool,
    /// The scroll offset and the height of the rows in the last frame, for keeping the
    /// selection in view and for paging.
    pub scroll: f32,
    pub height: f32,
}

impl CommitList {
    /// Selects row `i` (or none) and keeps it in view.
    pub fn select(&mut self, i: Option<usize>) {
        self.selected = i;
        self.reveal = true;
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
}

/// What a click on a row asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clicks {
    /// A click with either button: the row is now the selected one.
    pub clicked: Option<usize>,
    pub double_clicked: Option<usize>,
}

/// Where the columns are: x and width of the hash, subject, author and date, after the graph
/// column.
struct Columns {
    graph: f32,
    hash: f32,
    author: f32,
    date: f32,
}

impl Columns {
    fn at(&self, left: f32, width: f32) -> ([f32; 4], [f32; 4]) {
        let (width, left) = (width - self.graph, left + self.graph);
        let subject = (width - self.hash - self.author - self.date).max(80.0);
        let x = [
            left,
            left + self.hash,
            left + self.hash + subject,
            left + self.hash + subject + self.author,
        ];
        (x, [self.hash, subject, self.author, self.date])
    }
}

impl CommitTable<'_> {
    /// The column headings and, unless there are no rows, the rows as `row` describes them;
    /// `menu` fills a row's menu. A click with either button selects the row. Returns the
    /// clicks. With no rows, the window can say why below the headings.
    pub fn show<'r>(
        &self,
        ui: &mut Ui,
        c: &Colors,
        list: &mut CommitList,
        row: impl Fn(usize) -> Row<'r>,
        mut menu: impl FnMut(&mut Ui, usize),
    ) -> Clicks {
        let cols = self.columns(ui, c);
        let weak = ui.visuals().weak_text_color();
        let text = ui.visuals().text_color();
        let mono = FontId::monospace(12.0);
        let body = egui::TextStyle::Body.resolve(ui.style());
        headings(ui, &cols, c);
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
        let output = area.show_rows(ui, ROW, self.rows, |ui, range| {
            let graph = self.graph.rows(range.clone());
            for (i, graph) in range.zip(&graph) {
                let r = row(i);
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
                let selected = list.selected == Some(i);
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
                let (x, w) = cols.at(rect.left(), rect.width());
                let y = rect.center().y;
                let painter = ui.painter();
                let graph_rect = Rect::from_min_size(rect.min, vec2(cols.graph, ROW));
                paint_graph(painter, graph_rect, graph, c, bg);
                let put = |g: Arc<Galley>, x: f32, color| {
                    painter.galley(pos2(x, y - g.size().y / 2.0), g, color);
                };
                if let Some(fill) = r.hash_fill {
                    let cell_rect = Rect::from_x_y_ranges(x[0]..=x[0] + w[0], rect.y_range());
                    painter.rect_filled(cell_rect, 0.0, fill);
                }
                put(
                    cell(ui, &r.hash, mono.clone(), fg_weak, w[0]),
                    x[0] + CELL_PAD,
                    fg_weak,
                );

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
                let response = if over_author {
                    response.on_hover_text(format!("{} <{}>", r.author, r.author_email))
                } else {
                    response
                };
                if response.clicked() || response.secondary_clicked() {
                    clicks.clicked = Some(i);
                }
                if response.double_clicked() {
                    clicks.double_clicked = Some(i);
                }
                egui::Popup::context_menu(&response)
                    .style(crate::menu::style)
                    .show(|ui| {
                        crate::menu::fit_window(ui, |ui| {
                            ui.set_min_width(crate::menu::MIN_WIDTH);
                            menu(ui, i);
                        });
                    });
            }
        });
        list.scroll = output.state.offset.y;
        list.height = output.inner_rect.height();
        if let Some(i) = clicks.clicked {
            list.selected = Some(i);
        }
        clicks
    }

    /// The columns' widths in the width available: the graph only as wide as leaves the
    /// subject its room.
    fn columns(&self, ui: &Ui, c: &Colors) -> Columns {
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
            let room = ui.available_width() - hash - author - date - GRAPH_SUBJECT_ROOM;
            lanes.min(room.max(3.0 * GRAPH_LANE)) + 2.0 * GRAPH_PAD
        };
        Columns {
            graph,
            hash,
            author,
            date,
        }
    }
}

/// The column headings: Graph (where it fits), Hash, Subject, Author, Date.
fn headings(ui: &mut Ui, cols: &Columns, c: &Colors) {
    let weak = ui.visuals().weak_text_color();
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADING), Sense::hover());
    heading_background(ui, head, c);
    let (x, w) = cols.at(head.left(), head.width());
    let title = ui
        .painter()
        .layout_no_wrap("Graph".into(), FontId::proportional(12.0), weak);
    if title.size().x + 2.0 * GRAPH_PAD <= cols.graph {
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
