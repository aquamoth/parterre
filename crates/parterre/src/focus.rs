//! Keyboard focus as desktop GUIs show it (#387). egui moves the focus with Tab and Shift+Tab, in
//! the order widgets are laid out, and with the arrow keys to the nearest widget that way; it
//! shows no focus of its own beyond a text field's cursor. Here:
//!
//! - Which widget has the keyboard shows once the keyboard is used, as on Windows and in
//!   browsers; a click hides it again ([`Cues`]).
//! - parterre's widgets show it themselves ([`shows_own`], [`ring`]). Any other, such as egui's own
//!   buttons, gets a ring painted around it.
//! - In a dialog, a click on a button gives it the keyboard, as on Windows and Linux
//!   ([`take_on_click`]), but not on macOS, where a click never moves the keyboard to a button.
//!   A click on a list's row gives the list the keyboard everywhere ([`click_row`]). Outside
//!   dialogs a click leaves the keyboard where it was, as a toolbar's buttons do.
//! - A control of several parts, such as tabs or a list, is one stop, which takes the arrow
//!   keys it moves within itself by ([`claim`]).

use eframe::egui::{
    self, Context, Event, EventFilter, Id, Key, Painter, Rect, Response, Stroke, StrokeKind, Ui,
};

use crate::keys::Platform;
use crate::widgets;

/// The corners of the ring around a widget that doesn't show focus itself.
const CORNER: u8 = 4;

/// Notes whether the keyboard moved the focus last or a pointer was pressed, and rings the
/// widget with the keyboard that doesn't show it itself. Registered once, with
/// [`egui::Context::add_plugin`].
#[derive(Debug, Default)]
pub struct Cues {
    /// The keyboard was used to move around last, not the pointer.
    keyboard: bool,
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
                } if moves(*key) => self.keyboard = true,
                Event::PointerButton { pressed: true, .. } => self.keyboard = false,
                _ => {}
            }
        }
    }

    fn on_begin_pass(&mut self, ui: &mut Ui) {
        let key = shown_id(ui.ctx());
        ui.ctx().data_mut(|d| d.remove::<Id>(key));
    }

    fn on_end_pass(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let Some(id) = ctx.memory(|m| m.focused()) else {
            return;
        };
        let key = shown_id(&ctx);
        let own = ctx.data(|d| d.get_temp::<Id>(key)) == Some(id);
        if !self.keyboard || own || ctx.text_edit_focused() {
            return;
        }
        if let Some(response) = ctx.read_response(id) {
            let accent = widgets::tones(ui).accent;
            let painter = ctx.layer_painter(response.layer_id);
            // Inside the part of it in view, if it's cut off (scrolled partly out of a list).
            let (rect, painter) = if response.interact_rect == response.rect {
                (response.rect, painter)
            } else {
                let clip = response.interact_rect;
                (clip.shrink(2.0), painter.with_clip_rect(clip))
            };
            paint_ring(&painter, rect, CORNER, accent);
        }
    }
}

/// The keys that move around: they show where the keyboard is.
fn moves(key: Key) -> bool {
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
    )
}

/// Where the widget that shows its own focus this pass is noted, per window.
fn shown_id(ctx: &Context) -> Id {
    Id::new("focus-shown").with(ctx.viewport_id())
}

/// Whether the keyboard was used to move around last; never without [`Cues`].
fn keyboard(ctx: &Context) -> bool {
    ctx.with_plugin(|cues: &mut Cues| cues.keyboard)
        .unwrap_or(false)
}

/// `response`'s widget shows its own focus, so [`Cues`] paints no ring around it: whether to show
/// it now. It shows while the widget has the keyboard and the keyboard was used last.
pub fn shows_own(response: &Response) -> bool {
    shows_own_by_id(&response.ctx, response.id)
}

/// [`shows_own`] by the widget's id, for one that paints its parts before its own response is
/// made.
pub fn shows_own_by_id(ctx: &Context, id: Id) -> bool {
    if !ctx.memory(|m| m.has_focus(id)) {
        return false;
    }
    let key = shown_id(ctx);
    ctx.data_mut(|d| d.insert_temp(key, id));
    keyboard(ctx)
}

