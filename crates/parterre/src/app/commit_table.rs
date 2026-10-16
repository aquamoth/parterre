use std::collections::BTreeSet;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Galley, Id, Modifiers, Pos2, Rect, ScrollArea, Sense,
    Stroke, Ui, Vec2, pos2, vec2,
};
use parterre_core::columns::{ColumnWidths, Layout};
use parterre_core::glyphs::Glyph;
use parterre_core::log::Found;
use parterre_core::log_graph::{GraphRow, LogGraph};

use super::column_borders::{self, Columns};
use super::table_style::{
    BADGE_FONT_SIZE, BADGE_HEIGHT, BADGE_PAD, Badge, CELL_PAD, Colors, HEADING_HEIGHT, badge, cell,
    heading_background,
};
use crate::theme::Palette;
use crate::{focus, widgets};

pub const ROW_HEIGHT: f32 = 24.0;
const GRAPH_LANE_WIDTH: f32 = 11.0;
const GRAPH_PAD: f32 = 4.0;
/// Lanes further right are cut off.
const GRAPH_MAX_LANES: usize = 24;
const SUBJECT_ROOM_BESIDE_GRAPH: f32 = 220.0;
const SUBJECT_MIN_WIDTH: f32 = 80.0;
const AUTHOR_WIDTH: f32 = 170.0;
const DATE_WIDTH: f32 = 128.0;
const NARROW_LIST_WIDTH: f32 = 720.0;
const NARROW_AUTHOR_WIDTH: f32 = 128.0;
const NARROW_DATE_WIDTH: f32 = 118.0;
const ICON_COLUMN_WIDTH: f32 = 24.0;
const ICON_SIZE: f32 = 14.0;
const HASH_FONT_SIZE: f32 = 12.0;
const HEADING_FONT_SIZE: f32 = 12.0;
const DOT_RADIUS: f32 = 3.5;

mod column {
    pub const GRAPH: usize = 0;
    pub const HASH: usize = 1;
    pub const SUBJECT: usize = 2;
    pub const AUTHOR: usize = 3;
    pub const DATE: usize = 4;
}

#[derive(Debug, Default)]
pub struct CommitList {
    /// The row clicked last, whose details the window shows.
    pub selected: Option<usize>,
    /// The row selected first, where a Shift+click range starts.
    pub anchor: Option<usize>,
    /// Every selected row, in [`Select::Many`] tables.
    pub many: BTreeSet<usize>,
    pub reveal_selected: bool,
    pub open_selected_menu: bool,
    pub scroll_offset: f32,
    pub view_height: f32,
    pub widths: ColumnWidths,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowClick {
    Plain,
    Toggle,
    Range,
    Secondary,
}

impl RowClick {
    fn of(modifiers: Modifiers, secondary: bool) -> RowClick {
        if secondary {
            RowClick::Secondary
        } else if modifiers.shift {
            RowClick::Range
        } else if modifiers.command {
            RowClick::Toggle
        } else {
            RowClick::Plain
        }
    }
}

impl CommitList {
    pub fn select(&mut self, row: Option<usize>) {
        self.selected = row;
        self.anchor = row;
        self.many = row.into_iter().collect();
        self.reveal_selected = true;
    }

    pub fn select_all(&mut self, rows: usize) {
        self.many = (0..rows).collect();
        self.anchor = self.anchor.or(self.selected).or((rows > 0).then_some(0));
        self.selected = self.selected.or(self.anchor);
    }

    /// When both are selected and differ.
    pub fn selected_first_and_last(&self) -> Option<(usize, usize)> {
        let (first, last) = (self.anchor?, self.selected?);
        let both = self.many.contains(&first) && self.many.contains(&last);
        (first != last && both).then_some((first, last))
    }

    pub fn is_selected(&self, row: usize) -> bool {
        self.selected == Some(row) || self.many.contains(&row)
    }

    fn click(&mut self, row: usize, click: RowClick, select: Select) {
        match select {
            Select::One => {
                if !(click == RowClick::Secondary && self.is_selected(row)) {
                    self.selected = Some(row);
                }
            }
            Select::Many => self.click_many(row, click),
        }
    }

