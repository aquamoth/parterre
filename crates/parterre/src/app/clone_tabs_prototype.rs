//! PROTOTYPE, throwaway (branch prototype/clone-tabs): five looks for the Clone dialog's tabs,
//! switched with the bar at the dialog's top (or `PARTERRE_TABS_PROTOTYPE=A..E` at start), to
//! pick one. Keyboard focus is not part of the question; Ctrl+Tab works in all of them.

use std::sync::atomic::{AtomicUsize, Ordering};

use eframe::egui::{self, Color32, CornerRadius, Id, Margin, Rect, Sense, Shape, Stroke, Ui, vec2};

use crate::widgets;

const NAMES: [&str; 5] = [
    "egui: selectable labels over a group",
    "Windows 11: selector bar over a card",
    "Windows 11: document tabs joined to the page",
    "macOS: segmented control on the box's edge",
    "Current: tabs as the card's header",
];

static VARIANT: AtomicUsize = AtomicUsize::new(usize::MAX);

fn variant() -> usize {
    let v = VARIANT.load(Ordering::Relaxed);
    if v != usize::MAX {
        return v;
    }
    let start = std::env::var("PARTERRE_TABS_PROTOTYPE")
        .ok()
        .and_then(|s| s.trim().bytes().next())
        .map_or(0, |b| {
            usize::from(b.to_ascii_uppercase().saturating_sub(b'A'))
        })
        .min(NAMES.len() - 1);
    VARIANT.store(start, Ordering::Relaxed);
    start
}

/// The prototype's own bar: ◀ variant ▶, plainly not part of the design.
pub fn switcher(ui: &mut Ui) {
    let v = variant();
    egui::Frame::new()
        .fill(Color32::from_rgb(0x22, 0x22, 0x2a))
        .corner_radius(14)
        .inner_margin(Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.visuals_mut().override_text_color = Some(Color32::WHITE);
                ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::from_gray(60);
                ui.visuals_mut().widgets.hovered.weak_bg_fill = Color32::from_gray(90);
                let step = |by: usize| VARIANT.store((v + by) % NAMES.len(), Ordering::Relaxed);
                if ui.small_button("◀").clicked() {
                    step(NAMES.len() - 1);
                }
                let letter = char::from(b'A' + v as u8);
                ui.label(format!("PROTOTYPE {letter}: {}", NAMES[v]));
                if ui.small_button("▶").clicked() {
                    step(1);
                }
            });
        });
    ui.add_space(8.0);
}

/// The tabs as the variant shown draws them; as `widgets::tabs` otherwise.
pub fn tabs<T: PartialEq + Copy, R>(
    ui: &mut Ui,
    id: Id,
    current: &mut T,
    items: &[(T, &str)],
    page: impl FnOnce(&mut Ui) -> R,
) -> R {
    keys(ui, current, items);
    match variant() {
        0 => egui_style(ui, current, items, page),
        1 => selector_bar(ui, id, current, items, page),
        2 => document_tabs(ui, id, current, items, page),
        3 => mac(ui, current, items, page),
        _ => widgets::tabs(ui, id, current, items, page),
    }
}

fn keys<T: PartialEq + Copy>(ui: &mut Ui, current: &mut T, items: &[(T, &str)]) {
    let selected = items.iter().position(|(v, _)| v == current).unwrap_or(0);
    let (previous, next) = ui.input_mut(|i| {
        let previous = i.consume_key(
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            egui::Key::Tab,
        );
        (
            previous,
            !previous && i.consume_key(egui::Modifiers::CTRL, egui::Key::Tab),
        )
    });
    if next || previous {
        let step = if next { 1 } else { items.len() - 1 };
        *current = items[(selected + step) % items.len()].0;
    }
}

/// A: egui's own idiom, as its demo app does it: selectable labels in a row, and the page in
/// egui's group frame.
fn egui_style<T: PartialEq + Copy, R>(
    ui: &mut Ui,
    current: &mut T,
    items: &[(T, &str)],
    page: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.horizontal(|ui| {
        for (value, label) in items {
            if ui.selectable_label(value == current, *label).clicked() {
                *current = *value;
            }
        }
    });
    ui.add_space(2.0);
    egui::Frame::group(ui.style())
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            page(ui)
        })
        .inner
}

/// B: Windows 11's SelectorBar: text items, the selected one in strong text over a short accent
/// pill; the page in a Fluent card under them, with room between.
fn selector_bar<T: PartialEq + Copy, R>(
    ui: &mut Ui,
    id: Id,
    current: &mut T,
    items: &[(T, &str)],
    page: impl FnOnce(&mut Ui) -> R,
) -> R {
    let t = widgets::tones(ui);
    let font = egui::FontId::proportional(14.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (i, (value, label)) in items.iter().enumerate() {
            let on = value == current;
            let galley = ui.painter().layout_no_wrap(
                label.to_string(),
                font.clone(),
                ui.visuals().strong_text_color(),
            );
            let size = vec2(galley.size().x + 24.0, 36.0);
            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
            let _ = id.with(i);
            if response.hovered() && !on {
                ui.painter().rect_filled(rect.shrink(2.0), 5, t.hover);
            }
            let color = if on || response.hovered() {
                ui.visuals().strong_text_color()
            } else {
                ui.visuals().weak_text_color()
            };
            let galley = ui
                .painter()
                .layout_no_wrap(label.to_string(), font.clone(), color);
            ui.painter().galley(
                rect.center() - galley.size() / 2.0 - vec2(0.0, 2.0),
                galley,
                color,
            );
            if on {
                let pill = Rect::from_center_size(
                    egui::pos2(rect.center().x, rect.bottom() - 4.0),
                    vec2(16.0, 3.0),
                );
                ui.painter().rect_filled(pill, 2, t.accent);
            }
            if response.clicked() {
                *current = *value;
            }
        }
    });
    ui.add_space(6.0);
    egui::Frame::new()
        .fill(t.group)
        .stroke(Stroke::new(1.0, t.field_line))
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            page(ui)
        })
        .inner
}

