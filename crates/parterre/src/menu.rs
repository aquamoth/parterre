//! Menus and popovers styled after current desktop menus (GNOME, Chrome) rather than egui's
//! compact ones: rounder, a soft shadow, roomy rows, and shortcuts in a weaker colour.

use eframe::egui::{
    self, Atom, Button, Color32, CornerRadius, Id, Margin, Response, Shadow, Style, Ui, Vec2,
    WidgetText, vec2,
};
use parterre_core::glyphs;

use crate::widgets::paint_glyph;

pub const MIN_WIDTH: f32 = 200.0;

/// In place of egui's `menu_style`, which egui applies before making the menu's frame: so it
/// sets the frame too.
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

pub fn popover_style(style: &mut Style) {
    frame(style);
    style.spacing.menu_margin = Margin::same(12);
    style.spacing.item_spacing.y = 8.0;
}

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

/// Scrolls a menu or popover taller than the window: egui moves such a popup to the top and cuts
/// off its bottom, and Wayland gives winit no way to place a popup outside the window (#93).
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

pub fn separator(ui: &mut Ui) {
    ui.add(egui::Separator::default().spacing(12.0));
}

#[derive(Clone, Copy, Debug)]
pub enum Mark {
    None,
    Check(bool),
    Radio(bool),
}

pub const MARK_WIDTH: f32 = 16.0;

fn mark_id() -> Id {
    Id::new("menu-mark")
}

/// Room for a mark before every item's label, so that the labels of a menu line up.
fn mark_room() -> Atom<'static> {
    Atom::custom(mark_id(), Vec2::splat(MARK_WIDTH))
}

pub fn item(ui: &mut Ui, label: impl Into<WidgetText>, shortcut: &str, mark: Mark) -> Response {
    let laid_out = Button::new((mark_room(), label.into()))
        .shortcut_text(shortcut)
        .atom_ui(ui);
    if let Some(rect) = laid_out.rect(mark_id()) {
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

/// Lined up with [`item`]s. `open` opens or closes it, as the keyboard does in the menu bar;
/// `None` leaves it to the pointer.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn submenu(
    ui: &mut Ui,
    label: impl Into<WidgetText>,
    open: Option<bool>,
    content: impl FnOnce(&mut Ui),
) {
    let button = Button::new((mark_room(), label.into()));
    submenu_button(ui, button, open, content);
}

/// Lined up with plain buttons, as in a node's menu.
pub fn plain_submenu(ui: &mut Ui, label: &str, content: impl FnOnce(&mut Ui)) {
    submenu_button(ui, Button::new(label), None, content);
}

fn submenu_button(ui: &mut Ui, button: Button, open: Option<bool>, content: impl FnOnce(&mut Ui)) {
    use egui::containers::menu::{MenuState, SubMenu};
    const CHEVRON_SIZE: f32 = 12.0;
    if let Some(open) = open {
        // The id egui's submenu button gives itself, next.
        let submenu = SubMenu::id_from_widget_id(ui.next_auto_id());
        if open {
            // Shown, as far as egui's menu state knows, so that it isn't closed at once.
            MenuState::mark_shown(ui.ctx(), submenu);
        }
        MenuState::from_ui(ui, |state, _| {
            if open {
                state.open_item = Some(submenu);
            } else if state.open_item == Some(submenu) {
                state.open_item = None;
            }
        });
    }
    let button = button.right_text(Atom::custom(
        Id::new("menu-arrow"),
        Vec2::splat(CHEVRON_SIZE),
    ));
    let (response, _) = egui::containers::menu::SubMenuButton::from_button(button)
        .ui(ui, |ui| fit_window(ui, content));
    let padding = ui.spacing().button_padding.x;
    let center = egui::pos2(
        response.rect.right() - padding - CHEVRON_SIZE / 2.0,
        response.rect.center().y,
    );
    let color = ui.visuals().weak_text_color();
    paint_glyph(
        ui.painter(),
        egui::Rect::from_center_size(center, Vec2::splat(CHEVRON_SIZE)),
        glyphs::CHEVRON_RIGHT,
        color,
    );
}
