use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{
    self, Color32, CornerRadius, Galley, Id, InputState, Key, Modifiers, Painter, Rect, Response,
    Sense, Shape, Stroke, StrokeKind, Ui, Vec2, vec2,
};
use parterre_core::glyphs::{self, Glyph, Part};
use parterre_core::list_keys::{Move, target};
use parterre_core::worktree_folder;

use crate::focus::{self, Arrows};

const ICON_SIZE: f32 = 18.0;
pub const BUTTON_SIZE: f32 = 30.0;
pub const MINI_BUTTON_SIZE: f32 = 22.0;
const BUTTON_CORNER: u8 = 7;
const FOCUS_LINE_WIDTH: f32 = 1.5;

/// Colours beyond egui's visuals.
pub struct Tones {
    /// Background and foreground of a toggle that is on.
    pub on_bg: Color32,
    pub on_fg: Color32,
    pub hover: Color32,
    pub press: Color32,
    /// Background of a segmented control, and of its selected segment.
    pub seg_bg: Color32,
    pub seg_on: Color32,
    pub field: Color32,
    pub field_line: Color32,
    pub accent: Color32,
    /// Background of the rows in the settings window.
    pub group: Color32,
}

pub fn tones(ui: &Ui) -> Tones {
    tones_of(ui.visuals().dark_mode)
}

pub fn tones_of(dark: bool) -> Tones {
    if dark {
        Tones {
            on_bg: Color32::from_rgb(0x21, 0x3a, 0x57),
            on_fg: Color32::from_rgb(0x8e, 0xc2, 0xff),
            hover: Color32::from_white_alpha(20),
            press: Color32::from_white_alpha(33),
            seg_bg: Color32::from_white_alpha(18),
            seg_on: Color32::from_gray(74),
            field: Color32::from_gray(42),
            field_line: Color32::from_white_alpha(31),
            accent: Color32::from_rgb(0x35, 0x84, 0xe4),
            group: Color32::from_gray(30),
        }
    } else {
        Tones {
            on_bg: Color32::from_rgb(0xd7, 0xe8, 0xfd),
            on_fg: Color32::from_rgb(0x1a, 0x5f, 0xb4),
            hover: Color32::from_black_alpha(14),
            press: Color32::from_black_alpha(26),
            seg_bg: Color32::from_black_alpha(15),
            seg_on: Color32::WHITE,
            field: Color32::WHITE,
            field_line: Color32::from_black_alpha(36),
            accent: Color32::from_rgb(0x35, 0x84, 0xe4),
            group: Color32::WHITE,
        }
    }
}

/// Paints `glyph` scaled into `rect`.
pub fn paint_glyph(painter: &Painter, rect: Rect, glyph: Glyph, color: Color32) {
    const CURVE_TOLERANCE: f32 = 0.2;
    let scale = rect.width() / glyphs::SIZE;
    let at = |p: [f32; 2]| rect.min + vec2(p[0], p[1]) * scale;
    let stroke = Stroke::new(glyphs::STROKE * scale, color);
    for part in glyph {
        match *part {
            Part::Path(d) => {
                for line in glyphs::flatten(d, CURVE_TOLERANCE / scale).unwrap_or_default() {
                    let points: Vec<_> = line.points.into_iter().map(at).collect();
                    painter.add(if line.closed {
                        Shape::closed_line(points, stroke)
                    } else {
                        Shape::line(points, stroke)
                    });
                }
            }
            Part::Circle {
                center,
                radius,
                filled,
            } => {
                if filled {
                    painter.circle_filled(at(center), radius * scale, color);
                } else {
                    painter.circle_stroke(at(center), radius * scale, stroke);
                }
            }
            Part::Rect {
                min,
                size,
                radius,
                filled,
            } => {
                let r = Rect::from_min_size(at(min), vec2(size[0], size[1]) * scale);
                let corner = CornerRadius::same((radius * scale).round() as u8);
                if filled {
                    painter.rect_filled(r, corner, color);
                } else {
                    painter.rect_stroke(r, corner, stroke, StrokeKind::Middle);
                }
            }
        }
    }
}

/// The background of a button: `on` tinted, `open` (its popover is showing) or pressed darker,
/// hovered lighter.
fn paint_background(ui: &Ui, rect: Rect, response: &Response, on: bool, open: bool) {
    let t = tones(ui);
    let fill = if on {
        t.on_bg
    } else if open || response.is_pointer_button_down_on() {
        t.press
    } else if response.hovered() {
        t.hover
    } else {
        return;
    };
    ui.painter().rect_filled(rect, BUTTON_CORNER, fill);
}

fn foreground(ui: &Ui, on: bool) -> Color32 {
    if !ui.is_enabled() {
        ui.visuals().weak_text_color()
    } else if on {
        tones(ui).on_fg
    } else {
        ui.visuals().text_color()
    }
}

