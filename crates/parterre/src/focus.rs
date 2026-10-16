//! Keyboard focus as desktop GUIs show it (#387). egui moves the focus with Tab and Shift+Tab, in
//! the order widgets are laid out, and with the arrow keys to the nearest widget that way, but
//! shows no focus beyond a text field's cursor. Here:
//!
//! - Which widget has the keyboard shows once the keyboard is used, as on Windows and in
//!   browsers; a click hides it again ([`Cues`]).
//! - parterre's widgets show it themselves ([`shows_own`], [`ring`]); any other, such as egui's
//!   own buttons, gets a ring painted around it.
//! - In a dialog, a click on a button gives it the keyboard, as on Windows and Linux, but not on
//!   macOS, where a click never moves the keyboard to a button ([`take_on_click`]). A click on a
//!   list's row gives the list the keyboard everywhere ([`take_on_row_click`]). Outside dialogs
//!   a click leaves the keyboard where it was, as a toolbar's buttons do.
//! - A control of several parts, such as tabs or a list, is one stop, which keeps the arrow keys
//!   it moves within itself by ([`claim_arrows`]).

use eframe::egui::{
    self, Color32, Context, Event, EventFilter, Id, Key, Painter, Rect, Response, Stroke,
    StrokeKind, Ui,
};

use crate::keys::Platform;
use crate::widgets;

const RING_WIDTH: f32 = 2.0;
const FALLBACK_RING_RADIUS: u8 = 4;
const BUTTONS_TAKE_FOCUS: bool = !matches!(Platform::CURRENT, Platform::Mac);

/// Rings the widget with the keyboard while the keyboard was used last, unless the widget shows
/// that itself.
#[derive(Debug, Default)]
pub struct Cues {
    keyboard_used_last: bool,
}

impl egui::Plugin for Cues {
    fn debug_name(&self) -> &'static str {
        "parterre-focus-cues"
    }

    fn input_hook(&mut self, _ctx: &Context, input: &mut egui::RawInput) {
        for event in &input.events {
            match event {
                Event::Key {
                    key, pressed: true, ..
                } if reveals_focus(*key) => self.keyboard_used_last = true,
                Event::PointerButton { pressed: true, .. } => self.keyboard_used_last = false,
                _ => {}
            }
        }
    }

    fn on_begin_pass(&mut self, ui: &mut Ui) {
        let own_cue = own_cue_id(ui.ctx());
        ui.ctx().data_mut(|d| d.remove::<Id>(own_cue));
    }

    fn on_end_pass(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx();
        if !self.keyboard_used_last || ctx.text_edit_focused() {
            return;
        }
        let Some(focused) = ctx.memory(|m| m.focused()) else {
            return;
        };
        let own_cue = own_cue_id(ctx);
        if ctx.data(|d| d.get_temp::<Id>(own_cue)) == Some(focused) {
            return;
        }
        let Some(response) = ctx.read_response(focused) else {
            return;
        };
        let accent = widgets::tones(ui).accent;
        let painter = ctx.layer_painter(response.layer_id);
        let in_view = response.interact_rect;
        if in_view == response.rect {
            paint_ring(&painter, response.rect, FALLBACK_RING_RADIUS, accent);
        } else {
            // Scrolled partly out of a list: ringed inside the part in view.
            let painter = painter.with_clip_rect(in_view);
            let rect = in_view.shrink(RING_WIDTH);
            paint_ring(&painter, rect, FALLBACK_RING_RADIUS, accent);
        }
    }
}

/// `Sense::click_and_drag()` without its stop for Tab: for what works by the pointer alone, such
/// as the graph, a text view or a divider, which the window's own keys act on.
pub fn pointer_only() -> egui::Sense {
    egui::Sense::CLICK | egui::Sense::DRAG
}

fn reveals_focus(key: Key) -> bool {
    matches!(
        key,
        Key::Tab
            | Key::ArrowUp
            | Key::ArrowDown
            | Key::ArrowLeft
            | Key::ArrowRight
            | Key::Home
            | Key::End
            | Key::PageUp
            | Key::PageDown
            // Shift+F10 opens the context menu of what has the keyboard.
            | Key::F10
    )
}

fn own_cue_id(ctx: &Context) -> Id {
    Id::new("focus-shown").with(ctx.viewport_id())
}

fn keyboard_used_last(ctx: &Context) -> bool {
    ctx.with_plugin(|cues: &mut Cues| cues.keyboard_used_last)
        .unwrap_or(false)
}

