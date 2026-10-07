//! The look of menus (the menu bar's on Windows and Linux, and the canvas's context menu) and of
//! the toolbar's popovers.
//!
//! egui's own menus are compact, with a small corner radius and a hard, offset shadow. These
//! follow current desktop menus instead (GNOME, Chrome): rounder, a soft shadow, roomy rows
//! with a rounded highlight under the pointer, and shortcuts right-aligned in a weaker colour.

use eframe::egui::{
    self, Atom, Button, Color32, CornerRadius, Id, Margin, Response, Shadow, Style, Ui, Vec2,
    WidgetText, vec2,
};
use parterre_core::glyphs;

use crate::widgets::paint_glyph;

/// Menus are at least this wide, so that short ones don't look cramped.
pub const MIN_WIDTH: f32 = 200.0;

/// The style of a menu, in place of egui's `menu_style`. egui applies it before making the
/// menu's frame, so it sets the frame too.
pub fn style(style: &mut Style) {
    egui::containers::menu::menu_style(style);
    frame(style);
    style.spacing.button_padding = vec2(12.0, 6.0);
    style.spacing.item_spacing.y = 0.0;
    let dark = style.visuals.dark_mode;
    let w = &mut style.visuals.widgets;
    w.hovered.weak_bg_fill = if dark {
        Color32::from_white_alpha(20)
    } else {
        Color32::from_black_alpha(14)
    };
    w.active.weak_bg_fill = if dark {
        Color32::from_white_alpha(32)
    } else {
        Color32::from_black_alpha(26)
    };
    w.open.weak_bg_fill = w.hovered.weak_bg_fill;
    for v in [&mut w.hovered, &mut w.active, &mut w.open] {
        v.corner_radius = CornerRadius::same(6);
        v.expansion = 0.0;
    }
}

/// The style of a toolbar popover: a menu's frame around ordinary controls.
pub fn popover_style(style: &mut Style) {
    frame(style);
    style.spacing.menu_margin = Margin::same(12);
    style.spacing.item_spacing.y = 8.0;
}

/// The frame shared by menus and popovers.
fn frame(style: &mut Style) {
    let dark = style.visuals.dark_mode;
    let v = &mut style.visuals;
    v.menu_corner_radius = CornerRadius::same(10);
    v.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 120 } else { 48 }),
    };
    // Lifted off the canvas: white in the light theme, a lighter grey in the dark one.
    if dark {
        v.window_fill = Color32::from_gray(40);
        v.window_stroke.color = Color32::from_white_alpha(20);
        v.widgets.inactive.fg_stroke.color = Color32::from_gray(220);
        v.widgets.noninteractive.bg_stroke.color = Color32::from_white_alpha(22);
    } else {
        v.window_fill = Color32::WHITE;
        v.window_stroke.color = Color32::from_black_alpha(28);
        v.widgets.inactive.fg_stroke.color = Color32::from_gray(30);
        v.widgets.noninteractive.bg_stroke.color = Color32::from_black_alpha(24);
    }
    style.spacing.menu_margin = Margin::same(6);
}

/// The content of a menu or popover, scrolling if it is taller than the window.
///
/// egui keeps a popup inside the window, moving it up when it would overflow the bottom, but it
/// moves a popup taller than the window to the top and cuts off the bottom. Scrolling keeps
/// all of it reachable. TortoiseGit's native menus can overflow the window instead; egui draws
/// inside the one window, and Wayland gives winit no way to place a popup outside it (#93).
pub fn fit_window<R>(ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> R {
    let frame = ui.spacing().menu_margin.sum().y + 2.0 * ui.visuals().window_stroke.width;
    let max_height = ui.ctx().content_rect().height() - frame;
    // The scroll area is as tall as the room it is given, and the popup's first, sizing pass
    // gives it `default_area_size`, which can be less than the window.
    ui.set_max_height(max_height);
    // A handle that shows without hovering, so that a menu cut short looks scrollable.
    ui.spacing_mut().scroll = egui::style::ScrollStyle::thin();
    egui::ScrollArea::vertical().show(ui, content).inner
}

/// A line between groups of items, with some air around it.
pub fn separator(ui: &mut Ui) {
    ui.add(egui::Separator::default().spacing(12.0));
}

/// What a menu item shows left of its label.
#[derive(Clone, Copy, Debug)]
pub enum Mark {
    None,
    Check(bool),
    Radio(bool),
}

/// The room left of a menu item's label for its mark.
pub const MARK: f32 = 16.0;

/// A menu item: room on the left for a check mark or radio dot, so that the labels of a menu
/// line up, and `shortcut` on the right.
pub fn item(ui: &mut Ui, label: impl Into<WidgetText>, shortcut: &str, mark: Mark) -> Response {
    let id = Id::new("menu-mark");
    let laid_out = Button::new((Atom::custom(id, Vec2::splat(MARK)), label.into()))
        .shortcut_text(shortcut)
        .atom_ui(ui);
    if let Some(rect) = laid_out.rect(id) {
        let color = ui.style().interact(&laid_out.response).text_color();
        match mark {
            Mark::Check(true) => paint_glyph(ui.painter(), rect, glyphs::CHECK, color),
            Mark::Radio(true) => {
                ui.painter().circle_filled(rect.center(), 3.5, color);
            }
            _ => {}
        }
    }
    laid_out.response
}

/// An item opening a submenu, lined up with [`item`]s, with a chevron on the right.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn submenu(ui: &mut Ui, label: &str, content: impl FnOnce(&mut Ui)) {
    let button = Button::new((Atom::custom(Id::new("menu-mark"), Vec2::splat(MARK)), label));
    submenu_button(ui, button, content);
}

/// An item opening a submenu, lined up with plain buttons (a node's menu), with a chevron on
/// the right.
pub fn plain_submenu(ui: &mut Ui, label: &str, content: impl FnOnce(&mut Ui)) {
    submenu_button(ui, Button::new(label), content);
}

fn submenu_button(ui: &mut Ui, button: Button, content: impl FnOnce(&mut Ui)) {
    const ARROW: f32 = 12.0;
    let button = button.right_text(Atom::custom(Id::new("menu-arrow"), Vec2::splat(ARROW)));
    let (response, _) = egui::containers::menu::SubMenuButton::from_button(button)
        .ui(ui, |ui| fit_window(ui, content));
    let padding = ui.spacing().button_padding.x;
    let center = egui::pos2(
        response.rect.right() - padding - ARROW / 2.0,
        response.rect.center().y,
    );
    let color = ui.visuals().weak_text_color();
    paint_glyph(
        ui.painter(),
        egui::Rect::from_center_size(center, Vec2::splat(ARROW)),
        glyphs::CHEVRON_RIGHT,
        color,
    );
}
