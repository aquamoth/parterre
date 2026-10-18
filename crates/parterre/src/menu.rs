//! Menus and popovers styled after current desktop menus (GNOME, Chrome) rather than egui's
//! compact ones: rounder, a soft shadow, roomy rows, and shortcuts in a weaker colour.

use eframe::egui::{
    self, Atom, Button, Color32, CornerRadius, Id, Margin, Pos2, Response, Shadow, Style, Ui, Vec2,
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

/// `response`'s right-click menu, as egui's [`egui::Popup::context_menu`], which the
/// context-menu key ([`crate::keys::CONTEXT_MENU`]) opens too: at `key_pressed_at`, the item's
/// own place rather than the pointer's, with the keyboard on its first item and back on what had
/// it once the menu closes (#387). Esc closes it, and only it ([`ContextMenus`]).
pub fn context_menu<'a>(response: &Response, key_pressed_at: Option<Pos2>) -> egui::Popup<'a> {
    let ctx = &response.ctx;
    let menu = egui::Popup::default_response_id(response);
    let open = egui::Popup::is_id_open(ctx, menu);
    if open || key_pressed_at.is_some() {
        let open_menu = open_menu_id(ctx);
        ctx.data_mut(|d| d.insert_temp(open_menu, menu));
    }
    if let Some(at) = key_pressed_at.filter(|_| !open) {
        open_by_key(ctx, menu, at);
    } else if response.secondary_clicked() {
        ctx.data_mut(|d| {
            d.remove::<Pos2>(key_place_id(menu));
            d.remove::<Id>(focus_back_id(menu));
        });
    } else if !open && let Some(had_focus) = ctx.data(|d| d.get_temp::<Id>(focus_back_id(menu))) {
        ctx.data_mut(|d| d.remove::<Id>(focus_back_id(menu)));
        ctx.memory_mut(|m| m.request_focus(had_focus));
    }
    // egui's popup is a stop of its own, before its items (the handle that moves it): on, then.
    let handle = menu.with("move");
    if ctx.memory(|m| m.focused()) == Some(handle) {
        ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::Next));
    }
    let popup = egui::Popup::context_menu(response);
    let key_place = ctx.data(|d| d.get_temp::<Pos2>(key_place_id(menu)));
    match key_place.filter(|_| egui::Popup::is_id_open(ctx, menu)) {
        Some(at) => popup.at_position(at),
        None => popup,
    }
}

fn open_by_key(ctx: &egui::Context, menu: Id, at: Pos2) {
    egui::Popup::open_id(ctx, menu);
    let had_focus = ctx.memory(|m| m.focused());
    ctx.data_mut(|d| {
        d.insert_temp(key_place_id(menu), at);
        if let Some(had_focus) = had_focus {
            d.insert_temp(focus_back_id(menu), had_focus);
        }
    });
    // Into the menu: the first of its items made after this takes the keyboard.
    ctx.memory_mut(|m| {
        if let Some(had_focus) = had_focus {
            m.surrender_focus(had_focus);
        }
        m.move_focus(egui::FocusDirection::Next);
    });
}

fn key_place_id(menu: Id) -> Id {
    menu.with("by-key")
}

fn focus_back_id(menu: Id) -> Id {
    menu.with("keyboard-back")
}

fn open_menu_id(ctx: &egui::Context) -> Id {
    Id::new("open-context-menu").with(ctx.viewport_id())
}

/// Esc closes the open context menu ([`context_menu`]), and nothing else: egui closes it too,
/// but leaves the key to the dialog or window it is in, which closed as well.
#[derive(Debug, Default)]
pub struct ContextMenus;

impl egui::Plugin for ContextMenus {
    fn debug_name(&self) -> &'static str {
        "parterre-context-menus"
    }

    fn on_begin_pass(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let key = open_menu_id(&ctx);
        let Some(menu) = ctx.data(|d| d.get_temp::<Id>(key)) else {
            return;
        };
        if !egui::Popup::is_id_open(&ctx, menu) {
            ctx.data_mut(|d| d.remove::<Id>(key));
        } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            egui::Popup::close_id(&ctx, menu);
        }
    }
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

#[cfg(test)]
mod tests {
    use eframe::egui::{self, Key};

    use crate::focus::tests::Screen;
    use crate::keys::CONTEXT_MENU;

    fn button_with_menu(ui: &mut egui::Ui, ids: &std::cell::Cell<[egui::Id; 2]>) {
        let button = ui.button("Item");
        let key_pressed = button.has_focus() && ui.input_mut(|i| CONTEXT_MENU.consume(i));
        let key_pressed_at = key_pressed.then(|| button.rect.left_bottom());
        let mut first = egui::Id::NULL;
        super::context_menu(&button, key_pressed_at).show(|ui| {
            first = ui.button("Copy").id;
            let _ = ui.button("Paste");
        });
        ids.set([button.id, first]);
    }

    #[test]
    fn the_key_opens_the_menu_with_the_keyboard_in_it() {
        let mut s = Screen::new();
        let ids = std::cell::Cell::new([egui::Id::NULL; 2]);
        s.frame(|ui| button_with_menu(ui, &ids));
        let [button, _] = ids.get();
        s.ctx.memory_mut(|m| m.request_focus(button));
        s.frame(|ui| button_with_menu(ui, &ids));
        s.key(CONTEXT_MENU.key, CONTEXT_MENU.modifiers);
        s.frame(|ui| button_with_menu(ui, &ids));
        s.frame(|ui| button_with_menu(ui, &ids));
        let [_, first] = ids.get();
        let popup = egui::Popup::default_response_id(&s.ctx.read_response(button).unwrap());
        assert!(egui::Popup::is_id_open(&s.ctx, popup), "open");
        assert_eq!(s.focused(), Some(first), "on its first item");
        s.press(Key::Escape);
        for _ in 0..3 {
            s.frame(|ui| button_with_menu(ui, &ids));
        }
        assert!(!egui::Popup::is_id_open(&s.ctx, popup), "closed");
        assert_eq!(s.focused(), Some(button), "the keyboard back");
    }
}