/// Whether `response`'s widget is to paint, now, that it has the keyboard; asking keeps [`Cues`]
/// from ringing it.
pub fn shows_own(response: &Response) -> bool {
    shows_own_by_id(&response.ctx, response.id)
}

/// [`shows_own`], for a widget that paints its parts before its own response is made.
pub fn shows_own_by_id(ctx: &Context, id: Id) -> bool {
    if !ctx.memory(|m| m.has_focus(id)) {
        return false;
    }
    let own_cue = own_cue_id(ctx);
    ctx.data_mut(|d| d.insert_temp(own_cue, id));
    keyboard_used_last(ctx)
}

pub fn ring(ui: &Ui, response: &Response, corner_radius: u8) {
    if shows_own(response) {
        let accent = widgets::tones(ui).accent;
        paint_ring(ui.painter(), response.rect, corner_radius, accent);
    }
}

/// Against the outside of `rect`, with no gap, as Windows 11 draws it.
fn paint_ring(painter: &Painter, rect: Rect, corner_radius: u8, color: Color32) {
    let stroke = Stroke::new(RING_WIDTH, color);
    painter.rect_stroke(rect, corner_radius, stroke, StrokeKind::Outside);
}

/// The arrow keys a control of several parts moves within itself by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrows {
    LeftRight,
    UpDown,
}

impl Arrows {
    fn keys(self) -> [Key; 2] {
        match self {
            Arrows::LeftRight => [Key::ArrowLeft, Key::ArrowRight],
            Arrows::UpDown => [Key::ArrowUp, Key::ArrowDown],
        }
    }

    fn filter(self) -> EventFilter {
        EventFilter {
            horizontal_arrows: self == Arrows::LeftRight,
            vertical_arrows: self == Arrows::UpDown,
            ..Default::default()
        }
    }
}

/// While `response`'s widget has the keyboard, `arrows` act on it rather than move the keyboard
/// away. Call every frame.
pub fn claim_arrows(response: &Response, arrows: Arrows) {
    if !response.has_focus() {
        return;
    }
    let ctx = &response.ctx;
    // egui applies the filter from the frame after the one the widget got the keyboard in, so
    // an arrow pressed in that first frame would still move the keyboard away: that move is
    // called off.
    let pressed = ctx.input(|i| {
        i.raw.events.iter().any(|e| {
            matches!(e, Event::Key { key, pressed: true, modifiers, .. }
                if !modifiers.any() && arrows.keys().contains(key))
        })
    });
    ctx.memory_mut(|m| {
        m.set_focus_lock_filter(response.id, arrows.filter());
        if pressed {
            m.move_focus(egui::FocusDirection::None);
        }
    });
}

/// For a dialog: a click gives the keyboard to the button, switch or other widget it clicks that
/// can have it (not on macOS), so that Tab goes on from there and Space clicks it again; to the
/// control a part clicked belongs to ([`take_on_part_click`]); to a list whose row it clicks
/// ([`take_on_row_click`]). Call every frame, before the window's content.
pub fn take_on_click(ctx: &Context) {
    let taking_pass = taking_pass_id(ctx);
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(taking_pass, pass));
    if !BUTTONS_TAKE_FOCUS {
        return;
    }
    let Some(clicked) = ctx.interaction_snapshot(|s| s.clicked) else {
        return;
    };
    let takes = ctx.read_response(clicked).is_some_and(|r| {
        r.sense.senses_click() && r.sense.is_focusable() && !r.sense.senses_drag()
    });
    if takes {
        ctx.memory_mut(|m| m.request_focus(clicked));
    }
}

fn taking_pass_id(ctx: &Context) -> Id {
    Id::new("focus-on-click").with(ctx.viewport_id())
}

/// In a window that [`take_on_click`]s, a click on `part` gives the keyboard to the `control` of
/// buttons it belongs to, such as tabs, as a click on a button would.
pub fn take_on_part_click(part: &Response, control: Id) {
    if BUTTONS_TAKE_FOCUS {
        take_on_row_click(part, control);
    }
}