/// A bookmark ribbon filling `rect`, notched at the bottom: the mark for comparison.
pub fn paint_ribbon(painter: &Painter, rect: Rect, fill: Color32, outline: Stroke) {
    let (left, top, right, bottom) = (rect.left(), rect.top(), rect.right(), rect.bottom());
    let mid = rect.center().x;
    let notch = bottom - rect.height() / 4.0;
    // egui fills convex shapes only, so the notched ribbon is two halves.
    for half in [
        [(left, top), (mid, top), (mid, notch), (left, bottom)],
        [(mid, top), (right, top), (right, bottom), (mid, notch)],
    ] {
        let points = half.map(|(x, y)| egui::pos2(x, y)).to_vec();
        painter.add(egui::Shape::convex_polygon(points, fill, Stroke::NONE));
    }
    let outline_points = [
        (left, top),
        (right, top),
        (right, bottom),
        (mid, notch),
        (left, bottom),
        (left, top),
    ]
    .map(|(x, y)| egui::pos2(x, y))
    .to_vec();
    painter.add(egui::Shape::line(outline_points, outline));
}

/// Tinted while `on`.
pub fn icon_button(ui: &mut Ui, glyph: Glyph, on: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON_SIZE), Sense::click());
    paint_background(ui, rect, &response, on, false);
    let icon = Rect::from_center_size(rect.center(), Vec2::splat(ICON_SIZE));
    paint_glyph(ui.painter(), icon, glyph, foreground(ui, on));
    focus::ring(ui, &response, BUTTON_CORNER);
    response
}

/// An icon button in `color`. With `progress` (0 to 1), a small arrow inside a ring that fills
/// clockwise from the top as a download does, as t3code shows its own (#433).
pub fn progress_button(
    ui: &mut Ui,
    glyph: Glyph,
    color: Color32,
    progress: Option<f32>,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON_SIZE), Sense::click());
    paint_background(ui, rect, &response, false, false);
    match progress {
        Some(progress) => {
            let radius = ICON_SIZE / 2.0;
            let width = 2.0;
            let center = rect.center();
            let painter = ui.painter();
            painter.circle_stroke(center, radius, Stroke::new(width, tones(ui).field_line));
            let steps = 48;
            let filled = (progress.clamp(0.0, 1.0) * steps as f32).round() as usize;
            if filled > 0 {
                let points = (0..=filled)
                    .map(|i| {
                        let angle = std::f32::consts::TAU * i as f32 / steps as f32
                            - std::f32::consts::FRAC_PI_2;
                        center + vec2(angle.cos(), angle.sin()) * radius
                    })
                    .collect();
                painter.add(Shape::line(points, Stroke::new(width, color)));
            }
            let arrow = Rect::from_center_size(center, Vec2::splat(ICON_SIZE * 0.6));
            paint_glyph(painter, arrow, glyphs::ARROW_DOWN, color);
        }
        None => {
            let icon = Rect::from_center_size(rect.center(), Vec2::splat(ICON_SIZE));
            paint_glyph(ui.painter(), icon, glyph, color);
        }
    }
    focus::ring(ui, &response, BUTTON_CORNER);
    response
}

/// The diff and blame windows' toggle of syntax colouring.
pub fn syntax_button(ui: &mut Ui, on: &mut bool) {
    let (title, body) = if *on {
        (
            "Syntax colour",
            "Code is coloured by its language. Click for plain text.",
        )
    } else {
        ("Plain text", "Click to colour code by its language.")
    };
    let response = icon_button(ui, glyphs::PALETTE, *on);
    if tip_explained(response, title, "", body).clicked() {
        *on = !*on;
    }
}

/// A small icon button, e.g. inside the find field.
pub fn mini_button(ui: &mut Ui, glyph: Glyph) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(MINI_BUTTON_SIZE), Sense::click());
    paint_background(ui, rect, &response, false, false);
    let icon = Rect::from_center_size(rect.center(), Vec2::splat(14.0));
    paint_glyph(ui.painter(), icon, glyph, foreground(ui, false));
    focus::ring(ui, &response, BUTTON_CORNER);
    response
}

pub const COPIED_GREEN: Color32 = Color32::from_rgb(0x2e, 0xa0, 0x43);
const COPIED_SHOWN_SECONDS: f64 = 1.5;

/// Copies `text()` when the button `button` draws is clicked. `button` is told whether that
/// happened a moment ago, to say so.
pub fn copy_on_click(
    ui: &mut Ui,
    id: Id,
    text: impl FnOnce() -> String,
    button: impl FnOnce(&mut Ui, bool) -> Response,
) {
    let now = ui.input(|i| i.time);
    let copied_at: Option<f64> = ui.data(|d| d.get_temp(id));
    let copied = copied_at.is_some_and(|at| now - at < COPIED_SHOWN_SECONDS);
    if copied {
        ui.ctx().request_repaint_after(Duration::from_millis(300));
    }
    if button(ui, copied).clicked() {
        ui.ctx().copy_text(text());
        ui.data_mut(|d| d.insert_temp(id, now));
    }
}

/// Copies `text()` when clicked, with a check mark in `done` for a moment after.
pub fn copy_button(ui: &mut Ui, id: Id, text: impl FnOnce() -> String, hover: &str, done: Color32) {
    copy_on_click(ui, id, text, |ui, copied| {
        copy_icon_button(ui, copied, done).on_hover_text(hover)
    });
}

/// Shows a check mark in `done` instead of its icon while `copied`.
pub fn copy_icon_button(ui: &mut Ui, copied: bool, done: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
    paint_background(ui, rect, &response, false, false);
    let icon = Rect::from_center_size(rect.center(), Vec2::splat(14.0));
    if copied {
        paint_glyph(ui.painter(), icon, glyphs::CHECK, done);
    } else {
        let color = ui.visuals().weak_text_color();
        paint_glyph(ui.painter(), icon, glyphs::COPY, color);
    }
    focus::ring(ui, &response, BUTTON_CORNER);
    response
}

