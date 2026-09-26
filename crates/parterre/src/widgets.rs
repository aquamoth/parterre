//! Controls of the toolbar, its popovers and the settings window, drawn to match the menus
//! (`menu.rs`): icon buttons, segmented buttons and switches. The icons are
//! [`parterre_core::glyphs`].

use eframe::egui::{
    self, Color32, CornerRadius, Id, Painter, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui,
    Vec2, vec2,
};
use parterre_core::glyphs::{self, Glyph, Part};

/// Size of an icon in a button.
pub const ICON: f32 = 18.0;
/// Side of an icon button.
pub const BUTTON: f32 = 30.0;

/// Colours beyond egui's visuals.
pub struct Tones {
    /// Background and icon of a toggle that is on.
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
    if ui.visuals().dark_mode {
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
    let scale = rect.width() / glyphs::SIZE;
    let at = |p: [f32; 2]| rect.min + vec2(p[0], p[1]) * scale;
    let stroke = Stroke::new(glyphs::STROKE * scale, color);
    for part in glyph {
        match *part {
            Part::Path(d) => {
                // Within a fifth of a pixel of the true curve.
                for line in glyphs::flatten(d, 0.2 / scale).unwrap_or_default() {
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
    ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
}

fn icon_color(ui: &Ui, on: bool) -> Color32 {
    if !ui.is_enabled() {
        ui.visuals().weak_text_color()
    } else if on {
        tones(ui).on_fg
    } else {
        ui.visuals().text_color()
    }
}

/// A square button showing `glyph`, tinted while `on`.
pub fn icon_button(ui: &mut Ui, glyph: Glyph, on: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON), Sense::click());
    paint_background(ui, rect, &response, on, false);
    let icon = Rect::from_center_size(rect.center(), Vec2::splat(ICON));
    paint_glyph(ui.painter(), icon, glyph, icon_color(ui, on));
    response
}

/// A small icon button, e.g. inside the find field.
pub fn mini_button(ui: &mut Ui, glyph: Glyph) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    paint_background(ui, rect, &response, false, false);
    let icon = Rect::from_center_size(rect.center(), Vec2::splat(14.0));
    paint_glyph(ui.painter(), icon, glyph, icon_color(ui, false));
    response
}

/// A text button on a light fill, e.g. "Reset" in the zoom popover.
pub fn text_button(ui: &mut Ui, text: &str) -> Response {
    let color = if ui.is_enabled() {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::TextStyle::Button.resolve(ui.style()),
        color,
    );
    let size = vec2(galley.size().x + 24.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let t = tones(ui);
    let fill = if response.is_pointer_button_down_on() || response.hovered() {
        t.press
    } else {
        t.seg_bg
    };
    ui.painter().rect_filled(rect, CornerRadius::same(7), fill);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    response
}

/// A dialog's main button: white text on the accent colour, at least `min_width` wide.
pub fn primary_button(ui: &mut Ui, text: &str, min_width: f32) -> Response {
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::TextStyle::Button.resolve(ui.style()),
        Color32::WHITE,
    );
    let size = vec2((galley.size().x + 32.0).max(min_width), 30.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let accent = tones(ui).accent;
    let fill = if response.is_pointer_button_down_on() {
        accent.gamma_multiply(0.8)
    } else if response.hovered() {
        accent.gamma_multiply(0.9)
    } else {
        accent
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(8), fill.to_opaque());
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    response
}

/// A button that opens a popover (with [`egui::Popup::from_toggle_button_response`]), shown
/// pressed while it is open. `glyph` of `None` makes it a narrow chevron.
pub fn popover_button(ui: &mut Ui, id: Id, glyph: Option<Glyph>, on: bool) -> Response {
    let size = if glyph.is_some() {
        Vec2::splat(BUTTON)
    } else {
        vec2(20.0, BUTTON)
    };
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let response = ui.interact(rect, id, Sense::click());
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
    paint_background(ui, rect, &response, on, open);
    let color = icon_color(ui, on);
    match glyph {
        Some(glyph) => {
            let icon = Rect::from_center_size(rect.center(), Vec2::splat(ICON));
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
    let widths = vec![BUTTON; items.len()];
    segments(ui, &widths, |ui, i, rect, response| {
        let (value, glyph) = items[i];
        paint_segment(ui, rect, &response, value == current);
        let icon = Rect::from_center_size(rect.center(), Vec2::splat(ICON));
        paint_glyph(ui.painter(), icon, glyph, icon_color(ui, false));
        tip(value, response).clicked().then_some(value)
    })
}

/// Text segments of which one is selected, e.g. the theme in the settings.
pub fn text_segmented<T: PartialEq + Copy>(ui: &mut Ui, current: &mut T, items: &[(T, &str)]) {
    let color = ui.visuals().text_color();
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galleys: Vec<_> = items
        .iter()
        .map(|(_, text)| {
            ui.painter()
                .layout_no_wrap(text.to_string(), font.clone(), color)
        })
        .collect();
    let widths: Vec<f32> = galleys.iter().map(|g| g.size().x + 18.0).collect();
    let selected = *current;
    let clicked = segments(ui, &widths, |ui, i, rect, response| {
        let value = items[i].0;
        paint_segment(ui, rect, &response, value == selected);
        let galley = galleys[i].clone();
        ui.painter()
            .galley(rect.center() - galley.size() / 2.0, galley, color);
        response.clicked().then_some(value)
    });
    if let Some(value) = clicked {
        *current = value;
    }
}

/// Segments of the given widths on a shared background, laid out by hand so that they read
/// left to right in any layout. `segment` paints one and says whether it was chosen.
fn segments<T>(
    ui: &mut Ui,
    widths: &[f32],
    mut segment: impl FnMut(&mut Ui, usize, Rect, Response) -> Option<T>,
) -> Option<T> {
    const PAD: f32 = 2.0;
    let total = widths.iter().sum::<f32>() + PAD * (widths.len() + 1) as f32;
    let (group, response) = ui.allocate_exact_size(vec2(total, BUTTON), Sense::hover());
    ui.painter()
        .rect_filled(group, CornerRadius::same(9), tones(ui).seg_bg);
    let mut x = group.left() + PAD;
    let mut chosen = None;
    for (i, &w) in widths.iter().enumerate() {
        let rect = Rect::from_min_size(
            egui::pos2(x, group.top() + PAD),
            vec2(w, BUTTON - 2.0 * PAD),
        );
        let r = ui.interact(rect, response.id.with(i), Sense::click());
        if let Some(value) = segment(ui, i, rect, r) {
            chosen = Some(value);
        }
        x += w + PAD;
    }
    chosen
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

/// An on/off switch.
pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(36.0, 20.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = tones(ui);
    let how_on = ui.ctx().animate_bool_responsive(response.id, *on);
    let off_fill = t.seg_bg;
    let fill = lerp_color(off_fill, t.accent, how_on);
    let fill = if ui.is_enabled() {
        fill
    } else {
        fill.gamma_multiply(0.5)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(10), fill);
    if how_on < 1.0 {
        painter.rect_stroke(
            rect,
            CornerRadius::same(10),
            Stroke::new(1.0, t.field_line.gamma_multiply(1.0 - how_on)),
            StrokeKind::Inside,
        );
    }
    let x = egui::lerp(rect.left() + 10.0..=rect.right() - 10.0, how_on);
    let knob = egui::pos2(x, rect.center().y);
    painter.circle_filled(knob + vec2(0.0, 0.5), 8.0, Color32::from_black_alpha(60));
    painter.circle_filled(knob, 8.0, Color32::WHITE);
    response
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
        mix(a.a(), b.a()),
    )
}

/// A single-line text field in a rounded frame that shows on any background.
pub fn text_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    let t = tones(ui);
    egui::Frame::new()
        .fill(t.field)
        .stroke(Stroke::new(1.0, t.field_line))
        .corner_radius(7)
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .frame(egui::Frame::NONE)
                    .hint_text(hint)
                    .desired_width(width - 16.0),
            )
        })
        .inner
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