/// C: Windows 11's TabView, as File Explorer's and Terminal's: a strip of tabs on a darker
/// band, the selected one with rounded top corners in the page's colour, flowing into it; the
/// others flat, parted by thin lines.
fn document_tabs<T: PartialEq + Copy, R>(
    ui: &mut Ui,
    id: Id,
    current: &mut T,
    items: &[(T, &str)],
    page: impl FnOnce(&mut Ui) -> R,
) -> R {
    let t = widgets::tones(ui);
    let dark = ui.visuals().dark_mode;
    let band = if dark {
        Color32::from_gray(24)
    } else {
        Color32::from_gray(232)
    };
    let font = egui::TextStyle::Button.resolve(ui.style());
    let outer = ui.spacing().item_spacing;
    egui::Frame::new()
        .fill(t.group)
        .stroke(Stroke::new(1.0, t.field_line))
        .corner_radius(9)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let (strip, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());
            let top = CornerRadius {
                nw: 8,
                ne: 8,
                sw: 0,
                se: 0,
            };
            ui.painter().rect_filled(strip, top, band);
            let mut x = strip.left() + 8.0;
            let selected = items.iter().position(|(v, _)| v == current);
            for (i, (value, label)) in items.iter().enumerate() {
                let on = Some(i) == selected;
                let width = widgets_text_width(ui, label, &font) + 32.0;
                let rect = Rect::from_min_size(egui::pos2(x, strip.top() + 6.0), vec2(width, 32.0));
                x = rect.right();
                let response = ui.interact(rect, id.with(i), Sense::click());
                if on {
                    ui.painter().rect_filled(rect, top, t.group);
                } else {
                    if response.hovered() {
                        ui.painter()
                            .rect_filled(rect.shrink2(vec2(2.0, 3.0)), 5, t.hover);
                    }
                    // A thin line between two tabs that aren't selected.
                    let next_on = selected == Some(i + 1);
                    if i + 1 < items.len() && !next_on {
                        let y = rect.center().y;
                        ui.painter().vline(
                            rect.right(),
                            (y - 8.0)..=(y + 8.0),
                            Stroke::new(1.0, t.field_line),
                        );
                    }
                }
                let color = if on {
                    ui.visuals().strong_text_color()
                } else {
                    ui.visuals().text_color()
                };
                let galley = ui
                    .painter()
                    .layout_no_wrap(label.to_string(), font.clone(), color);
                ui.painter()
                    .galley(rect.center() - galley.size() / 2.0, galley, color);
                if response.clicked() {
                    *current = *value;
                }
            }
            egui::Frame::new()
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = outer;
                    page(ui)
                })
                .inner
        })
        .inner
}

/// D: macOS's tab view (NSTabView): a segmented control centred on the top edge of a
/// recessed, rounded box holding the page.
fn mac<T: PartialEq + Copy, R>(
    ui: &mut Ui,
    current: &mut T,
    items: &[(T, &str)],
    page: impl FnOnce(&mut Ui) -> R,
) -> R {
    let t = widgets::tones(ui);
    let dark = ui.visuals().dark_mode;
    let recessed = if dark {
        Color32::from_white_alpha(8)
    } else {
        Color32::from_black_alpha(9)
    };
    // The box and an opaque patch under the control are painted behind what comes after.
    let behind = ui.painter().add(Shape::Noop);
    let patch = ui.painter().add(Shape::Noop);
    let control = ui
        .vertical_centered(|ui| widgets::text_segmented(ui, current, items))
        .response
        .rect;
    let shown = egui::Frame::new()
        .inner_margin(Margin {
            left: 12,
            right: 12,
            top: 6,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            page(ui)
        });
    let area = Rect::from_min_max(
        egui::pos2(shown.response.rect.left(), control.center().y),
        shown.response.rect.max,
    );
    ui.painter().set(
        behind,
        Shape::Vec(vec![
            Shape::rect_filled(area, 8, recessed),
            Shape::rect_stroke(
                area,
                8,
                Stroke::new(1.0, t.field_line),
                egui::StrokeKind::Inside,
            ),
        ]),
    );
    // The control is translucent: the window's own colour under it hides the box's edge.
    ui.painter().set(
        patch,
        Shape::rect_filled(control, 9, ui.visuals().window_fill),
    );
    shown.inner
}

/// How wide `text` is in `font`.
fn widgets_text_width(ui: &Ui, text: &str, font: &egui::FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER)
        .size()
        .x
}