/// In [`Color32::PLACEHOLDER`], for the painter to colour: measured and painted, a button's
/// text is laid out once.
fn button_galley(ui: &Ui, text: &str) -> Arc<Galley> {
    let font = egui::TextStyle::Button.resolve(ui.style());
    ui.painter()
        .layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER)
}

const TEXT_BUTTON_HEIGHT: f32 = 28.0;
const TEXT_BUTTON_PADDING: f32 = 12.0;

/// A button for text on a light fill, `width` wide.
fn text_button_frame(ui: &mut Ui, width: f32) -> (Rect, Response) {
    let size = vec2(width + 2.0 * TEXT_BUTTON_PADDING, TEXT_BUTTON_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let t = tones(ui);
    let fill = if response.is_pointer_button_down_on() || response.hovered() {
        t.press
    } else {
        t.seg_bg
    };
    ui.painter().rect_filled(rect, BUTTON_CORNER, fill);
    (rect, response)
}

/// A text button on a light fill, e.g. "Reset" in the zoom popover.
pub fn text_button(ui: &mut Ui, text: &str) -> Response {
    let color = foreground(ui, false);
    let galley = button_galley(ui, text);
    let (rect, response) = text_button_frame(ui, galley.size().x);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    focus::ring(ui, &response, BUTTON_CORNER);
    response
}

pub fn text_button_width(ui: &Ui, text: &str) -> f32 {
    button_galley(ui, text).size().x + 2.0 * TEXT_BUTTON_PADDING
}

/// A [`text_button`] with `glyph` in `color` before the text.
pub fn icon_text_button(ui: &mut Ui, glyph: Glyph, color: Color32, text: &str) -> Response {
    const GLYPH_SIZE: f32 = 16.0;
    const GLYPH_GAP: f32 = 6.0;
    let text_color = foreground(ui, false);
    let color = if ui.is_enabled() { color } else { text_color };
    let galley = button_galley(ui, text);
    let (rect, response) = text_button_frame(ui, GLYPH_SIZE + GLYPH_GAP + galley.size().x);
    let left = rect.left() + TEXT_BUTTON_PADDING;
    let icon = Rect::from_min_size(
        egui::pos2(left, rect.center().y - GLYPH_SIZE / 2.0),
        Vec2::splat(GLYPH_SIZE),
    );
    paint_glyph(ui.painter(), icon, glyph, color);
    let at = egui::pos2(
        left + GLYPH_SIZE + GLYPH_GAP,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(at, galley, text_color);
    focus::ring(ui, &response, BUTTON_CORNER);
    response
}

/// White text on the accent colour, at least `min_width` wide.
pub fn primary_button(ui: &mut Ui, text: &str, min_width: f32) -> Response {
    let accent = tones(ui).accent;
    filled_button(ui, text, min_width, accent)
}

/// White text on the error colour, for what loses work.
pub fn danger_button(ui: &mut Ui, text: &str, min_width: f32) -> Response {
    let error = ui.visuals().error_fg_color;
    filled_button(ui, text, min_width, error)
}

const FILLED_BUTTON_HEIGHT: f32 = 30.0;
const FILLED_BUTTON_PADDING: f32 = 16.0;
const FILLED_BUTTON_CORNER: u8 = 8;

/// How wide a [`primary_button`] or [`danger_button`] showing `text` is.
pub fn filled_button_width(ui: &Ui, text: &str, min_width: f32) -> f32 {
    let galley = button_galley(ui, text);
    filled_width(&galley, min_width)
}

fn filled_width(galley: &Galley, min_width: f32) -> f32 {
    (galley.size().x + 2.0 * FILLED_BUTTON_PADDING).max(min_width)
}

fn filled_button(ui: &mut Ui, text: &str, min_width: f32, color: Color32) -> Response {
    let galley = button_galley(ui, text);
    let size = vec2(filled_width(&galley, min_width), FILLED_BUTTON_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let fill = if response.is_pointer_button_down_on() {
        color.gamma_multiply(0.8)
    } else if response.hovered() {
        color.gamma_multiply(0.9)
    } else {
        color
    };
    ui.painter()
        .rect_filled(rect, FILLED_BUTTON_CORNER, fill.to_opaque());
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    focus::ring(ui, &response, FILLED_BUTTON_CORNER);
    response
}

/// A button that opens a popover (with [`egui::Popup::from_toggle_button_response`]), shown
/// pressed while it is open. `glyph` of `None` makes it a narrow chevron.
pub fn popover_button(ui: &mut Ui, id: Id, glyph: Option<Glyph>, on: bool) -> Response {
    let size = if glyph.is_some() {
        Vec2::splat(BUTTON_SIZE)
    } else {
        vec2(20.0, BUTTON_SIZE)
    };
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let response = ui.interact(rect, id, Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    paint_background(ui, rect, &response, on, open);
    focus::ring(ui, &response, BUTTON_CORNER);
    let color = foreground(ui, on);
    match glyph {
        Some(glyph) => {
            let icon = Rect::from_center_size(rect.center(), Vec2::splat(ICON_SIZE));
            paint_glyph(ui.painter(), icon, glyph, color);
        }
        None => {
            let icon = Rect::from_center_size(rect.center(), Vec2::splat(13.0));
            paint_glyph(
                ui.painter(),
                icon,
                glyphs::CHEVRON_DOWN,
                color.gamma_multiply(0.8),
            );
        }
    }
    response
}

/// Icon segments of which one is selected, e.g. the drag modes. `tip` adds each segment's
/// tooltip. Returns the value of the segment clicked, if any.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    current: T,
    items: &[(T, Glyph)],
    tip: impl Fn(T, Response) -> Response,
) -> Option<T> {
    let widths = std::iter::repeat_n(BUTTON_SIZE, items.len());
    let selected = items.iter().position(|(v, _)| *v == current);
    let chosen = segments(ui, widths, selected, |ui, i, rect, response| {
        let (value, glyph) = items[i];
        paint_segment(ui, rect, &response, value == current);
        let icon = Rect::from_center_size(rect.center(), Vec2::splat(ICON_SIZE));
        paint_glyph(ui.painter(), icon, glyph, foreground(ui, false));
        tip(value, response)
    });
    chosen.map(|i| items[i].0)
}

const TAB_HEADER_HEIGHT: f32 = 34.0;

/// Tabs over the page they show, in one rounded card: the tabs in its header, the page that
/// `page` lays out under them. The page holds the controls of one tab, while those of the whole
/// window go outside the card (Windows' tab guidelines). Ctrl+Tab and Ctrl+Shift+Tab go to the
/// next and the previous tab.
///
/// The tabs are one stop for Tab, `id`, shown by a dashed line under the selected tab's text:
/// Left and Right then go to the tab before and after, and Tab on into the page.
pub fn tabs<T: PartialEq + Copy, R>(
    ui: &mut Ui,
    id: Id,
    current: &mut T,
    items: &[(T, &str)],
    page: impl FnOnce(&mut Ui) -> R,
) -> R {
    const CARD_CORNER: u8 = 9;
    const PAGE_MARGIN: i8 = 10;
    let selected = items.iter().position(|(v, _)| v == current).unwrap_or(0);
    if let Some(i) = ctrl_tab(ui, selected, items.len()) {
        *current = items[i].0;
    }
    let t = tones(ui);
    let line = Stroke::new(1.0, t.field_line);
    egui::Frame::new()
        .fill(t.group)
        .stroke(line)
        .corner_radius(CARD_CORNER)
        .show(ui, |ui| {
            let outer_spacing = ui.spacing().item_spacing;
            ui.spacing_mut().item_spacing.y = 0.0;
            tab_header(ui, id, current, items, line);
            egui::Frame::new()
                .inner_margin(PAGE_MARGIN)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = outer_spacing;
                    page(ui)
                })
                .inner
        })
        .inner
}

/// The tab Ctrl+Tab or Ctrl+Shift+Tab goes to from `selected` of `len`, round past the ends.
fn ctrl_tab(ui: &Ui, selected: usize, len: usize) -> Option<usize> {
    ui.input_mut(|i| {
        // With Shift first: a shortcut without it takes it too.
        if i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab) {
            Some((selected + len - 1) % len)
        } else if i.consume_key(Modifiers::CTRL, Key::Tab) {
            Some((selected + 1) % len)
        } else {
            None
        }
    })
}