/// A ring around `response`'s widget while it shows it has the keyboard ([`shows_own`]),
/// following its corners, which are rounded by `corner`.
pub fn ring(ui: &Ui, response: &Response, corner: u8) {
    if shows_own(response) {
        paint_ring(
            ui.painter(),
            response.rect,
            corner,
            widgets::tones(ui).accent,
        );
    }
}

/// A ring against the outside of `rect`, whose corners are rounded by `corner`: with no gap, as
/// Windows 11 draws it.
fn paint_ring(painter: &Painter, rect: Rect, corner: u8, color: egui::Color32) {
    painter.rect_stroke(rect, corner, Stroke::new(2.0, color), StrokeKind::Outside);
}

/// While `response`'s widget has the keyboard, the arrow keys `filter` names act on it rather
/// than move the focus away. Call every frame.
pub fn claim(response: &Response, filter: EventFilter) {
    if !response.has_focus() {
        return;
    }
    let ctx = &response.ctx;
    // egui applies the filter from the frame after the one the widget got the keyboard in, so
    // until then an arrow key pressed at once would still move it away: that move is called off.
    let claimed = ctx.input(|i| {
        i.raw.events.iter().any(|e| match e {
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if !modifiers.any() => match key {
                Key::ArrowLeft | Key::ArrowRight => filter.horizontal_arrows,
                Key::ArrowUp | Key::ArrowDown => filter.vertical_arrows,
                _ => false,
            },
            _ => false,
        })
    });
    ctx.memory_mut(|m| {
        m.set_focus_lock_filter(response.id, filter);
        if claimed {
            m.move_focus(egui::FocusDirection::None);
        }
    });
}

/// Whether a click on a button gives it the keyboard: on Windows and Linux, not on macOS.
pub const BUTTONS_TAKE_FOCUS: bool = !matches!(Platform::CURRENT, Platform::Mac);

/// The window is a dialog, in which a click gives the keyboard to what it clicks: a button,
/// switch or other widget that is clicked and can have it ([`BUTTONS_TAKE_FOCUS`]), so that
/// Tab goes on from there and Space clicks it again; the control a part clicked belongs to
/// ([`click_part`]); a list ([`click_row`]). Call before the window's content, every frame.
pub fn take_on_click(ctx: &Context) {
    let key = on_click_id(ctx);
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(key, pass));
    if !BUTTONS_TAKE_FOCUS {
        return;
    }
    let Some(id) = ctx.interaction_snapshot(|s| s.clicked) else {
        return;
    };
    let takes = ctx.read_response(id).is_some_and(|r| {
        r.sense.senses_click() && r.sense.is_focusable() && !r.sense.senses_drag()
    });
    if takes {
        ctx.memory_mut(|m| m.request_focus(id));
    }
}

/// Where [`take_on_click`] notes the pass it was called in, per window.
fn on_click_id(ctx: &Context) -> Id {
    Id::new("focus-on-click").with(ctx.viewport_id())
}

/// A click on `part` of a control of buttons that is one stop, `stop`, such as a tab, gives the
/// control the keyboard as a click on a button would ([`take_on_click`]).
pub fn click_part(part: &Response, stop: Id) {
    if BUTTONS_TAKE_FOCUS {
        click_row(part, stop);
    }
}

/// A click on a row of `list` gives the list the keyboard, in a window where a click gives it
/// ([`take_on_click`]), on any platform. Elsewhere the keyboard stays where it was, and the
/// window's own keys with it.
pub fn click_row(row: &Response, list: Id) {
    let ctx = &row.ctx;
    let key = on_click_id(ctx);
    let pass = ctx.cumulative_pass_nr();
    if row.clicked() && ctx.data(|d| d.get_temp::<u64>(key)) == Some(pass) {
        ctx.memory_mut(|m| m.request_focus(list));
    }
}

#[cfg(test)]
pub mod tests {
    use eframe::egui::{self, Event, Key, Pos2, Rect, Shape, epaint::ClippedShape};

    use super::Cues;