    fn select_row(&mut self, row: usize) {
        self.many = BTreeSet::from([row]);
        self.selected = Some(row);
        self.anchor = Some(row);
    }

    fn click_many(&mut self, row: usize, click: RowClick) {
        // A row selected without a click, as a window opens with its first.
        if self.many.is_empty()
            && let Some(selected) = self.selected
        {
            self.many.insert(selected);
            self.anchor = Some(selected);
        }
        match click {
            RowClick::Secondary if self.is_selected(row) => {}
            RowClick::Range => {
                let from = self.anchor.or(self.selected).unwrap_or(row);
                self.many = (from.min(row)..=from.max(row)).collect();
                self.anchor = Some(from);
                self.selected = Some(row);
            }
            RowClick::Toggle => {
                if self.many.remove(&row) {
                    if self.anchor == Some(row) {
                        self.anchor = self.many.first().copied();
                    }
                    if self.selected == Some(row) {
                        self.selected = self.anchor;
                    }
                } else {
                    self.many.insert(row);
                    self.anchor = self.anchor.filter(|a| self.many.contains(a)).or(Some(row));
                    self.selected = Some(row);
                }
            }
            RowClick::Plain | RowClick::Secondary => self.select_row(row),
        }
    }

    /// Those in view, less one.
    pub fn rows_per_page(&self) -> usize {
        ((self.view_height / ROW_HEIGHT).floor() as usize)
            .saturating_sub(1)
            .max(1)
    }
}

#[derive(Default)]
pub struct Row<'a> {
    /// The glyph, its colour, and its name on hover.
    pub icon: Option<(Glyph, Color32, &'a str)>,
    pub hash: String,
    pub hash_fill: Option<Color32>,
    pub marked_for_comparison: bool,
    pub refs: Vec<Badge<'a>>,
    pub outlined_label: Option<&'a str>,
    pub subject: &'a str,
    /// Weak text after the subject.
    pub note: Option<String>,
    pub author: &'a str,
    pub author_email: &'a str,
    pub date: &'a str,
    pub greyed: bool,
    pub found: Found,
}

pub struct CommitTable<'a> {
    /// One per list shown, so that each keeps its own scroll position.
    pub id: Id,
    pub rows: usize,
    pub graph: &'a LogGraph,
    pub abbrev_len: usize,
    pub palette: &'a Palette,
    pub select: Select,
    /// An icon column before the graph, for [`Row::icon`].
    pub icons: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Select {
    #[default]
    One,
    Many,
}

pub type SubjectTip<'a> = dyn FnMut(&mut Ui, usize) + 'a;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clicks {
    /// With either button.
    pub clicked: Option<usize>,
    pub double_clicked: Option<usize>,
    /// On a row's icon, which neither selects the row nor counts towards a double-click.
    pub icon: Option<usize>,
}

/// The stop for Tab of the table `table`.
pub(super) fn list_stop_id(table: Id) -> Id {
    table.with("stop")
}

fn columns(layout: &Layout) -> Columns<'_> {
    Columns {
        layout,
        flex: column::SUBJECT,
        flex_min: SUBJECT_MIN_WIDTH,
    }
}