/// The selected tab is in strong text, underlined in the accent colour on the header's `line`;
/// the others in weak text.
fn tab_header<T: PartialEq + Copy>(
    ui: &mut Ui,
    id: Id,
    current: &mut T,
    items: &[(T, &str)],
    line: Stroke,
) {
    const HEADER_CORNER: u8 = 8;
    const FIRST_TAB_INSET: f32 = 6.0;
    const TAB_PADDING: f32 = 24.0;
    let (header, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), TAB_HEADER_HEIGHT),
        Sense::hover(),
    );
    let control = ui.interact(header, id, Sense::focusable_noninteractive());
    let selected = items.iter().position(|(v, _)| v == current);
    if let Some(i) = left_right_step(ui, &control, selected, items.len()) {
        *current = items[i].0;
    }
    let cue = focus::shows_own(&control);
    let t = tones(ui);
    let top_corners = CornerRadius {
        nw: HEADER_CORNER,
        ne: HEADER_CORNER,
        sw: 0,
        se: 0,
    };
    ui.painter().rect_filled(header, top_corners, t.seg_bg);
    ui.painter().hline(header.x_range(), header.bottom(), line);
    let font = egui::TextStyle::Button.resolve(ui.style());
    let strong = ui.visuals().strong_text_color();
    let weak = ui.visuals().weak_text_color();
    let mut x = header.left() + FIRST_TAB_INSET;
    for (i, (value, label)) in items.iter().enumerate() {
        let on = value == current;
        let galley =
            ui.painter()
                .layout_no_wrap((*label).to_owned(), font.clone(), Color32::PLACEHOLDER);
        let rect = Rect::from_min_max(
            egui::pos2(x, header.top()),
            egui::pos2(
                x + galley.size().x + TAB_PADDING,
                header.top() + TAB_HEADER_HEIGHT,
            ),
        );
        let response = ui.interact(rect, id.with(i), Sense::CLICK);
        x = rect.right();
        let color = if on || response.hovered() {
            strong
        } else {
            weak
        };
        let text = Rect::from_center_size(rect.center(), galley.size());
        ui.painter().galley(text.min, galley, color);
        if on {
            paint_tab_underline(ui.painter(), rect, t.accent);
            if cue {
                paint_tab_cue(ui.painter(), text, color);
            }
        }
        focus::take_on_part_click(&response, id);
        if response.clicked() && !on {
            *current = *value;
        }
        response.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
}

