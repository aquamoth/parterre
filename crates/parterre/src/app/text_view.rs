use std::ops::Range;
use std::sync::{Arc, mpsc};

use eframe::egui::scroll_area::ScrollAreaOutput;
use eframe::egui::text::{CCursor, LayoutJob, TextFormat};
use eframe::egui::{
    self, Color32, CornerRadius, FontId, Painter, Pos2, Rangef, Rect, Sense, Stroke, StrokeKind,
    Ui, UiBuilder, pos2, vec2,
};
use parterre_core::Repo;
use parterre_core::file_diff::{Rev, raw_offset};
use parterre_core::find::{self, Step};

use super::table_style;
use crate::{keys, widgets, worker};

pub(super) const TOOLBAR_HEIGHT: f32 = 44.0;
pub(super) const HEADER_HEIGHT: f32 = 34.0;
pub(super) const TOOL_GAP: f32 = 14.0;
pub(super) const OVERVIEW_WIDTH: f32 = 14.0;
pub(super) const SCROLLBAR_HEIGHT: f32 = 10.0;
/// Shown left of a place scrolled to sideways.
pub(super) const SIDEWAYS_MARGIN: f32 = 80.0;
pub(super) const LINE_END: usize = usize::MAX;
const FONT_SIZE: f32 = 13.0;
const FIND_FIELD_MIN_WIDTH: f32 = 60.0;
const FIND_FIELD_MAX_WIDTH: f32 = 380.0;
/// Kept free on either side of the find field.
const FIND_FIELD_MARGIN: f32 = 8.0;

#[derive(Debug)]
pub(super) enum Load<T> {
    Loading(mpsc::Receiver<Result<T, String>>),
    Ready(Box<T>),
    Failed(String),
}

impl<T> Load<T> {
    pub(super) fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(r) => Some(r),
            _ => None,
        }
    }

    pub(super) fn is_loading(&self) -> bool {
        matches!(self, Load::Loading(_))
    }

    pub(super) fn received(&self) -> Option<Result<T, String>> {
        match self {
            Load::Loading(rx) => {
                worker::received(rx, || Err("loading stopped unexpectedly".into()))
            }
            _ => None,
        }
    }
}

pub(super) fn text_font() -> FontId {
    FontId::monospace(FONT_SIZE)
}

pub(super) struct TextMetrics {
    pub(super) row_h: f32,
    pub(super) char_w: f32,
}

impl TextMetrics {
    pub(super) fn new(ctx: &egui::Context) -> TextMetrics {
        let font = text_font();
        let (row_h, char_w) =
            ctx.fonts_mut(|f| (f.row_height(&font).ceil() + 3.0, f.glyph_width(&font, '0')));
        TextMetrics { row_h, char_w }
    }
}

/// The width of the longest of `lines` line numbers.
pub(super) fn line_numbers_width(lines: usize, char_w: f32) -> f32 {
    (lines.max(1).ilog10() + 1) as f32 * char_w
}

pub(super) fn file_window_title(path: &str, rev: Option<Rev>, repo: &Repo, window: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let rev = match rev {
        Some(Rev::Commit(oid)) => oid.short(repo.abbrev_len),
        Some(Rev::WorkingTree) => "working tree".to_owned(),
        None => String::new(),
    };
    format!("{file} ({rev}) – {} – {window}", repo.display_name())
}