/// In a window that [`take_on_click`]s, a click on `row` gives the keyboard to `list`, on any
/// platform. Elsewhere the keyboard stays where it was, and the window's own keys with it.
pub fn take_on_row_click(row: &Response, list: Id) {
    if !row.clicked() {
        return;
    }
    let ctx = &row.ctx;
    let taking_pass = taking_pass_id(ctx);
    if ctx.data(|d| d.get_temp::<u64>(taking_pass)) == Some(ctx.cumulative_pass_nr()) {
        ctx.memory_mut(|m| m.request_focus(list));
    }
}

#[cfg(test)]
pub mod tests {
    use eframe::egui::{self, Event, Key, Pos2, Rect, Shape, Vec2, epaint::ClippedShape, vec2};

    use super::{Arrows, Cues};

    const SCREEN_SIZE: Vec2 = vec2(800.0, 600.0);
    const FRAME_SECONDS: f64 = 1.0 / 60.0;

    /// A window of its own, frame by frame, with parterre's focus plugins in it.
    pub struct Screen {
        pub ctx: egui::Context,
        time: f64,
        pub events: Vec<Event>,
        pub shapes: Vec<ClippedShape>,
    }

    impl Screen {
        pub fn new() -> Screen {
            let ctx = egui::Context::default();
            ctx.add_plugin(Cues::default());
            ctx.add_plugin(crate::menu::ContextMenus);
            Screen::of(ctx)
        }

        pub fn of(ctx: egui::Context) -> Screen {
            Screen {
                ctx,
                time: 0.0,
                events: Vec::new(),
                shapes: Vec::new(),
            }
        }