/// On the header's line at the bottom of `tab`.
fn paint_tab_underline(painter: &Painter, tab: Rect, color: Color32) {
    const INSET: f32 = 8.0;
    const CORNER: u8 = 2;
    let underline = Rect::from_min_max(
        egui::pos2(tab.left() + INSET, tab.bottom() - 2.5),
        egui::pos2(tab.right() - INSET, tab.bottom() + 0.5),
    );
    painter.rect_filled(underline, CORNER, color);
}

fn paint_tab_cue(painter: &Painter, text: Rect, color: Color32) {
    const GAP_BELOW_TEXT: f32 = 1.5;
    const DASH: f32 = 2.0;
    let y = text.bottom() + GAP_BELOW_TEXT;
    painter.extend(Shape::dashed_line(
        &[egui::pos2(text.left(), y), egui::pos2(text.right(), y)],
        Stroke::new(1.0, color),
        DASH,
        DASH,
    ));
}

/// A row against the right edge of what is left of `ui`'s row, which `add` lays out left to
/// right, so that Tab visits it in the order it reads (egui visits a right-to-left row
/// backwards). It is placed by its width in the frame before ([`trailing_width`]); a frame in
/// which that changed is laid out again before it is shown. `name` tells a window's rows apart.
pub fn trailing<R>(ui: &mut Ui, name: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    const WIDTH_TOLERANCE: f32 = 0.5;
    let id = trailing_id(ui, name);
    let width = trailing_width(ui, name);
    let room = ui.available_rect_before_wrap();
    let room = room.with_min_x((room.right() - width).max(room.left()));
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(room)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let inner = add(&mut row);
    let laid_out = row.min_rect();
    if (laid_out.width() - width).abs() > WIDTH_TOLERANCE {
        ui.data_mut(|d| d.insert_temp(id, laid_out.width()));
        ui.ctx().request_discard("a trailing row changed width");
    }
    ui.advance_cursor_after_rect(laid_out);
    inner
}

/// As wide as [`trailing`] row `name` was in the frame before: what goes before it has the rest.
pub fn trailing_width(ui: &Ui, name: &str) -> f32 {
    let id = trailing_id(ui, name);
    ui.data(|d| d.get_temp(id)).unwrap_or(0.0)
}

fn trailing_id(ui: &Ui, name: &str) -> Id {
    Id::new(("trailing", name)).with(ui.ctx().viewport_id())
}

fn text_width(ui: &Ui, text: &str, font: &egui::FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER)
        .size()
        .x
}

/// Text segments of which one is selected, e.g. the theme in the settings.
pub fn text_segmented<T: PartialEq + Copy>(ui: &mut Ui, current: &mut T, items: &[(T, &str)]) {
    let color = ui.visuals().text_color();
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galleys: Vec<_> = items
        .iter()
        .map(|&(_, text)| {
            ui.painter()
                .layout_no_wrap(text.to_owned(), font.clone(), color)
        })
        .collect();
    let widths = galleys.iter().map(|g| g.size().x + 18.0);
    let selected = *current;
    let at = items.iter().position(|(v, _)| *v == selected);
    let clicked = segments(ui, widths, at, |ui, i, rect, response| {
        paint_segment(ui, rect, &response, items[i].0 == selected);
        let galley = galleys[i].clone();
        ui.painter()
            .galley(rect.center() - galley.size() / 2.0, galley, color);
        response
    });
    if let Some(i) = clicked {
        *current = items[i].0;
    }
}

/// Segments of the given widths on a shared background, laid out by hand so that they read
/// left to right in any layout. `segment` paints one; the one clicked is returned. The control
/// is one stop for Tab: with the keyboard, Left and Right choose the segment before and after
/// the `selected` one.
fn segments(
    ui: &mut Ui,
    widths: impl ExactSizeIterator<Item = f32> + Clone,
    selected: Option<usize>,
    mut segment: impl FnMut(&mut Ui, usize, Rect, Response) -> Response,
) -> Option<usize> {
    const INSET: f32 = 2.0;
    const CORNER: u8 = 9;
    let count = widths.len();
    let total = widths.clone().sum::<f32>() + INSET * (count + 1) as f32;
    let (group, response) =
        ui.allocate_exact_size(vec2(total, BUTTON_SIZE), Sense::focusable_noninteractive());
    let mut chosen = left_right_step(ui, &response, selected, count);
    ui.painter().rect_filled(group, CORNER, tones(ui).seg_bg);
    let mut x = group.left() + INSET;
    for (i, w) in widths.enumerate() {
        let rect = Rect::from_min_size(
            egui::pos2(x, group.top() + INSET),
            vec2(w, BUTTON_SIZE - 2.0 * INSET),
        );
        let part = ui.interact(rect, response.id.with(i), Sense::CLICK);
        let part = segment(ui, i, rect, part);
        focus::take_on_part_click(&part, response.id);
        if part.clicked() {
            chosen = Some(i);
        }
        x += w + INSET;
    }
    focus::ring(ui, &response, CORNER);
    chosen
}