impl CommitTable<'_> {
    /// With no rows, only the headings show, and the window can say why below them.
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
            list.widths.reset(0, column::SUBJECT);
        }
        let icon_column = self.icon_column_width();
        let defaults = self.default_widths(ui, &list.widths);
        let layout = |widths: &ColumnWidths, rect: Rect| {
            let (left, width) = (rect.left() + icon_column, rect.width() - icon_column);
            widths.layout(&defaults, column::SUBJECT, SUBJECT_MIN_WIDTH, left, width)
        };
        let weak = ui.visuals().weak_text_color();
        let text = ui.visuals().text_color();
        let mono = FontId::monospace(HASH_FONT_SIZE);
        let body = egui::TextStyle::Body.resolve(ui.style());
        let (head, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), HEADING_HEIGHT), Sense::hover());
        heading_background(ui, head, c);
        // Laid out again after the drag, so that the columns follow the pointer in this frame.
        let before = layout(&list.widths, head);
        let active = column_borders::drag(ui, self.id, head, &columns(&before), &mut list.widths);
        let cols = layout(&list.widths, head);
        headings(ui, head, &cols);
        column_borders::paint(ui, head, &columns(&cols), active, c);
        if self.rows == 0 {
            return Clicks::default();
        }

        ui.spacing_mut().item_spacing.y = 0.0;
        // The rows are one stop for Tab, which the keys move the selection in.
        let stop_id = list_stop_id(self.id);
        let mut clicks = Clicks::default();
        let keys = widgets::list_keys(ui, stop_id, list.selected, self.rows, list.rows_per_page());
        if let Some(i) = keys.select {
            list.select(Some(i));
            clicks.clicked = Some(i);
        }
        if keys.enter {
            clicks.double_clicked = list.selected;
        }
        if keys.menu {
            list.open_selected_menu = true;
            list.reveal_selected = true;
        }
        let cue = focus::shows_own_by_id(ui.ctx(), stop_id).then(|| list.selected.unwrap_or(0));
        let mut cued = false;
        // As short as one row, not egui's default: a dialog's list of one commit is that tall.
        let mut area = ScrollArea::vertical()
            .id_salt(self.id)
            .auto_shrink(false)
            .min_scrolled_height(ROW_HEIGHT);
        if list.reveal_selected
            && let Some(selected) = list.selected
        {
            list.reveal_selected = false;
            let top = selected as f32 * ROW_HEIGHT;
            // Before the rows were first shown: the room they will have.
            let height = if list.view_height > 0.0 {
                list.view_height
            } else {
                ui.available_height()
            };
            let height = height.max(ROW_HEIGHT);
            if top < list.scroll_offset {
                area = area.vertical_scroll_offset(top);
            } else if top + ROW_HEIGHT > list.scroll_offset + height {
                area = area.vertical_scroll_offset(top + ROW_HEIGHT - height);
            }
        }
        let modifiers = ui.input(|i| i.modifiers);
        let stop = widgets::list_stop(ui, stop_id, ui.available_rect_before_wrap());
        let output = area.show_rows(ui, ROW_HEIGHT, self.rows, |ui, range| {
            let graph = self.graph.rows(range.clone());
            for (i, graph_row) in range.zip(&graph) {
                let r = row(i);
                // Not a stop of its own: the table is one.
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::CLICK);
                focus::take_on_row_click(&response, stop_id);
                let selected = list.is_selected(i);
                let fill = if selected {
                    Some(c.selected_bg)
                } else if response.hovered() {
                    Some(c.hover)
                } else if i % 2 == 1 {
                    Some(c.stripe)
                } else {
                    None
                };
                if let Some(fill) = fill {
                    ui.painter().rect_filled(rect, 0.0, fill);
                }
                if cue == Some(i) {
                    widgets::paint_list_cue(ui, rect);
                    cued = true;
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
                let cols = layout(&list.widths, rect);
                let (x, w) = (&cols.x, &cols.w);
                let span =
                    |col: usize| Rect::from_x_y_ranges(x[col]..=x[col] + w[col], rect.y_range());
                let y = rect.center().y;
                let painter = ui.painter();
                paint_graph(painter, span(column::GRAPH), graph_row, c, fill);
                if let Some((glyph, color, _)) = r.icon {
                    let color = if r.greyed {
                        color.gamma_multiply(0.6)
                    } else {
                        color
                    };
                    let at = pos2(rect.left() + icon_column / 2.0, y);
                    let icon = Rect::from_center_size(at, Vec2::splat(ICON_SIZE));
                    widgets::paint_glyph(painter, icon, glyph, color);
                }
                let put = |g: Arc<Galley>, left: f32, color| {
                    painter.galley(pos2(left, y - g.size().y / 2.0), g, color);
                };
                if let Some(fill) = r.hash_fill {
                    painter.rect_filled(span(column::HASH), 0.0, fill);
                }
                let hash_x = x[column::HASH] + CELL_PAD;
                let g = cell(ui, &r.hash, mono.clone(), fg_weak, w[column::HASH]);
                let digits = r.found.hash_digits.min(r.hash.chars().count());
                if digits > 0 {
                    paint_found(painter, &g, pos2(hash_x, y), 0..digits, c.found);
                }
                put(g, hash_x, fg_weak);

                let mut left = x[column::SUBJECT] + CELL_PAD;
                let right = x[column::SUBJECT] + w[column::SUBJECT] - CELL_PAD;
                if r.marked_for_comparison {
                    let ribbon = Rect::from_center_size(pos2(left + 5.0, y), vec2(10.0, 15.0));
                    widgets::paint_ribbon(painter, ribbon, self.palette.marked, Stroke::NONE);
                    left += 16.0;
                }
                for b in &r.refs {
                    if left >= right {
                        break;
                    }
                    left += badge(ui, b, self.palette, pos2(left, y), right - left) + 4.0;
                }
                if let Some(label) = r.outlined_label
                    && left < right
                {
                    left += outlined_label(ui, label, fg_weak, pos2(left, y), right - left) + 6.0;
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
                    cell(
                        ui,
                        r.author,
                        body.clone(),
                        fg,
                        w[column::AUTHOR] - 2.0 * CELL_PAD,
                    ),
                    x[column::AUTHOR] + CELL_PAD,
                    fg,
                );
                put(
                    cell(
                        ui,
                        r.date,
                        body.clone(),
                        fg_weak,
                        w[column::DATE] - 2.0 * CELL_PAD,
                    ),
                    x[column::DATE] + CELL_PAD,
                    fg_weak,
                );
                let hover = response.hover_pos();
                let over = |rect: Rect| hover.is_some_and(|p| rect.contains(p));
                let over_author = !r.author.is_empty() && over(span(column::AUTHOR));
                let over_subject = over(span(column::SUBJECT));
                let icon_rect =
                    Rect::from_x_y_ranges(rect.left()..=rect.left() + icon_column, rect.y_range());
                let over_icon = r.icon.is_some() && over(icon_rect);
                if over_icon {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                let response = match &mut subject_tip {
                    _ if over_icon => response.on_hover_text(r.icon.map_or("", |i| i.2)),
                    _ if over_author => {
                        response.on_hover_text(format!("{} <{}>", r.author, r.author_email))
                    }
                    Some(tip) if over_subject => response.on_hover_ui(|ui| tip(ui, i)),
                    _ => response,
                };
                if over_icon && response.clicked() {
                    clicks.icon = Some(i);
                } else if response.clicked() || response.secondary_clicked() {
                    clicks.clicked = Some(i);
                    let click = RowClick::of(modifiers, response.secondary_clicked());
                    list.click(i, click, self.select);
                }
                if response.double_clicked() && !over_icon {
                    clicks.double_clicked = Some(i);
                }
                let by_key = list.open_selected_menu && list.selected == Some(i);
                let at = pos2(rect.left() + icon_column + CELL_PAD, rect.bottom());
                crate::menu::context_menu(&response, by_key.then_some(at))
                    .style(crate::menu::style)
                    .show(|ui| {
                        crate::menu::fit_window(ui, |ui| {
                            crate::usage::menu(ui.ctx(), crate::usage::Menu::Commit);
                            ui.set_min_width(crate::menu::MIN_WIDTH);
                            menu(ui, i, list);
                        });
                    });
            }
        });
        list.open_selected_menu = false;
        list.scroll_offset = output.state.offset.y;
        list.view_height = output.inner_rect.height();
        widgets::list_stop_ring(ui, &stop, cued);
        clicks
    }

    fn icon_column_width(&self) -> f32 {
        if self.icons { ICON_COLUMN_WIDTH } else { 0.0 }
    }

    /// The graph only as wide as leaves the subject its room beside the other columns.
    fn default_widths(&self, ui: &Ui, widths: &ColumnWidths) -> [f32; 5] {
        let digit = ui
            .painter()
            .layout_no_wrap(
                "0".into(),
                FontId::monospace(HASH_FONT_SIZE),
                Color32::PLACEHOLDER,
            )
            .size()
            .x;
        let hash = digit * self.abbrev_len as f32 + 2.0 * CELL_PAD + 2.0;
        let (author, date) = if ui.available_width() < NARROW_LIST_WIDTH {
            (NARROW_AUTHOR_WIDTH, NARROW_DATE_WIDTH)
        } else {
            (AUTHOR_WIDTH, DATE_WIDTH)
        };
        let graph = if self.graph.lanes == 0 {
            0.0
        } else {
            let lanes = self.graph.lanes.min(GRAPH_MAX_LANES) as f32 * GRAPH_LANE_WIDTH;
            let others = widths.get(column::HASH, hash)
                + widths.get(column::AUTHOR, author)
                + widths.get(column::DATE, date);
            let room = ui.available_width()
                - self.icon_column_width()
                - others
                - SUBJECT_ROOM_BESIDE_GRAPH;
            lanes.min(room.max(3.0 * GRAPH_LANE_WIDTH)) + 2.0 * GRAPH_PAD
        };
        [graph, hash, 0.0, author, date]
    }
}