    /// A window of its own, frame by frame, with [`Cues`] in it.
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
            Screen {
                ctx,
                time: 0.0,
                events: Vec::new(),
                shapes: Vec::new(),
            }
        }

        /// One frame of `ui`, with the events queued.
        pub fn frame(&mut self, mut add: impl FnMut(&mut egui::Ui)) {
            self.time += 1.0 / 60.0;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
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

        /// A click at `at`: the pointer moves there in one frame, and presses and lets go in
        /// the next, as a person's would.
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
            let accent = crate::widgets::tones_of(false).accent;
            fn rects(shape: &Shape, out: &mut Vec<egui::epaint::RectShape>) {
                match shape {
                    Shape::Rect(r) => out.push(r.clone()),
                    Shape::Vec(v) => v.iter().for_each(|s| rects(s, out)),
                    _ => {}
                }
            }
            let mut found = Vec::new();
            for clipped in &self.shapes {
                rects(&clipped.shape, &mut found);
            }
            found.iter().any(|r| {
                r.stroke.color == accent
                    && r.stroke.width >= 2.0
                    && r.rect.contains_rect(rect)
                    && r.rect.width() <= rect.width() + 8.0
            })
        }
    }

    /// Two of egui's buttons side by side, and where they were laid out.
    fn two_buttons(ui: &mut egui::Ui, rects: &mut [Rect; 2], take: bool) {
        if take {
            super::take_on_click(ui.ctx());
        }
        ui.horizontal(|ui| {
            rects[0] = ui.button("One").rect;
            rects[1] = ui.button("Two").rect;
        });
    }

    #[test]
    fn a_widget_with_the_keyboard_is_ringed_once_tab_is_pressed_until_a_click() {
        let mut s = Screen::new();
        let mut rects = [Rect::NOTHING; 2];
        s.frame(|ui| two_buttons(ui, &mut rects, false));
        assert!(!s.ringed(rects[0]));
        s.press(Key::Tab);
        s.frame(|ui| two_buttons(ui, &mut rects, false));
        assert!(s.focused().is_some());
        assert!(s.ringed(rects[0]), "the first button, after Tab");
        s.press(Key::Tab);
        s.frame(|ui| two_buttons(ui, &mut rects, false));
        assert!(s.ringed(rects[1]) && !s.ringed(rects[0]), "then the second");
        // A press anywhere hides it; the focus is where the click leaves it.
        s.click(rects[1].center());
        s.frame(|ui| two_buttons(ui, &mut rects, true));
        s.frame(|ui| two_buttons(ui, &mut rects, true));
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
        s.frame(|ui| two_buttons(ui, &mut rects, false));
        s.click(rects[1].center());
        s.frame(|ui| two_buttons(ui, &mut rects, false));
        assert_eq!(s.focused(), None, "as egui does");
        s.frame(|ui| two_buttons(ui, &mut rects, true));
        s.click(rects[1].center());
        s.frame(|ui| two_buttons(ui, &mut rects, true));
        if !super::BUTTONS_TAKE_FOCUS {
            assert_eq!(s.focused(), None, "on macOS, not even in a dialog");
            return;
        }
        assert!(s.focused().is_some(), "in a dialog");
        // Shift+Tab goes back from the button clicked.
        s.key(Key::Tab, egui::Modifiers::SHIFT);
        s.frame(|ui| two_buttons(ui, &mut rects, true));
        s.frame(|ui| two_buttons(ui, &mut rects, true));
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
                super::claim(
                    &one,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        ..Default::default()
                    },
                );
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
        let show = |ui: &mut egui::Ui, claim: bool| {
            ui.horizontal(|ui| {
                let one = ui.button("One");
                if claim {
                    super::claim(
                        &one,
                        egui::EventFilter {
                            horizontal_arrows: true,
                            ..Default::default()
                        },
                    );
                }
                first.set(one.id);
                let _ = ui.button("Two");
            });
        };
        for claim in [true, false] {
            s.frame(|ui| show(ui, claim));
            s.ctx.memory_mut(|m| m.request_focus(first.get()));
            s.frame(|ui| show(ui, claim));
            s.frame(|ui| show(ui, claim));
            s.press(Key::ArrowRight);
            s.frame(|ui| show(ui, claim));
            s.frame(|ui| show(ui, claim));
            assert_eq!(s.focused() == Some(first.get()), claim, "claimed: {claim}");
        }
    }
}