/// Where Left or Right goes from `selected` of `len` while `control` has the keyboard, which
/// keeps those keys: no further than the ends.
fn left_right_step(
    ui: &Ui,
    control: &Response,
    selected: Option<usize>,
    len: usize,
) -> Option<usize> {
    focus::claim_arrows(control, Arrows::LeftRight);
    if !control.has_focus() || len == 0 {
        return None;
    }
    let moves = [
        (Key::ArrowLeft, Move::Back),
        (Key::ArrowRight, Move::Forward),
    ];
    let step = ui.input_mut(|i| consume_move(i, &moves));
    step.and_then(|step| target(selected, len, step))
}

/// The first of `moves` whose key, unmodified, was pressed.
fn consume_move(input: &mut InputState, moves: &[(Key, Move)]) -> Option<Move> {
    moves
        .iter()
        .find(|(key, _)| input.consume_key(Modifiers::NONE, *key))
        .map(|&(_, step)| step)
}

/// The selected segment is raised: lighter, with a thin shadow.
fn paint_segment(ui: &Ui, rect: Rect, response: &Response, selected: bool) {
    let t = tones(ui);
    let painter = ui.painter();
    let corner = CornerRadius::same(7);
    if selected {
        let shadow = if ui.visuals().dark_mode { 120 } else { 45 };
        painter.rect_filled(
            rect.translate(vec2(0.0, 1.0)),
            corner,
            Color32::from_black_alpha(shadow),
        );
        painter.rect_filled(rect, corner, t.seg_on);
    } else if response.hovered() {
        painter.rect_filled(rect, corner, t.hover);
    }
}

/// What the keyboard did to a list that is one stop for Tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListKeys {
    pub select: Option<usize>,
    /// Enter: as a double click on the selected row.
    pub enter: bool,
    /// The context-menu key: the selected row's menu.
    pub menu: bool,
}

/// While list `id` has the keyboard: Up and Down, Page Up and Page Down (by `page` rows), Home
/// and End move the selection from the `selected` row of `len`. Read before the rows are laid
/// out, so that they show where it went.
pub fn list_keys(ui: &Ui, id: Id, selected: Option<usize>, len: usize, page: usize) -> ListKeys {
    if len == 0 || !ui.memory(|m| m.has_focus(id)) {
        return ListKeys::default();
    }
    let moves = [
        (Key::Home, Move::First),
        (Key::End, Move::Last),
        (Key::ArrowDown, Move::Forward),
        (Key::ArrowUp, Move::Back),
        (Key::PageDown, Move::PageForward(page)),
        (Key::PageUp, Move::PageBack(page)),
    ];
    ui.input_mut(|i| {
        let step = consume_move(i, &moves);
        ListKeys {
            select: step.and_then(|step| target(selected, len, step)),
            enter: i.consume_key(Modifiers::NONE, Key::Enter),
            menu: crate::keys::CONTEXT_MENU.consume(i),
        }
    })
}

/// List `id`'s stop for Tab, over the `rect` its rows are then laid out in, which sense clicks
/// ([`Sense::CLICK`]) but aren't stops themselves. A click on a row gives the list the keyboard
/// ([`focus::take_on_row_click`]), so the stop comes first: made after the click, egui would
/// take the keyboard from it again for a click elsewhere.
pub fn list_stop(ui: &mut Ui, id: Id, rect: Rect) -> Response {
    let response = ui.interact(rect, id, Sense::focusable_noninteractive());
    focus::claim_arrows(&response, Arrows::UpDown);
    response
}

const LIST_CUE_CORNER: u8 = 5;

/// Once the rows of list `stop` are laid out: unless a row in view showed that the list has the
/// keyboard ([`paint_list_cue`]), a ring around the list does.
pub fn list_stop_ring(ui: &Ui, stop: &Response, cued: bool) {
    if !cued {
        focus::ring(ui, stop, LIST_CUE_CORNER);
    }
}

/// A line inside `row` that shows its list has the keyboard ([`focus::shows_own_by_id`]).
pub fn paint_list_cue(ui: &Ui, row: Rect) {
    ui.painter().rect_stroke(
        row.shrink(1.0),
        LIST_CUE_CORNER,
        Stroke::new(FOCUS_LINE_WIDTH, tones(ui).accent),
        StrokeKind::Inside,
    );
}

pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    const CORNER: u8 = 10;
    let (rect, mut response) = ui.allocate_exact_size(vec2(36.0, 20.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = tones(ui);
    let how_on = ui.ctx().animate_bool_responsive(response.id, *on);
    let fill = t.seg_bg.lerp_to_gamma(t.accent, how_on);
    let fill = if ui.is_enabled() {
        fill
    } else {
        fill.gamma_multiply(0.5)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CORNER, fill);
    if how_on < 1.0 {
        painter.rect_stroke(
            rect,
            CORNER,
            Stroke::new(1.0, t.field_line.gamma_multiply(1.0 - how_on)),
            StrokeKind::Inside,
        );
    }
    let x = egui::lerp(rect.left() + 10.0..=rect.right() - 10.0, how_on);
    let knob = egui::pos2(x, rect.center().y);
    painter.circle_filled(knob + vec2(0.0, 0.5), 8.0, Color32::from_black_alpha(60));
    painter.circle_filled(knob, 8.0, Color32::WHITE);
    focus::ring(ui, &response, CORNER);
    response
}

/// A single-line text field in a rounded frame that shows on any background.
pub fn text_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    const CORNER: u8 = 7;
    let t = tones(ui);
    let shown = egui::Frame::new()
        .fill(t.field)
        .stroke(Stroke::new(1.0, t.field_line))
        .corner_radius(CORNER)
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .frame(egui::Frame::NONE)
                    .hint_text(hint)
                    .desired_width(width - 16.0),
            )
        });
    if shown.inner.has_focus() {
        ui.painter().rect_stroke(
            shown.response.rect,
            CORNER,
            Stroke::new(FOCUS_LINE_WIDTH, t.accent),
            StrokeKind::Inside,
        );
    }
    shown.inner
}