/// The toolbar across the top of `ui`, and a row in it for the tools.
pub(super) fn toolbar_row(ui: &mut Ui) -> (Rect, Ui) {
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), TOOLBAR_HEIGHT), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, ui.visuals().panel_fill);
    let mut row = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(vec2(10.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    row.spacing_mut().item_spacing.x = 4.0;
    (rect, row)
}

pub(super) fn message(ui: &Ui, body: Rect, text: &str, color: Color32) {
    let g = ui
        .painter()
        .layout_job(with_arrows(text, &FontId::proportional(14.0), color));
    ui.painter().galley(body.min + vec2(20.0, 20.0), g, color);
}

/// `text` in `font`, but its arrows in the monospace font: the proportional one has none.
pub(super) fn with_arrows(text: &str, font: &FontId, color: Color32) -> LayoutJob {
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

/// Find in the text (Ctrl+F), in one window alone.
#[derive(Debug)]
pub(super) struct Find<P> {
    pub(super) query: String,
    /// Offered by Ctrl+F after Esc cleared the field.
    pub(super) last_query: String,
    pub(super) matches: Vec<P>,
    /// The place gone to, an index into `matches`.
    pub(super) current: Option<usize>,
    /// Focus the field, the query selected, in the next frame.
    focus_field: bool,
    reveal_current: bool,
}

impl<P> Default for Find<P> {
    fn default() -> Self {
        Find {
            query: String::new(),
            last_query: String::new(),
            matches: Vec::new(),
            current: None,
            focus_field: false,
            reveal_current: false,
        }
    }
}

impl<P> Find<P> {
    pub(super) fn current_match(&self) -> Option<&P> {
        self.matches.get(self.current?)
    }

    /// The place just gone to, to scroll into view: once.
    pub(super) fn take_reveal(&mut self) -> Option<&P> {
        if std::mem::take(&mut self.reveal_current) {
            self.current_match()
        } else {
            None
        }
    }
}

/// What a find field shows beside the query: how many places it found, and which is current.
pub(super) fn match_count(current: Option<usize>, matches: usize) -> String {
    match (current, matches) {
        (_, 0) => "No matches".to_owned(),
        (Some(c), n) => format!("{} of {n}", c + 1),
        (None, 1) => "1 match".to_owned(),
        (None, n) => format!("{n} matches"),
    }
}

/// The find field's width in `room`: a narrow window squeezes the field rather than the tools.
pub(super) fn find_field_width(room: f32) -> f32 {
    (room - 2.0 * FIND_FIELD_MARGIN).clamp(FIND_FIELD_MIN_WIDTH, FIND_FIELD_MAX_WIDTH)
}

/// Where a find field `width` wide starts in `room`: centred on `centre`, the window's, if it
/// fits there, as in the other windows; else in the middle of the room.
pub(super) fn find_field_left(room: Rangef, centre: f32, width: f32) -> f32 {
    let (lo, hi) = (
        room.min + FIND_FIELD_MARGIN,
        room.max - FIND_FIELD_MARGIN - width,
    );
    if lo <= hi {
        (centre - width / 2.0).clamp(lo, hi)
    } else {
        room.center() - width / 2.0
    }
}

/// A window whose find field finds in its text ([`Find`]).
pub(super) trait FindInText {
    type Place: find::Place;

    fn find(&self) -> &Find<Self::Place>;
    fn find_mut(&mut self) -> &mut Find<Self::Place>;
    fn find_id(&self) -> egui::Id;
    /// Every place the query occurs, in reading order.
    fn places(&self) -> Vec<Self::Place>;
    /// The first row drawn last frame: finding goes on from there.
    fn top(&self) -> usize;
    /// The chosen characters, if they are within one line.
    fn chosen_in_line(&self) -> Option<String>;
    /// The chosen text as in the file, tabs kept.
    fn selected_text(&self) -> Option<String>;

    /// Finds the query again, with nothing gone to.
    fn refind(&mut self) {
        let matches = self.places();
        let state = self.find_mut();
        state.matches = matches;
        state.current = None;
    }

    /// The query was typed: goes to its first place from the top of the view on.
    fn query_changed(&mut self) {
        self.refind();
        let top = self.top();
        let state = self.find_mut();
        if !state.query.is_empty() {
            state.last_query = state.query.clone();
        }
        state.current = find::first_from(&state.matches, top);
        state.reveal_current = true;
    }

    /// Goes to the next place, or the previous one, round the ends.
    fn find_step(&mut self, step: Step) {
        let top = self.top();
        let state = self.find_mut();
        state.current = find::step(&state.matches, state.current, step, top);
        state.reveal_current = true;
    }

    /// Focuses the find field with the chosen characters, if they are within one line, or else
    /// the last query, selected so that typing replaces it.
    fn open_find(&mut self) {
        let query = self
            .chosen_in_line()
            .unwrap_or_else(|| self.find().last_query.clone());
        if query != self.find().query {
            self.find_mut().query = query;
            self.query_changed();
        }
        self.find_mut().focus_field = true;
    }

    /// Empties the find field, and leaves it.
    fn close_find(&mut self, ctx: &egui::Context) {
        self.find_mut().query.clear();
        self.refind();
        ctx.memory_mut(|m| m.surrender_focus(self.find_id()));
    }

    fn find_field(&mut self, ui: &mut Ui, width: f32, hint: &str) {
        let id = self.find_id();
        let state = self.find_mut();
        let count = match_count(state.current, state.matches.len());
        let focus = std::mem::take(&mut state.focus_field);
        let field = widgets::Find {
            id,
            width,
            hint,
            count: &count,
            keys: &keys::find_field_keys(),
            focus,
            select: focus,
        };
        let found = widgets::find_field(ui, &field, &mut state.query);
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

    /// Ctrl+F, F3 and Shift+F3, wherever the focus is. True while the find field has the keys.
    fn find_keys(&mut self, ui: &Ui) -> bool {
        let id = self.find_id();
        // egui drops the focus on Esc before the frame starts.
        let in_field = ui.memory(|m| m.has_focus(id) || m.had_focus_last_frame(id));
        if ui.input_mut(|i| keys::FIND.consume(i)) {
            self.open_find();
        }
        // The previous place first: egui takes F3 for Shift+F3 too, and ⌘G for ⇧⌘G.
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
        in_field
    }

    /// What Ctrl+C copies from the text: nothing while the find field has the focus, as it
    /// copies from itself.
    fn copied(&self, ui: &Ui) -> Option<String> {
        let copy = ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        let in_field = ui.memory(|m| m.has_focus(self.find_id()));
        (copy && !in_field).then(|| self.selected_text()).flatten()
    }
}

/// The display columns chosen in `row` by characters chosen from `start` to `end`, each a row
/// and a display column, in order.
pub(super) fn chosen_columns(
    start: (usize, usize),
    end: (usize, usize),
    row: usize,
) -> Option<Range<usize>> {
    if row < start.0 || row > end.0 {
        return None;
    }
    let from = if row == start.0 { start.1 } else { 0 };
    let to = if row == end.0 { end.1 } else { LINE_END };
    Some(from..to)
}

/// What display `columns` of a line show of its `raw` text.
pub(super) fn raw_slice(raw: &str, columns: Range<usize>) -> &str {
    let start = raw_offset(raw, columns.start);
    let end = if columns.end == LINE_END {
        raw.len()
    } else {
        raw_offset(raw, columns.end)
    };
    &raw[start..end.max(start)]
}

/// A line's text as laid out, and where it was drawn.
pub(super) struct TextAt {
    pub(super) galley: Arc<egui::Galley>,
    pub(super) origin: Pos2,
}

impl TextAt {
    pub(super) fn column_at(&self, p: Pos2) -> usize {
        if p.x <= self.origin.x {
            0
        } else {
            self.galley.cursor_from_pos(p - self.origin).index.0
        }
    }

    fn x_of(&self, col: usize) -> f32 {
        self.origin.x + self.galley.pos_from_cursor(CCursor::new(col)).min.x
    }
}

/// The row a line's text is painted in.
pub(super) struct TextRow<'a> {
    pub(super) y_range: Rangef,
    pub(super) row_h: f32,
    pub(super) colors: &'a Colors,
}

/// Paints a line's `text` over the places found in it (display columns, and whether it is the
/// one gone to) and its `chosen` display columns.
pub(super) fn paint_text(
    painter: &Painter,
    text: &TextAt,
    row: &TextRow,
    found: impl IntoIterator<Item = (Range<usize>, bool)>,
    chosen: Option<Range<usize>>,
) {
    let c = row.colors;
    for (columns, current) in found {
        let fill = if current { c.found_current } else { c.found };
        let xs = text.x_of(columns.start)..=text.x_of(columns.end);
        painter.rect_filled(Rect::from_x_y_ranges(xs, row.y_range), 0.0, fill);
    }
    if let Some(columns) = chosen {
        let end = if columns.end == LINE_END {
            // Past the end, a little, to show the line break is included.
            text.origin.x + text.galley.size().x + row.row_h / 2.0
        } else {
            text.x_of(columns.end)
        };
        let xs = text.x_of(columns.start)..=end;
        painter.rect_filled(Rect::from_x_y_ranges(xs, row.y_range), 0.0, c.selection);
    }
    painter.galley(text.origin, text.galley.clone(), c.text);
}

/// The sideways scroll offset that brings display `columns` into view, if they are out of view
/// at `scroll_x` in text `text_w` wide.
pub(super) fn reveal_columns(
    scroll_x: f32,
    columns: &Range<usize>,
    char_w: f32,
    text_w: f32,
) -> Option<f32> {
    let (x0, x1) = (columns.start as f32 * char_w, columns.end as f32 * char_w);
    let in_view = text_w - char_w;
    let start = if x1 < in_view {
        0.0
    } else {
        x0 - SIDEWAYS_MARGIN
    };
    (x0 < scroll_x || x1 > scroll_x + in_view).then_some(start)
}

/// Scrolls sideways with the wheel or trackpad while the pointer is over `area`.
pub(super) fn scroll_sideways(ui: &Ui, area: Rect, scroll_x: &mut f32, max: f32) {
    if ui.rect_contains_pointer(area) {
        *scroll_x -= ui.input(|i| i.smooth_scroll_delta.x);
    }
    *scroll_x = scroll_x.clamp(0.0, max);
}

/// The scroll offset that puts row `i` a third of the way down the `height` in view.
pub(super) fn third_of_the_way_down(i: usize, height: f32, row_h: f32) -> f32 {
    let above = (height / row_h / 3.0).floor();
    ((i as f32 - above) * row_h).max(0.0)
}

/// [`third_of_the_way_down`], if row `i` is out of view at `scroll` in text `height` tall.
pub(super) fn reveal(i: usize, scroll: f32, height: f32, row_h: f32) -> Option<f32> {
    let y = i as f32 * row_h;
    (y < scroll || y + row_h > scroll + height).then(|| third_of_the_way_down(i, height, row_h))
}

/// A scroll area's offset, the height in view, and the height of all its rows.
#[derive(Clone, Copy)]
pub(super) struct ScrollView {
    pub(super) offset: f32,
    pub(super) height: f32,
    pub(super) content: f32,
}

impl ScrollView {
    pub(super) fn of<R>(out: &ScrollAreaOutput<R>) -> ScrollView {
        ScrollView {
            offset: out.state.offset.y,
            height: out.inner_rect.height(),
            content: out.content_size.y,
        }
    }
}

/// At most the row height: a text shorter than the strip has its marks level with its rows.
pub(super) fn overview_scale(strip: Rect, rows: usize, row_h: f32) -> f32 {
    (strip.height() / rows.max(1) as f32).min(row_h)
}

pub(super) fn overview_background(ui: &Ui, strip: Rect, c: &Colors) {
    let painter = ui.painter();
    painter.rect_filled(strip, 0.0, ui.visuals().panel_fill);
    painter.vline(
        strip.left() + 0.5,
        strip.y_range(),
        Stroke::new(1.0, c.line),
    );
}

/// Marks `rows` in an overview strip, at least 2 points tall.
pub(super) fn overview_mark(ui: &Ui, strip: Rect, scale: f32, rows: Range<usize>, color: Color32) {
    let (y0, y1) = (rows.start as f32 * scale, rows.end as f32 * scale);
    let mark = Rect::from_min_max(
        pos2(strip.left() + 3.0, strip.top() + y0),
        pos2(strip.right() - 2.0, strip.top() + y1.max(y0 + 2.0)),
    );
    ui.painter().rect_filled(mark, 0.0, color);
}

/// Marks the rows places were found in, given in order.
pub(super) fn overview_found(
    ui: &Ui,
    strip: Rect,
    scale: f32,
    rows: impl IntoIterator<Item = usize>,
    c: &Colors,
) {
    let mut marked = None;
    for i in rows {
        if marked != Some(i) {
            marked = Some(i);
            overview_mark(ui, strip, scale, i..i + 1, c.found_mark);
        }
    }
}

pub(super) fn overview_view(ui: &Ui, strip: Rect, view: ScrollView, c: &Colors) {
    if view.content > view.height {
        let y0 = strip.top() + view.offset / view.content * strip.height();
        let y1 = strip.top() + (view.offset + view.height) / view.content * strip.height();
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

/// A click or drag on an overview strip: the scroll offset that puts that place in the middle.
pub(super) fn overview_scroll(
    ui: &Ui,
    id: egui::Id,
    strip: Rect,
    rows: usize,
    row_h: f32,
    view: ScrollView,
) -> Option<f32> {
    let response = ui.interact(strip, id, crate::focus::pointer_only());
    let p = response.interact_pointer_pos()?;
    if !(response.clicked() || response.dragged()) || view.content <= view.height {
        return None;
    }
    ui.ctx().request_repaint();
    let row = (p.y - strip.top()) / overview_scale(strip, rows, row_h);
    Some(row * row_h - view.height / 2.0)
}

/// Drag the thumb, or click to put it there; `visible` is the share of the width in view.
pub(super) fn hscrollbar(
    ui: &mut Ui,
    id: egui::Id,
    track: Rect,
    scroll_x: &mut f32,
    visible: f32,
    max: f32,
    c: &Colors,
) {
    ui.painter()
        .rect_filled(track, 0.0, ui.visuals().panel_fill);
    let thumb_w = (track.width() * visible).clamp(30.0, track.width());
    let room = (track.width() - thumb_w).max(1.0);
    let x = track.left() + room * (*scroll_x / max);
    let thumb = Rect::from_min_size(
        pos2(x, track.top() + 2.0),
        vec2(thumb_w, SCROLLBAR_HEIGHT - 4.0),
    );
    let response = ui.interact(track, id, crate::focus::pointer_only());
    if response.dragged() {
        *scroll_x = (*scroll_x + response.drag_delta().x * max / room).clamp(0.0, max);
    } else if response.clicked()
        && let Some(p) = response.interact_pointer_pos()
    {
        *scroll_x = ((p.x - track.left() - thumb_w / 2.0) / room * max).clamp(0.0, max);
    }
    let fill = if response.hovered() || response.dragged() {
        c.thumb_hover
    } else {
        c.thumb
    };
    ui.painter().rect_filled(thumb, CornerRadius::same(3), fill);
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
    pub(super) selection: Color32,
    pub(super) found: Color32,
    pub(super) found_current: Color32,
    pub(super) found_mark: Color32,
    pub(super) thumb: Color32,
    pub(super) thumb_hover: Color32,
}

pub(super) fn colors(ui: &Ui) -> Colors {
    let table = table_style::colors(ui);
    let weak = ui.visuals().weak_text_color();
    let text = ui.visuals().text_color();
    if ui.visuals().dark_mode {
        Colors {
            pane: table.pane,
            line: table.line,
            weak,
            text,
            filler: Color32::from_gray(30),
            fold: Color32::from_gray(28),
            fold_hover: Color32::from_gray(36),
            removed_line: Color32::from_rgb(0x3d, 0x1c, 0x20),
            removed_word: Color32::from_rgb(0x80, 0x2c, 0x35),
            added_line: Color32::from_rgb(0x17, 0x33, 0x21),
            added_word: Color32::from_rgb(0x26, 0x62, 0x37),
            removed: table.removed,
            added: table.added,
            renamed: table.renamed,
            note: Color32::from_rgb(0xe8, 0xc0, 0x6a),
            selected_bg: table.selected_bg,
            selected_fg: table.selected_fg,
            selection: Color32::from_rgba_unmultiplied(0x35, 0x84, 0xe4, 110),
            found: table.found,
            found_current: Color32::from_rgba_unmultiplied(0xc2, 0x6a, 0x00, 200),
            found_mark: Color32::from_rgb(0xf0, 0xa0, 0x30),
            thumb: Color32::from_white_alpha(50),
            thumb_hover: Color32::from_white_alpha(90),
        }
    } else {
        Colors {
            pane: table.pane,
            line: table.line,
            weak,
            text,
            filler: Color32::from_gray(243),
            fold: Color32::from_rgb(0xf3, 0xf6, 0xfa),
            fold_hover: Color32::from_rgb(0xe6, 0xee, 0xf8),
            removed_line: Color32::from_rgb(0xff, 0xeb, 0xe9),
            removed_word: Color32::from_rgb(0xff, 0xc0, 0xc0),
            added_line: Color32::from_rgb(0xe6, 0xff, 0xec),
            added_word: Color32::from_rgb(0xab, 0xf2, 0xbc),
            removed: table.removed,
            added: table.added,
            renamed: table.renamed,
            note: Color32::from_rgb(0x9a, 0x5b, 0x00),
            selected_bg: table.selected_bg,
            selected_fg: table.selected_fg,
            selection: Color32::from_rgba_unmultiplied(0x35, 0x84, 0xe4, 80),
            found: table.found,
            found_current: Color32::from_rgba_unmultiplied(0xff, 0x8c, 0x00, 190),
            found_mark: Color32::from_rgb(0xe0, 0x82, 0x00),
            thumb: Color32::from_black_alpha(45),
            thumb_hover: Color32::from_black_alpha(90),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worker_gone_without_an_answer_fails_the_load() {
        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        let load = Load::Loading(rx);
        assert_eq!(load.received(), None);
        drop(tx);
        assert_eq!(
            load.received(),
            Some(Err("loading stopped unexpectedly".into()))
        );
    }
}