fn paint_found(
    painter: &egui::Painter,
    g: &Galley,
    left_center: Pos2,
    chars: std::ops::Range<usize>,
    fill: Color32,
) {
    let x = |i| left_center.x + g.pos_from_cursor(egui::text::CCursor::new(i)).min.x;
    let top = left_center.y - g.size().y / 2.0;
    let place = Rect::from_x_y_ranges(x(chars.start)..=x(chars.end), top..=top + g.size().y);
    painter.rect_filled(place, 2.0, fill);
}

fn headings(ui: &Ui, head: Rect, cols: &Layout) {
    let weak = ui.visuals().weak_text_color();
    let font = FontId::proportional(HEADING_FONT_SIZE);
    let painter = ui.painter();
    let graph = painter.layout_no_wrap("Graph".into(), font.clone(), weak);
    if graph.size().x + 2.0 * GRAPH_PAD <= cols.w[column::GRAPH] {
        let at = pos2(
            cols.x[column::GRAPH] + GRAPH_PAD,
            head.center().y - graph.size().y / 2.0,
        );
        painter.galley(at, graph, weak);
    }
    let titles = [
        (column::HASH, "Hash"),
        (column::SUBJECT, "Subject"),
        (column::AUTHOR, "Author"),
        (column::DATE, "Date"),
    ];
    for (col, title) in titles {
        let g = cell(ui, title, font.clone(), weak, cols.w[col] - 2.0 * CELL_PAD);
        let at = pos2(cols.x[col] + CELL_PAD, head.center().y - g.size().y / 2.0);
        painter.galley(at, g, weak);
    }
}