/// A folder's [`text_field`] with a *Browse…* [`text_button`] after it, as wide as the row, the
/// button greyed while `browsing`. The field's response, and whether *Browse…* was clicked.
pub fn folder_field(
    ui: &mut Ui,
    text: &mut String,
    hint: &str,
    browsing: bool,
) -> (Response, bool) {
    const BROWSE: &str = "Browse…";
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let button = text_button_width(ui, BROWSE);
        // The field's frame line adds a point on each side.
        let field = text_field(ui, text, hint, ui.available_width() - button - 4.0 - 2.0);
        let browse = ui
            .add_enabled_ui(!browsing, |ui| text_button(ui, BROWSE))
            .inner
            .clicked();
        (field, browse && !browsing)
    })
    .inner
}

/// A find field, as in the main toolbar and the blame windows' toolbars.
pub struct Find<'a> {
    pub id: Id,
    pub width: f32,
    pub hint: &'a str,
    /// While there is a query: how many places it was found at, e.g. "3 of 12".
    pub count: &'a str,
    /// The shortcuts of the previous, next and clear buttons, for their tooltips.
    pub keys: &'a [String; 3],
    /// Take the focus in this frame; `select` also selects the query, so typing replaces it.
    pub focus: bool,
    pub select: bool,
}

/// What was done in a find field this frame.
#[derive(Default)]
pub struct Found {
    pub changed: bool,
    /// By Enter or the button; `previous` by Shift+Enter or the button.
    pub next: bool,
    pub previous: bool,
    pub cleared: bool,
}

/// A folder name field, which drops what can't be in a folder's name (separators, and what
/// Windows forbids) as it's typed, and a button to have the name follow what it follows again.
/// Returns whether that was clicked.
pub fn folder_name_field(
    ui: &mut Ui,
    name: &mut String,
    edited: &mut bool,
    follow_tip: &str,
) -> bool {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let width = ui.available_width() - BUTTON_SIZE - 4.0;
        let mut typed = name.clone();
        if text_field(ui, &mut typed, "Folder name", width).changed() {
            *name = typed
                .chars()
                .filter(|c| !worktree_folder::forbidden(*c))
                .collect();
            *edited = true;
        }
        let follow = ui
            .add_enabled_ui(*edited, |ui| icon_button(ui, glyphs::RESET, false))
            .inner
            .on_hover_text(follow_tip)
            .clicked();
        if follow {
            *edited = false;
        }
        follow
    })
    .inner
}

/// A find field for `query`: the query (a hint and the find shortcut while empty, if there is
/// room), and while there is one, the count and buttons for the previous and next place and for
/// clearing. Enter keeps the focus.
pub fn find_field(ui: &mut Ui, find: &Find, query: &mut String) -> Found {
    let focused = ui.memory(|m| m.has_focus(find.id));
    let t = tones(ui);
    let stroke = if focused {
        Stroke::new(FOCUS_LINE_WIDTH, t.accent)
    } else {
        Stroke::new(1.0, t.field_line)
    };
    let mut found = Found::default();
    egui::Frame::new()
        .fill(t.field)
        .stroke(stroke)
        .corner_radius(8)
        .inner_margin(egui::Margin {
            left: 8,
            right: 4,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_width(find.width - 12.0);
            ui.set_height(28.0);
            ui.spacing_mut().item_spacing.x = 4.0;
            let weak = ui.visuals().weak_text_color();
            let (icon, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
            paint_glyph(ui.painter(), icon, glyphs::SEARCH, weak);
            if focused {
                // A line copied whole ends with a line break, which the field would keep as a
                // blank that no line ends with.
                ui.input_mut(|i| {
                    for event in &mut i.events {
                        if let egui::Event::Paste(text) = event {
                            text.truncate(text.trim_end_matches(['\r', '\n']).len());
                        }
                    }
                });
            }
            let searching = !query.is_empty();
            let has_room_for_shortcut = find.width > 220.0;
            let tail = if searching {
                128.0
            } else if has_room_for_shortcut {
                52.0
            } else {
                0.0
            };
            let edit = egui::TextEdit::singleline(query)
                .id(find.id)
                .frame(egui::Frame::NONE)
                .hint_text(find.hint)
                .desired_width((ui.available_width() - tail).max(40.0));
            let response = ui.add(edit);
            if find.focus {
                response.request_focus();
                if find.select
                    && let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), find.id)
                {
                    let all = egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(query.chars().count()),
                    );
                    state.cursor.set_char_range(Some(all));
                    state.store(ui.ctx(), find.id);
                }
            }
            found.changed = response.changed();
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if ui.input(|i| i.modifiers.shift) {
                    found.previous = true;
                } else {
                    found.next = true;
                }
                response.request_focus();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if !searching {
                    if !has_room_for_shortcut {
                        return;
                    }
                    egui::Frame::new()
                        .stroke(Stroke::new(1.0, t.field_line))
                        .corner_radius(4)
                        .inner_margin(egui::Margin::symmetric(4, 0))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(crate::keys::FIND.label())
                                    .small()
                                    .weak(),
                            )
                        });
                    return;
                }
                // Laid out left to right against the right edge, so that Tab visits the
                // buttons in the order they read.
                let small = egui::TextStyle::Small.resolve(ui.style());
                let room = text_width(ui, find.count, &small) + 3.0 * (MINI_BUTTON_SIZE + 2.0);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add_space((ui.available_width() - room).max(0.0));
                    ui.label(egui::RichText::new(find.count).small().weak());
                    let [previous, next, clear] = find.keys;
                    let (previous, next, clear) =
                        (previous.as_str(), next.as_str(), clear.as_str());
                    if tip(mini_button(ui, glyphs::CHEVRON_UP), "Previous", previous).clicked() {
                        found.previous = true;
                    }
                    if tip(mini_button(ui, glyphs::CHEVRON_DOWN), "Next", next).clicked() {
                        found.next = true;
                    }
                    if tip(mini_button(ui, glyphs::CLOSE), "Clear", clear).clicked() {
                        found.cleared = true;
                    }
                });
            });
        });
    found
}