        /// One frame of `add`, with the events queued.
        pub fn frame(&mut self, mut add: impl FnMut(&mut egui::Ui)) {
            self.time += FRAME_SECONDS;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN_SIZE)),
                time: Some(self.time),
                events: std::mem::take(&mut self.events),
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, |root| {
                egui::CentralPanel::default().show(root, |ui| add(ui));
            });
            output.textures_delta.clear();
            self.shapes = output.shapes;
        }

        pub fn key(&mut self, key: Key, modifiers: egui::Modifiers) {
            for pressed in [true, false] {
                self.events.push(Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers,
                });
            }
        }

        pub fn press(&mut self, key: Key) {
            self.key(key, egui::Modifiers::NONE);
        }

        pub fn click(&mut self, at: Pos2) {
            self.events.push(Event::PointerMoved(at));
            self.events
                .extend([true, false].map(|pressed| Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }));
        }

        pub fn focused(&self) -> Option<egui::Id> {
            self.ctx.memory(|m| m.focused())
        }

        /// Whether a ring is painted around `rect`: an accent line just outside it.
        pub fn ringed(&self, rect: Rect) -> bool {
            const MAX_GAP: f32 = 4.0;
            let accent = crate::widgets::tones_of(false).accent;
            let rings = |r: &egui::epaint::RectShape| {
                r.stroke.color == accent
                    && r.stroke.width >= super::RING_WIDTH
                    && r.rect.contains_rect(rect)
                    && r.rect.width() <= rect.width() + 2.0 * MAX_GAP
            };
            self.shapes
                .iter()
                .any(|clipped| any_rect(&clipped.shape, &rings))
        }
    }

    fn any_rect(shape: &Shape, found: &impl Fn(&egui::epaint::RectShape) -> bool) -> bool {
        match shape {
            Shape::Rect(r) => found(r),
            Shape::Vec(shapes) => shapes.iter().any(|s| any_rect(s, found)),
            _ => false,
        }
    }

    fn two_buttons(ui: &mut egui::Ui, rects: &mut [Rect; 2]) {
        ui.horizontal(|ui| {
            rects[0] = ui.button("One").rect;
            rects[1] = ui.button("Two").rect;
        });
    }

    fn dialog_of_two_buttons(ui: &mut egui::Ui, rects: &mut [Rect; 2]) {
        super::take_on_click(ui.ctx());
        two_buttons(ui, rects);
    }

    #[test]
    fn a_widget_with_the_keyboard_is_ringed_once_tab_is_pressed_until_a_click() {
        let mut s = Screen::new();
        let mut rects = [Rect::NOTHING; 2];
        s.frame(|ui| two_buttons(ui, &mut rects));
        assert!(!s.ringed(rects[0]));
        s.press(Key::Tab);
        s.frame(|ui| two_buttons(ui, &mut rects));
        assert!(s.focused().is_some());
        assert!(s.ringed(rects[0]), "the first button, after Tab");
        s.press(Key::Tab);
        s.frame(|ui| two_buttons(ui, &mut rects));
        assert!(s.ringed(rects[1]) && !s.ringed(rects[0]), "then the second");
        // A press anywhere hides it; the focus is where the click leaves it.
        s.click(rects[1].center());
        s.frame(|ui| dialog_of_two_buttons(ui, &mut rects));
        s.frame(|ui| dialog_of_two_buttons(ui, &mut rects));
        assert!(
            s.focused().is_some(),
            "the click gave the button the keyboard"
        );
        assert!(!s.ringed(rects[1]), "but shows it no ring");
    }

    #[test]
    fn a_widget_that_shows_its_own_focus_gets_no_ring() {
        let mut s = Screen::new();
        let mut rect = Rect::NOTHING;
        let mut own = false;
        let show = |ui: &mut egui::Ui, own: &mut bool, rect: &mut Rect| {
            let (r, response) =
                ui.allocate_exact_size(egui::vec2(40.0, 20.0), egui::Sense::click());
            *rect = r;
            *own = super::shows_own(&response);
        };
        s.press(Key::Tab);
        s.frame(|ui| show(ui, &mut own, &mut rect));
        s.frame(|ui| show(ui, &mut own, &mut rect));
        assert!(own, "it is told to show it");
        assert!(!s.ringed(rect), "and isn't ringed besides");
    }

    #[test]
    fn a_click_gives_a_button_the_keyboard_only_where_asked() {
        let mut s = Screen::new();
        let mut rects = [Rect::NOTHING; 2];
        s.frame(|ui| two_buttons(ui, &mut rects));
        s.click(rects[1].center());
        s.frame(|ui| two_buttons(ui, &mut rects));
        assert_eq!(s.focused(), None, "as egui does");
        s.frame(|ui| dialog_of_two_buttons(ui, &mut rects));
        s.click(rects[1].center());
        s.frame(|ui| dialog_of_two_buttons(ui, &mut rects));
        if !super::BUTTONS_TAKE_FOCUS {
            assert_eq!(s.focused(), None, "on macOS, not even in a dialog");
            return;
        }
        assert!(s.focused().is_some(), "in a dialog");
        // Shift+Tab goes back from the button clicked.
        s.key(Key::Tab, egui::Modifiers::SHIFT);
        s.frame(|ui| dialog_of_two_buttons(ui, &mut rects));
        s.frame(|ui| dialog_of_two_buttons(ui, &mut rects));
        assert!(s.ringed(rects[0]));
    }

    /// Right after Tab gives it the keyboard, before egui applies its claim, a widget keeps the
    /// arrow keys it claims too.
    #[test]
    fn a_widget_keeps_the_arrow_keys_it_claims_from_its_first_frame() {
        let mut s = Screen::new();
        let first = std::cell::Cell::new(egui::Id::NULL);
        let show = |ui: &mut egui::Ui| {
            ui.horizontal(|ui| {
                let one = ui.button("One");
                super::claim_arrows(&one, Arrows::LeftRight);
                first.set(one.id);
                let _ = ui.button("Two");
            });
        };
        s.frame(show);
        s.press(Key::Tab);
        s.frame(show);
        assert_eq!(s.focused(), Some(first.get()));
        s.press(Key::ArrowRight);
        s.frame(show);
        s.frame(show);
        assert_eq!(s.focused(), Some(first.get()));
    }

    #[test]
    fn a_widget_keeps_the_arrow_keys_it_claims() {
        let mut s = Screen::new();
        let first = std::cell::Cell::new(egui::Id::NULL);
        let show = |ui: &mut egui::Ui, claimed: Option<Arrows>| {
            ui.horizontal(|ui| {
                let one = ui.button("One");
                if let Some(arrows) = claimed {
                    super::claim_arrows(&one, arrows);
                }
                first.set(one.id);
                let _ = ui.button("Two");
            });
        };
        for claimed in [Some(Arrows::LeftRight), Some(Arrows::UpDown), None] {
            s.frame(|ui| show(ui, claimed));
            s.ctx.memory_mut(|m| m.request_focus(first.get()));
            s.frame(|ui| show(ui, claimed));
            s.frame(|ui| show(ui, claimed));
            s.press(Key::ArrowRight);
            s.frame(|ui| show(ui, claimed));
            s.frame(|ui| show(ui, claimed));
            let kept = s.focused() == Some(first.get());
            assert_eq!(kept, claimed == Some(Arrows::LeftRight), "{claimed:?}");
        }
    }
}