/// Like a ref badge without the fill. Returns its width.
fn outlined_label(ui: &Ui, text: &str, color: Color32, left_center: Pos2, max_width: f32) -> f32 {
    let font = FontId::proportional(BADGE_FONT_SIZE);
    let g = cell(ui, text, font, color, max_width - 2.0 * BADGE_PAD);
    let size = vec2(g.size().x + 2.0 * BADGE_PAD, BADGE_HEIGHT);
    let rect = Rect::from_min_size(pos2(left_center.x, left_center.y - size.y / 2.0), size);
    let painter = ui.painter();
    painter.rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(1.0, color),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        pos2(rect.left() + BADGE_PAD, rect.center().y - g.size().y / 2.0),
        g,
        color,
    );
    size.x
}

/// `row_fill` is the row's background over the pane's.
fn paint_graph(
    painter: &egui::Painter,
    rect: Rect,
    row: &GraphRow,
    c: &Colors,
    row_fill: Option<Color32>,
) {
    if rect.width() <= 0.0 {
        return;
    }
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let x = |lane: usize| rect.left() + GRAPH_PAD + (lane as f32 + 0.5) * GRAPH_LANE_WIDTH;
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
        let end_x = centre.x + if aside { 0.6 * GRAPH_LANE_WIDTH } else { 0.0 };
        let points = [centre, pos2(end_x, bottom)];
        painter.extend(egui::Shape::dashed_line(&points, weak, 2.0, 2.0));
    }
    if row.merge {
        painter.circle_filled(centre, DOT_RADIUS, c.pane);
        if let Some(fill) = row_fill {
            painter.circle_filled(centre, DOT_RADIUS, fill);
        }
        painter.circle_stroke(centre, DOT_RADIUS, Stroke::new(1.8, color(row.lane)));
    } else {
        painter.circle_filled(centre, DOT_RADIUS, color(row.lane));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_selects_one_row_and_a_right_click_keeps_it() {
        let mut l = CommitList::default();
        l.click(2, RowClick::Plain, Select::One);
        assert_eq!(l.selected, Some(2));
        l.click(2, RowClick::Secondary, Select::One);
        assert_eq!(l.selected, Some(2));
        l.click(4, RowClick::Secondary, Select::One);
        assert_eq!(l.selected, Some(4));
    }

    fn many(l: &CommitList) -> Vec<usize> {
        l.many.iter().copied().collect()
    }

    #[test]
    fn many_rows_are_added_toggled_and_ranged() {
        let mut l = CommitList::default();
        l.click_many(2, RowClick::Plain);
        assert_eq!(many(&l), [2]);
        l.click_many(5, RowClick::Toggle);
        assert_eq!(many(&l), [2, 5]);
        // From the row selected first, with a plain click.
        l.click_many(4, RowClick::Range);
        assert_eq!(many(&l), [2, 3, 4]);
        l.click_many(3, RowClick::Toggle);
        assert_eq!(many(&l), [2, 4]);
        assert!(!l.is_selected(3));
        // A right-click keeps a selection it is on, and replaces one it isn't.
        l.click_many(4, RowClick::Secondary);
        assert_eq!(many(&l), [2, 4]);
        l.click_many(0, RowClick::Secondary);
        assert_eq!(many(&l), [0]);
        l.click_many(1, RowClick::Plain);
        assert_eq!(many(&l), [1]);
    }

    #[test]
    fn the_pair_is_the_row_selected_first_and_the_one_clicked_last() {
        let mut l = CommitList::default();
        l.click_many(1, RowClick::Plain);
        assert_eq!(l.selected_first_and_last(), None);
        l.click_many(5, RowClick::Toggle);
        assert_eq!(l.selected_first_and_last(), Some((1, 5)));
        l.click_many(3, RowClick::Toggle);
        assert_eq!(l.selected_first_and_last(), Some((1, 3)));
        l.click_many(6, RowClick::Range);
        assert_eq!(
            (l.selected_first_and_last(), l.selected),
            (Some((1, 6)), Some(6))
        );
        // Taking out the first: the next selected row is first.
        l.click_many(1, RowClick::Toggle);
        assert_eq!(l.selected_first_and_last(), Some((2, 6)));
    }

    #[test]
    fn a_row_selected_without_a_click_is_where_adding_starts() {
        let mut l = CommitList {
            selected: Some(0),
            ..CommitList::default()
        };
        l.click_many(3, RowClick::Toggle);
        assert_eq!(
            (many(&l), l.selected_first_and_last()),
            (vec![0, 3], Some((0, 3)))
        );
    }

    #[test]
    fn selecting_by_key_leaves_one_row() {
        let mut l = CommitList::default();
        l.click_many(1, RowClick::Plain);
        l.click_many(4, RowClick::Range);
        l.select(Some(6));
        assert_eq!(
            (many(&l), l.selected, l.selected_first_and_last()),
            (vec![6], Some(6), None)
        );
        l.select_all(8);
        assert_eq!((many(&l).len(), l.selected), (8, Some(6)));
    }

    #[test]
    fn modifiers_pick_the_kind_of_click_and_a_right_click_ignores_them() {
        let both = Modifiers::SHIFT | Modifiers::COMMAND;
        assert_eq!(RowClick::of(Modifiers::NONE, false), RowClick::Plain);
        assert_eq!(RowClick::of(Modifiers::COMMAND, false), RowClick::Toggle);
        assert_eq!(RowClick::of(both, false), RowClick::Range);
        assert_eq!(RowClick::of(both, true), RowClick::Secondary);
    }
}