/// A tooltip: `text`, then `key` (a shortcut) in a weaker colour.
pub fn tip(response: Response, text: &str, key: &str) -> Response {
    response.on_hover_ui(|ui| {
        ui.horizontal(|ui| {
            ui.label(text);
            if !key.is_empty() {
                ui.weak(key);
            }
        });
    })
}

/// A tooltip with a title (and shortcut) over a sentence of explanation.
pub fn tip_explained(response: Response, title: &str, key: &str, body: &str) -> Response {
    response.on_hover_ui(|ui| {
        ui.set_max_width(280.0);
        ui.horizontal(|ui| {
            ui.strong(title);
            if !key.is_empty() {
                ui.weak(key);
            }
        });
        ui.label(body);
    })
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, Key};

    use crate::focus::tests::Screen;

    /// Text segments, then a button: and which of them has the keyboard.
    fn segments_then_a_button(
        ui: &mut egui::Ui,
        theme: &mut u8,
        button: &std::cell::Cell<egui::Id>,
    ) {
        super::text_segmented(ui, theme, &[(0, "Light"), (1, "Dark"), (2, "System")]);
        button.set(ui.button("After").id);
    }

    /// A row against the right edge, as a right-to-left one, that Tab visits in the order it
    /// reads.
    #[test]
    fn a_trailing_row_sits_at_the_right_edge_and_reads_left_to_right() {
        let mut s = Screen::new();
        let ids = std::cell::Cell::new([egui::Id::NULL; 3]);
        let rects = std::cell::Cell::new([egui::Rect::NOTHING; 2]);
        let edge = std::cell::Cell::new(0.0);
        let show = |ui: &mut egui::Ui| {
            ui.horizontal(|ui| {
                let field = ui.button("Before");
                super::trailing(ui, "tools", |ui| {
                    let one = ui.button("One");
                    let two = ui.button("Two");
                    ids.set([field.id, one.id, two.id]);
                    rects.set([one.rect, two.rect]);
                });
                edge.set(ui.max_rect().right());
            });
        };
        s.frame(show);
        s.frame(show);
        let [one, two] = rects.get();
        assert!(one.right() < two.left(), "One reads before Two");
        assert!(
            (two.right() - edge.get()).abs() < 1.0,
            "{two:?} at {}",
            edge.get()
        );
        let mut order = Vec::new();
        for _ in 0..3 {
            s.press(Key::Tab);
            s.frame(show);
            order.push(s.focused().unwrap());
        }
        assert_eq!(order, ids.get());
    }

    /// A segmented control is one stop for Tab, and Left and Right choose within it, up to
    /// its ends.
    #[test]
    fn segments_are_one_stop_chosen_with_left_and_right() {
        let mut s = Screen::new();
        let mut theme = 0;
        let button = std::cell::Cell::new(egui::Id::NULL);
        let frames = |s: &mut Screen, theme: &mut u8| {
            for _ in 0..2 {
                s.frame(|ui| segments_then_a_button(ui, theme, &button));
            }
        };
        frames(&mut s, &mut theme);
        s.press(Key::Tab);
        frames(&mut s, &mut theme);
        let control = s.focused();
        assert!(control.is_some() && control != Some(button.get()));
        for (key, expected) in [
            (Key::ArrowRight, 1),
            (Key::ArrowRight, 2),
            (Key::ArrowRight, 2),
            (Key::ArrowLeft, 1),
        ] {
            s.press(key);
            frames(&mut s, &mut theme);
            assert_eq!(theme, expected, "after {key:?}");
            assert_eq!(s.focused(), control, "the control keeps the keyboard");
        }
        s.press(Key::Tab);
        frames(&mut s, &mut theme);
        assert_eq!(
            s.focused(),
            Some(button.get()),
            "Tab leaves it for what follows"
        );
    }
}
