//! Shared native dialogs: windows of their own, theme, keyboard-safe action rows and command
//! previews. Tools provide their own fields, validation and Git decisions.
//!
//! A dialog is a window beside parterre's others, moved by its title bar. A modeless one leaves
//! them alone. A modal one locks them all, whichever window opened it, until it is answered:
//! [`ModalLock`] drops their input, so a click or a close there brings the dialog forward.
//!
//! A dialog is as big as its content. A resizable one opens that big, up to most of the screen,
//! and then the user sizes it: one part of it, such as a list or the [`fields`], takes what the
//! rest leaves (see [`growing`]). Its width is remembered; its height follows its content until
//! the user sizes it otherwise.

use std::sync::Arc;

use eframe::egui::{self, Align, Color32, Id, Layout, Pos2, RichText, Ui, Vec2, ViewportId, vec2};
use parterre_core::{Commit, glyphs::Glyph};

use crate::{menu, widgets};

/// Space around a dialog's content.
pub const MARGIN: f32 = 20.0;

/// The height a dialog's content is measured in: as tall as it wants.
const UNBOUNDED: f32 = 100_000.0;

/// How long, in seconds, a resizable dialog keeps the size it opened in, against a compositor
/// handing it back another.
const KEEP_OPENING_SIZE: f64 = 1.0;

/// The narrowest a resizable dialog's content gets.
const MIN_WIDTH: f32 = 420.0;

/// What dialog windows share with parterre's others: the icon and the title bar's theme.
#[derive(Clone, Debug)]
pub struct Look {
    pub icon: Arc<egui::IconData>,
    pub theme: Option<egui::SystemTheme>,
}

fn look_id() -> Id {
    Id::new("dialog-look")
}

/// Set every frame, before any dialog is shown.
pub fn set_look(ctx: &egui::Context, look: Look) {
    ctx.data_mut(|d| d.insert_temp(look_id(), look));
}

/// The least height of a resizable dialog's [`fields`]: they scroll below it.
const MIN_FIELDS: f32 = 70.0;

/// One dialog window, kept from frame to frame while it is shown.
#[derive(Clone, Copy, Debug, Default)]
struct Window {
    /// Measured in the frame before; `None` until the content was laid out once. A resizable
    /// one's is the size its window was last told to be.
    size: Option<Vec2>,
    /// Where it opened, over the window that opened it. Left alone after: the user moves it.
    position: Option<Pos2>,
    theme: Option<egui::SystemTheme>,
    /// The size its window was last told to be and keep; `None` before it exists.
    hinted: Option<Vec2>,
    /// When its size was last asked for again, after the window didn't take it.
    resized: f64,
    /// The main window's frame it was last shown in.
    frame: u64,
    /// When a resizable one's window was told its size: the user sizes it from a moment later.
    opened: f64,
    /// A resizable one's size as the user left it, once they could size it.
    user: Option<Vec2>,
    /// A resizable one is as tall as its content wants (or most of the screen), as it opened,
    /// until the user sizes it otherwise.
    fit: bool,
    /// A resizable one's height as tall as its content wants, in the frame before.
    natural: Option<f32>,
    /// The least size a resizable one's window was told.
    least: Option<Vec2>,
    /// What a resizable one's width is remembered by.
    remembered: Option<Id>,
}

/// A resizable dialog's size as laid out in its window: the least the window can be, and the
/// height the content wants. `settled` once laid out with what the frame before learned.
#[derive(Clone, Copy, Debug)]
struct Fit {
    least: Vec2,
    natural: f32,
    settled: bool,
}

/// The dialog whose content is being laid out, for [`growing`] and [`fields`].
#[derive(Clone, Copy, Debug)]
struct Current {
    id: Id,
    resizable: bool,
    /// Its growing part is being laid out.
    growing: bool,
}

fn current_id() -> Id {
    Id::new("dialog-current")
}

/// What a resizable dialog's growing part learned, kept from frame to frame.
#[derive(Clone, Copy, Debug, Default)]
struct Growth {
    /// The height of the content after it.
    rest: Option<f32>,
    /// Its height as tall as it wants.
    natural: Option<f32>,
}

/// The growing part as laid out this frame.
#[derive(Clone, Copy, Debug, Default)]
struct Part {
    /// Where it ends.
    end: f32,
    height: f32,
    min: f32,
    natural: f32,
}

#[derive(Debug)]
pub struct Dialog<'a> {
    id: Id,
    title: &'a str,
    width: f32,
    icon: Option<Glyph>,
    danger: bool,
    modal: bool,
    /// Closes only through its own buttons: no close button on its window.
    undismissable: bool,
    opener: ViewportId,
    raise: bool,
    resizable: bool,
    remember: Option<Id>,
    /// What the usage statistics call it, if they count it (#264).
    screen: Option<crate::usage::Screen>,
}

/// A dialog's answer for this frame. Closing the window (or Esc in [`actions`]) means cancel.
#[derive(Debug)]
pub struct Shown<R> {
    pub inner: R,
    close: bool,
}

impl<R> Shown<R> {
    /// The window was closed from its title bar.
    pub fn should_close(&self) -> bool {
        self.close
    }
}

impl<'a> Dialog<'a> {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, title: &'a str) -> Self {
        Self {
            id: Id::new(id),
            title,
            width: 470.0,
            icon: None,
            danger: false,
            modal: false,
            undismissable: false,
            opener: ViewportId::ROOT,
            raise: false,
            resizable: false,
            remember: None,
            screen: None,
        }
    }

    /// Counted in the usage statistics as `screen` each time it opens (#264).
    pub fn screen(mut self, screen: crate::usage::Screen) -> Self {
        self.screen = Some(screen);
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
    pub fn icon(mut self, icon: Glyph, danger: bool) -> Self {
        self.icon = Some(icon);
        self.danger = danger;
        self
    }
    /// Locks parterre's other windows while it is shown.
    pub fn modal(mut self) -> Self {
        self.modal = true;
        self
    }
    /// Closes only through its own buttons: its window has no close button where the platform
    /// lets parterre leave it out (winit 0.30: Windows and macOS), and the caller ignores
    /// [`Shown::should_close`]. Esc does nothing unless the caller makes it.
    pub fn undismissable(mut self) -> Self {
        self.undismissable = true;
        self
    }
    /// The window it opens over, where the platform lets parterre place windows (not Wayland).
    pub fn opener(mut self, opener: ViewportId) -> Self {
        self.opener = opener;
        self
    }
    /// The user can size it, from at least `width`'s narrower self, with one part of its content
    /// taking up the difference (see [`growing`]). It opens as wide as the user left it last
    /// time, or `width`, and as tall as its content, up to most of the screen.
    pub fn resizable(mut self) -> Self {
        self.resizable = true;
        self
    }
    /// A resizable one's width is remembered by `key` rather than by its id, e.g. one width
    /// with a pane and another without. A new key while it is open sizes it anew.
    pub fn remember_as(mut self, key: impl std::hash::Hash + std::fmt::Debug) -> Self {
        self.remember = Some(Id::new(key));
        self
    }
    /// Brings it forward, e.g. when asked for again while it is open.
    pub fn raise(mut self, raise: bool) -> Self {
        self.raise = raise;
        self
    }

    fn viewport(&self) -> ViewportId {
        ViewportId::from_hash_of(("dialog", self.id))
    }

    /// Where a resizable one's window width is kept, across runs.
    fn width_id(&self) -> Id {
        Id::new("dialog-width").with(self.remember.unwrap_or(self.id))
    }

    /// Shows the dialog in its own window, sized to its content. Call every frame while it is
    /// open; the window goes when the calls stop. `content` may run more than once a frame: the
    /// first time, it is measured before the window opens.
    pub fn show<R>(&self, ctx: &egui::Context, mut content: impl FnMut(&mut Ui) -> R) -> Shown<R> {
        if let Some(screen) = self.screen {
            crate::usage::screen(ctx, self.id, screen);
        }
        let key = self.id.with("window");
        let frame = ctx.cumulative_frame_nr_for(ViewportId::ROOT);
        let mut window = ctx
            .data(|d| d.get_temp::<Window>(key))
            // Not shown in the frame before: its window is gone, a new one opens.
            .filter(|w| w.frame + 1 >= frame)
            .unwrap_or_default();
        window.frame = frame;
        let look: Option<Look> = ctx.data(|d| d.get_temp(look_id()));
        let opener = ctx.input(|i| i.raw.viewports.get(&self.opener).cloned());
        let monitor = opener
            .as_ref()
            .and_then(|o| o.monitor_size)
            .unwrap_or(vec2(1280.0, 800.0));
        let remembered: Option<f32> = if self.resizable {
            ctx.data_mut(|d| d.get_persisted(self.width_id()))
        } else {
            None
        };
        let width = remembered
            .map_or(self.width, |w| w - 2.0 * MARGIN)
            .min(monitor.x - 72.0)
            .max(200.0);
        // Remembered by another key now: sized anew, as it opens.
        let rekeyed = self.resizable && window.remembered.is_some_and(|k| k != self.width_id());
        if rekeyed || window.size.is_none() {
            window.size = None;
            window.user = None;
            window.least = None;
            window.fit = true;
        }
        window.remembered = Some(self.width_id());
        let mut style = (*ctx.global_style()).clone();
        menu::popover_style(&mut style);
        // A new window opens in its size and place, rather than moving and growing in view.
        let size = *window.size.get_or_insert_with(|| {
            let size = self.measure(ctx, opener.as_ref(), width, &style, &mut content);
            if self.resizable {
                vec2(size.x, size.y.min((monitor.y * 0.85).round()))
            } else {
                size
            }
        });
        if window.position.is_none() {
            window.position = opener
                .as_ref()
                .and_then(|o| o.outer_rect)
                .map(|r| (r.center() - size / 2.0).max(r.min));
        }
        let mut builder = egui::ViewportBuilder::default()
            .with_title(self.title)
            .with_app_id(crate::settings::APP_ID)
            .with_inner_size(size)
            .with_minimize_button(false)
            .with_maximize_button(false)
            .with_window_type(egui::X11WindowType::Dialog);
        if self.undismissable {
            builder = builder.with_close_button(false);
        }
        // Same size always. On Wayland that is told to the window once it exists (below):
        // told here, winit would set the hints before its title bar exists and leave the bar
        // out of them; the compositor then holds the window to the hints, with the bar outside
        // its frame (above the screen, at the top) and the content cut short by its height.
        // Embedded in a screenshot, a window that size is cut short by its title bar.
        let min = vec2(MIN_WIDTH.min(width + 2.0 * MARGIN), 200.0);
        if self.resizable {
            // Its content raises the height as it learns what it needs (see `growing`). On
            // Wayland, the least size too is told once the window exists, as below.
            // Embedded in a screenshot, it is the size it was told and its window fits it.
            builder = builder.with_resizable(!ctx.embed_viewports());
            if !wayland() {
                builder = builder.with_min_inner_size(min);
            }
        } else if !wayland() && !ctx.embed_viewports() {
            builder = builder
                .with_min_inner_size(size)
                .with_max_inner_size(size)
                .with_resizable(false);
        }
        if let Some(look) = &look {
            builder = builder.with_icon(look.icon.clone());
        }
        if let Some(position) = window.position {
            builder = builder.with_position(position);
        }
        let viewport = self.viewport();
        if self.raise {
            ctx.request_repaint();
            if !ctx.embed_viewports() {
                ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Focus);
            }
        }
        let (inner, close, measured) =
            ctx.show_viewport_immediate(viewport, builder, |ui, class| {
                let embedded = class == egui::ViewportClass::EmbeddedWindow;
                let mut close = false;
                if !embedded {
                    let theme = look.as_ref().and_then(|l| l.theme);
                    if window.theme != theme {
                        window.theme = theme;
                        if let Some(theme) = theme {
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::SetTheme(theme));
                        }
                    }
                    close = ui.input(|i| i.viewport().close_requested());
                    // Once the window exists (its title bar with it): its size, and to keep
                    // it. Asked for again whenever the content changes size.
                    let now = ui.input(|i| i.time);
                    if rekeyed {
                        window.opened = now;
                    }
                    if self.resizable && window.hinted.is_none() {
                        if wayland() {
                            for command in [
                                egui::ViewportCommand::InnerSize(size),
                                egui::ViewportCommand::MinInnerSize(min),
                            ] {
                                ui.ctx().send_viewport_cmd(command);
                            }
                        }
                        window.hinted = Some(size);
                        window.resized = now;
                        window.opened = now;
                    } else if !self.resizable && wayland() && window.hinted != Some(size) {
                        window.hinted = Some(size);
                        window.resized = now;
                        for command in [
                            egui::ViewportCommand::InnerSize(size),
                            egui::ViewportCommand::MinInnerSize(size),
                            egui::ViewportCommand::MaxInnerSize(size),
                        ] {
                            ui.ctx().send_viewport_cmd(command);
                        }
                    }
                    // A Wayland compositor may hand a window back the size it last knew (on
                    // focus, or from a configure sent before its title bar was in its frame),
                    // and winit takes it. Ask again until the window has it. A resizable one is
                    // handed back a size without its title bar just after it opens (GNOME):
                    // its size is kept for a moment, and after that it is the user's.
                    let off = ui.ctx().content_rect().size() - size;
                    let keep = !self.resizable || now - window.opened < KEEP_OPENING_SIZE;
                    if self.resizable && keep {
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(50));
                    }
                    if off.abs().max_elem() > 1.0 && keep {
                        if now - window.resized > 0.03 {
                            window.resized = now;
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                        }
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(40));
                    }
                    if self.modal {
                        if ModalLock::shown(ui.ctx(), viewport) == Some(true) {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
                        }
                        // Answered, it is gone in the next frame, which unlocks the others: run
                        // that frame now, or the next input there is still dropped.
                        if close || ui.input(|i| !i.events.is_empty()) {
                            ui.ctx().request_repaint_of(ViewportId::ROOT);
                        }
                    }
                }
                // A resizable one fills the window the user sized.
                let width = if self.resizable && !embedded {
                    ui.ctx().content_rect().width() - 2.0 * MARGIN
                } else {
                    width
                };
                let fill = match (self.resizable, embedded) {
                    (false, _) => None,
                    (true, false) => Some(ui.max_rect().height() - 2.0 * MARGIN),
                    (true, true) => Some(size.y - 2.0 * MARGIN),
                };
                let (inner, measured, fit) = self.body(ui, width, fill, &style, &mut content);
                let cap = (monitor.y * 0.85).round();
                match fit {
                    Some(fit) if !embedded => self.follow(ui.ctx(), &mut window, fit, cap),
                    // Embedded, no one sizes it: it is as tall as its content wants.
                    Some(fit) if fit.settled => {
                        let wanted = fit.natural.min(cap).max(fit.least.y).round();
                        if (size.y - wanted).abs() > 1.0 {
                            window.size = Some(vec2(size.x, wanted));
                            ui.ctx().request_repaint();
                        }
                    }
                    _ => {}
                }
                (inner, close, measured)
            });
        if !self.resizable {
            window.size = Some(measured);
        }
        ctx.data_mut(|d| d.insert_temp(key, window));
        Shown { inner, close }
    }

    /// A resizable one's window, once the user can size it: the size they give it is theirs,
    /// and its width is remembered. Its height follows the content while it fits the content,
    /// and grows when the content does (more shown) and doesn't fit.
    fn follow(&self, ctx: &egui::Context, window: &mut Window, fit: Fit, cap: f32) {
        if fit.settled && window.least != Some(fit.least) {
            window.least = Some(fit.least);
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(fit.least));
        }
        let now = ctx.input(|i| i.time);
        let natural = window.natural.replace(fit.natural);
        let actual = ctx.content_rect().size();
        let told = window.size.unwrap_or(actual);
        let wanted = fit.natural.min(cap).max(fit.least.y).round();
        // Just told a size, the window may not have it yet: that is no user's.
        let keep = now - window.opened < KEEP_OPENING_SIZE;
        if keep {
            window.user = None;
        } else {
            let before = window.user.unwrap_or(told);
            if (actual - before).abs().max_elem() > 1.0 {
                // Sized by the user.
                if (actual.x - before.x).abs() > 1.0 {
                    ctx.data_mut(|d| d.insert_persisted(self.width_id(), actual.x));
                }
                window.user = Some(actual);
                window.fit = (actual.y - wanted).abs() <= 1.0;
                return;
            }
        }
        let size = if keep { told } else { actual };
        let grew = natural.is_some_and(|n| fit.natural > n + 0.5) && size.y < wanted;
        if fit.settled && (window.fit || grew) && (size.y - wanted).abs() > 1.0 {
            // Told through the builder's size, in the next frame.
            window.size = Some(vec2(size.x, wanted));
            window.user = None;
            window.opened = now;
            window.fit = true;
        }
    }

    /// The dialog's size, laid out before its window exists: in a context of its own with no
    /// input, so nothing in it is clicked or typed into. It shares `ctx`'s stored data, such as
    /// whether the Git command is unfolded.
    fn measure<R>(
        &self,
        ctx: &egui::Context,
        opener: Option<&egui::ViewportInfo>,
        width: f32,
        style: &egui::Style,
        content: &mut impl FnMut(&mut Ui) -> R,
    ) -> Vec2 {
        let sizer_id = Id::new("dialog-sizer");
        let sizer = ctx.data_mut(|d| {
            d.get_temp_mut_or_insert_with(sizer_id, egui::Context::default)
                .clone()
        });
        let mut data = ctx.memory(|m| m.data.clone());
        // Not itself, or it would keep itself alive.
        data.remove::<egui::Context>(sizer_id);
        sizer.memory_mut(|m| m.data = data);
        sizer.set_zoom_factor(ctx.zoom_factor());
        let info = egui::ViewportInfo {
            monitor_size: opener.and_then(|o| o.monitor_size),
            native_pixels_per_point: opener.and_then(|o| o.native_pixels_per_point),
            ..Default::default()
        };
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                vec2(width + 2.0 * MARGIN, UNBOUNDED),
            )),
            viewports: std::iter::once((ViewportId::ROOT, info)).collect(),
            ..Default::default()
        };
        let mut size = Vec2::ZERO;
        let mut output = sizer.run_ui(input, |ui| {
            size = self.body(ui, width, None, style, content).1;
        });
        // Nothing is painted.
        output.textures_delta.clear();
        // What follows a resizable one's growing part, for its first frame in the window.
        let key = self.id.with("growth");
        if let Some(growth) = sizer.data(|d| d.get_temp::<Growth>(key)) {
            ctx.data_mut(|d| d.insert_temp(key, growth));
        }
        size
    }

    /// The dialog's content, as tall as it wants (not the window: a window still too short
    /// would squeeze the fields, and be measured too short again), or a resizable one `fill`
    /// high. Returns its answer, the window size it needs, and a resizable one's fit in its
    /// window.
    fn body<R>(
        &self,
        ui: &mut Ui,
        width: f32,
        fill: Option<f32>,
        style: &egui::Style,
        content: &mut impl FnMut(&mut Ui) -> R,
    ) -> (R, Vec2, Option<Fit>) {
        let frame = egui::Frame::new()
            .fill(style.visuals.window_fill)
            .inner_margin(egui::Margin::same(MARGIN as i8));
        egui::CentralPanel::default()
            .frame(frame)
            .show(ui, |ui| {
                ui.set_style(style.clone());
                // A resizable one fills its window, the `fill` high.
                let height = fill.unwrap_or(UNBOUNDED);
                let room = egui::Rect::from_min_size(ui.max_rect().min, vec2(width, height));
                let builder = egui::UiBuilder::new()
                    .max_rect(room)
                    .layout(Layout::top_down(Align::Min));
                let current = Current {
                    id: self.id,
                    resizable: self.resizable,
                    growing: false,
                };
                ui.data_mut(|d| {
                    d.insert_temp(current_id(), current);
                    d.remove_temp::<Part>(self.id.with("part"));
                });
                let shown = ui.scope_builder(builder, |ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing.y = 10.0;
                    self.heading(ui);
                    content(ui)
                });
                let content = shown.response.rect.height();
                let size = (vec2(width, content) + Vec2::splat(2.0 * MARGIN)).round();
                let fit = self
                    .resizable
                    .then(|| self.fit(ui, &shown.response, width))
                    .filter(|_| fill.is_some());
                (shown.inner, size, fit)
            })
            .inner
    }

    /// A resizable one's fit in its window, from its content as laid out, with its growing
    /// part's height, if it has one, learned for the next frame.
    fn fit(&self, ui: &Ui, content: &egui::Response, width: f32) -> Fit {
        let part: Option<Part> = ui.data_mut(|d| d.remove_temp(self.id.with("part")));
        let height = content.rect.height();
        let (least, natural, settled) = match part {
            Some(part) => {
                let rest = (content.rect.bottom() - part.end).max(0.0);
                let key = self.id.with("growth");
                let mut growth: Growth = ui.data(|d| d.get_temp(key)).unwrap_or_default();
                let settled = growth.rest.is_some_and(|r| (r - rest).abs() <= 0.5);
                growth.rest = Some(rest);
                growth.natural = Some(part.natural);
                ui.data_mut(|d| d.insert_temp(key, growth));
                if !settled {
                    // Laid out with the old figure: again, with this one.
                    ui.ctx().request_repaint();
                }
                (
                    height - (part.height - part.min),
                    height - (part.height - part.natural),
                    settled,
                )
            }
            None => (height, height, true),
        };
        Fit {
            least: vec2(
                MIN_WIDTH.min(width + 2.0 * MARGIN),
                (least + 2.0 * MARGIN).round(),
            ),
            natural: (natural + 2.0 * MARGIN).round(),
            settled,
        }
    }

    /// A title with an icon is a question the dialog asks, shown big inside too.
    fn heading(&self, ui: &mut Ui) {
        let Some(glyph) = self.icon else { return };
        let tones = widgets::tones(ui);
        let fg = if self.danger {
            ui.visuals().error_fg_color
        } else {
            tones.on_fg
        };
        ui.horizontal(|ui| {
            let (badge, _) = ui.allocate_exact_size(vec2(32.0, 32.0), egui::Sense::hover());
            ui.painter().circle_filled(
                badge.center(),
                16.0,
                if self.danger {
                    fg.gamma_multiply(0.12)
                } else {
                    tones.on_bg
                },
            );
            widgets::paint_glyph(
                ui.painter(),
                egui::Rect::from_center_size(badge.center(), vec2(18.0, 18.0)),
                glyph,
                fg,
            );
            ui.add(egui::Label::new(RichText::new(self.title).size(16.0).strong()).wrap());
        });
    }
}

/// Whether the windows are Wayland ones: winit picks Wayland over X11 when this is set.
fn wayland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Locks parterre's windows while a modal dialog is shown: their input is dropped (a click or a
/// close there brings the dialog forward instead). Registered once, with
/// [`egui::Context::add_plugin`]; the main window's [`eframe::App::raw_input_hook`] calls
/// [`ModalLock::filter`] too, as eframe decides to close it before plugins see its input.
#[derive(Debug, Default)]
pub struct ModalLock {
    /// The modal dialog's window, shown in the main window's last frame.
    modal: Option<ViewportId>,
    /// Shown in the main window's current frame.
    seen: Option<ViewportId>,
    /// Someone clicked or closed a locked window.
    raise: bool,
}

impl ModalLock {
    /// Whether a modal dialog locks the other windows now.
    pub fn locked(ctx: &egui::Context) -> bool {
        ctx.with_plugin(|lock: &mut ModalLock| lock.modal.is_some())
            .unwrap_or(false)
    }

    /// Notes the modal dialog shown this frame. Returns whether to bring it forward, or `None`
    /// without the plugin (in tests).
    fn shown(ctx: &egui::Context, viewport: ViewportId) -> Option<bool> {
        ctx.with_plugin(|lock: &mut ModalLock| {
            lock.seen = Some(viewport);
            std::mem::take(&mut lock.raise)
        })
    }

    /// Drops a locked window's input. Its key and button releases and modifier changes stay, so
    /// nothing is held down when it is unlocked.
    pub fn filter(ctx: &egui::Context, input: &mut egui::RawInput) {
        ctx.with_plugin(|lock: &mut ModalLock| lock.lock(ctx, input));
    }

    fn lock(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        let Some(modal) = self.modal else { return };
        if input.viewport_id == modal || ctx.embed_viewports() {
            return;
        }
        let mut raise = false;
        if let Some(info) = input.viewports.get_mut(&input.viewport_id) {
            raise |= info.close_requested();
            info.events.retain(|e| *e != egui::ViewportEvent::Close);
        }
        input.events = std::mem::take(&mut input.events)
            .into_iter()
            .filter_map(|e| match e {
                egui::Event::PointerButton { pressed: true, .. } => {
                    raise = true;
                    None
                }
                egui::Event::PointerMoved(_) => Some(egui::Event::PointerGone),
                e @ (egui::Event::Key { pressed: false, .. }
                | egui::Event::PointerButton { pressed: false, .. }
                | egui::Event::PointerGone
                | egui::Event::ModifiersChanged(_)
                | egui::Event::WindowFocused(_)
                | egui::Event::Screenshot { .. }) => Some(e),
                _ => None,
            })
            .collect();
        input.hovered_files.clear();
        input.dropped_files.clear();
        if raise {
            self.raise = true;
            ctx.request_repaint_of(modal);
        }
    }
}

impl egui::Plugin for ModalLock {
    fn debug_name(&self) -> &'static str {
        "parterre-modal-lock"
    }

    fn input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        self.lock(ctx, input);
    }

    /// Other windows are drawn inside the main window's frame: at its end, it's known whether
    /// a modal dialog is still shown.
    fn on_end_pass(&mut self, ui: &mut Ui) {
        if ui.ctx().viewport_id() == ViewportId::ROOT {
            self.modal = self.seen.take();
        }
    }
}

/// A resizable dialog's growing part, such as a list, laid out by `part`. In its window, it is
/// given the height to fill: what the window has left once the rest of the content (as laid out
/// the frame before) is in, but at least `min`, or all of its own height when that is less.
/// Otherwise, while the dialog is measured or in a dialog of fixed size, it is given `None`, to
/// be as tall as it wants. `part` returns that height of its own either way. One per dialog.
pub fn growing<R>(ui: &mut Ui, min: f32, part: impl FnOnce(&mut Ui, Option<f32>) -> (R, f32)) -> R {
    let current: Option<Current> = ui.data(|d| d.get_temp(current_id()));
    let room = ui.max_rect().bottom() - ui.cursor().top();
    // Not inside another, nor laid out unseen for a widget's size.
    let grows = |c: &Current| c.resizable && !c.growing;
    let Some(current) = current.filter(grows).filter(|_| !ui.is_sizing_pass()) else {
        return part(ui, None).0;
    };
    let inside = Current {
        growing: true,
        ..current
    };
    ui.data_mut(|d| d.insert_temp(current_id(), inside));
    let growth: Growth = ui
        .data(|d| d.get_temp(current.id.with("growth")))
        .unwrap_or_default();
    let min = growth.natural.map_or(min, |n| min.min(n));
    // Measured, it is as tall as it wants, and what follows it is learned for the window.
    let height = (room < UNBOUNDED / 2.0).then(|| (room - growth.rest.unwrap_or(0.0)).max(min));
    let (inner, natural) = part(ui, height);
    ui.data_mut(|d| d.insert_temp(current_id(), current));
    let laid = Part {
        // Its end: the cursor is past the space after it.
        end: ui.cursor().top() - ui.spacing().item_spacing.y,
        height: height.unwrap_or(natural),
        min,
        natural,
    };
    ui.data_mut(|d| d.insert_temp(current.id.with("part"), laid));
    inner
}

/// The dialog's fields, scrolling once the window would be taller than most of the screen.
/// Callers keep the actions outside it, always in view. In a resizable dialog they are its
/// growing part, unless it has another.
pub fn fields<R>(ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> R {
    let current: Option<Current> = ui.data(|d| d.get_temp(current_id()));
    if current.is_some_and(|c| c.resizable) {
        return growing(ui, MIN_FIELDS, |ui, height| {
            ui.scope(|ui| {
                let inside = visible_scroll_bars(ui);
                let area = egui::ScrollArea::vertical().id_salt("dialog-fields");
                let area = match height {
                    Some(h) => area
                        .min_scrolled_height(h)
                        .max_height(h)
                        .auto_shrink([false, false]),
                    // No taller than the window can open (most of the screen); embedded in a
                    // screenshot, it isn't filled.
                    None => {
                        let used = ui.cursor().top() - ui.min_rect().top();
                        let screen = ui
                            .input(|i| i.viewport().monitor_size)
                            .map_or(800.0, |s| s.y);
                        area.max_height((screen * 0.85 - used).max(MIN_FIELDS))
                            .auto_shrink([false, true])
                    }
                };
                let shown = area.show(ui, |ui| {
                    *ui.visuals_mut() = inside;
                    content(ui)
                });
                (shown.inner, shown.content_size.y)
            })
            .inner
        });
    }
    let used = ui.cursor().top() - ui.min_rect().top();
    // The window grows with its content up to most of the screen; the fields scroll after that.
    let screen = ui
        .input(|i| i.viewport().monitor_size)
        .map_or(800.0, |s| s.y);
    let remaining = (screen * 0.85).min(630.0) - 2.0 * MARGIN - used - 165.0;
    ui.scope(|ui| {
        let inside = visible_scroll_bars(ui);
        egui::ScrollArea::vertical()
            .id_salt("dialog-fields")
            .max_height(remaining.clamp(70.0, 560.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                *ui.visuals_mut() = inside;
                content(ui)
            })
            .inner
    })
    .inner
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Open,
    Primary,
    Cancel,
}

/// Destructive dialogs always treat Enter as Cancel. Only an explicit click accepts loss.
pub fn actions(
    ui: &mut Ui,
    label: &str,
    enabled: bool,
    destructive: bool,
    focus_cancel: bool,
) -> Answer {
    let mut answer = Answer::Open;
    let size = vec2(ui.available_width(), 34.0);
    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
        if !label.is_empty() {
            let ok = ui
                .add_enabled_ui(enabled, |ui| {
                    if destructive {
                        let color = ui.visuals().error_fg_color;
                        ui.add(
                            egui::Button::new(RichText::new(label).color(Color32::WHITE))
                                .fill(color)
                                .corner_radius(8)
                                .min_size(vec2(90.0, 30.0)),
                        )
                    } else {
                        widgets::primary_button(ui, label, 90.0)
                    }
                })
                .inner;
            if ok.clicked() {
                answer = Answer::Primary;
            }
        }
        let cancel = widgets::text_button(ui, if label.is_empty() { "Close" } else { "Cancel" });
        if focus_cancel {
            cancel.request_focus();
        }
        if cancel.has_focus() {
            ui.painter().rect_stroke(
                cancel.rect.expand(2.0),
                7,
                egui::Stroke::new(2.0, widgets::tones(ui).accent),
                egui::StrokeKind::Outside,
            );
        }
        if cancel.clicked()
            || ui.input(|i| {
                i.key_pressed(egui::Key::Escape) || destructive && i.key_pressed(egui::Key::Enter)
            })
        {
            answer = Answer::Cancel;
        }
    });
    answer
}

pub fn command_box(ui: &mut Ui, commands: &[String]) {
    ui.separator();
    let id = egui::Id::new("git-command-expanded");
    let mut shown = ui.data_mut(|d| *d.get_persisted_mut_or(id, false));
    // The heading as tall as its text, close to the box.
    let row = ui.spacing().interact_size.y;
    ui.spacing_mut().interact_size.y = 16.0;
    let heading = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(12.0), egui::Sense::hover());
            let glyph = if shown {
                parterre_core::glyphs::CHEVRON_DOWN
            } else {
                parterre_core::glyphs::CHEVRON_RIGHT
            };
            let weak = ui.visuals().weak_text_color();
            widgets::paint_glyph(ui.painter(), rect, glyph, weak);
            ui.label(RichText::new("Git command").small().weak());
        })
        .response;
    let head = ui
        .interact(
            heading.rect,
            ui.id().with("git-command-heading"),
            egui::Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.spacing_mut().interact_size.y = row;
    if head.clicked() {
        shown = !shown;
        ui.data_mut(|d| d.insert_persisted(id, shown));
    }
    if !shown {
        return;
    }
    ui.add_space(-4.0);
    let t = widgets::tones(ui);
    let text = commands.join("\n");
    egui::Frame::new()
        .fill(t.seg_bg)
        .corner_radius(8)
        .inner_margin(egui::Margin {
            left: 12,
            right: 4,
            top: 4,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            // Two lines high at least, so the dialog keeps its shape; empty while there's no
            // command.
            // A line as the commands are drawn: one row of their font.
            let line = ui
                .painter()
                .layout_no_wrap("git".into(), egui::FontId::monospace(11.5), Color32::WHITE)
                .size()
                .y;
            ui.set_min_height(2.0 * line + 4.0);
            if commands.is_empty() {
                return;
            }
            ui.horizontal_top(|ui| {
                let w = ui.available_width() - 28.0;
                // Its own column: the row it's in would lay the lines side by side.
                // Three lines at most; more scroll, with a bar that shows it.
                ui.vertical(|ui| {
                    ui.set_width(w);
                    let inside = visible_scroll_bars(ui);
                    // egui keeps a scrolling area 64 points high at least: three lines are less.
                    let three = 3.0 * line + 4.0;
                    egui::ScrollArea::vertical()
                        .id_salt("command-lines")
                        .max_height(three)
                        .min_scrolled_height(three)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            *ui.visuals_mut() = inside;
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.add_space(4.0);
                            wrapped_commands(ui, commands, w - 16.0);
                        });
                });
                // A check mark for a moment after a click.
                let copied_id = egui::Id::new("copied-command").with(&text);
                let now = ui.input(|i| i.time);
                let at: Option<f64> = ui.data(|d| d.get_temp(copied_id));
                let copied = at.is_some_and(|at| now - at < 1.5);
                if copied {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(300));
                }
                let done = Color32::from_rgb(0x2e, 0xa0, 0x43);
                if widgets::copy_button(ui, copied, done)
                    .on_hover_text("Copy, to run in a terminal")
                    .clicked()
                {
                    ui.ctx().copy_text(text.clone());
                    ui.data_mut(|d| d.insert_temp(copied_id, now));
                }
            });
        });
}

/// Scroll bars that show there's more: a solid bar with a gray track the whole height of the
/// area and a darker handle, instead of egui's track in the dialog's own colour (white on
/// white) and a handle only on hover. Returns the visuals to put back inside the area, so its
/// content looks as it would outside.
fn visible_scroll_bars(ui: &mut Ui) -> egui::Visuals {
    let inside = ui.visuals().clone();
    let t = widgets::tones(ui);
    ui.spacing_mut().scroll = egui::style::ScrollStyle {
        bar_width: 8.0,
        ..egui::style::ScrollStyle::solid()
    };
    let weak = inside.weak_text_color();
    let v = ui.visuals_mut();
    v.extreme_bg_color = t.seg_bg;
    v.widgets.inactive.bg_fill = weak.gamma_multiply(0.6);
    v.widgets.hovered.bg_fill = weak;
    v.widgets.active.bg_fill = inside.text_color();
    inside
}

/// A return arrow, where a command is broken.
const RETURN: Glyph = &[
    parterre_core::glyphs::Part::Path("M19 5v9H6"),
    parterre_core::glyphs::Part::Path("M10 10l-4 4 4 4"),
];

/// Each command broken where the room ends, not at its spaces, with a return arrow at each
/// break; space between the commands, so each can be told apart.
fn wrapped_commands(ui: &mut Ui, commands: &[String], width: f32) {
    const MARK: f32 = 13.0;
    let font = egui::FontId::monospace(11.5);
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let per_line = (((width - MARK) / char_w).floor() as usize).max(10);
    let color = ui.visuals().text_color();
    let weak = ui.visuals().weak_text_color();
    for (k, command) in commands.iter().enumerate() {
        if k > 0 {
            ui.add_space(10.0);
        }
        // One block of text, a row per line, so the lines sit exactly a font's height apart
        // (three of them fill the box).
        let chars: Vec<char> = command.chars().collect();
        let lines: Vec<String> = chars.chunks(per_line).map(|l| l.iter().collect()).collect();
        let galley = ui
            .painter()
            .layout_no_wrap(lines.join("\n"), font.clone(), color);
        let size = egui::vec2(galley.size().x + MARK + 2.0, galley.size().y);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let rows = galley.rows.len();
        for (i, row) in galley.rows.iter().enumerate() {
            if i + 1 < rows {
                let r = row.rect().translate(rect.min.to_vec2());
                let at = egui::pos2(r.right() + 2.0 + MARK / 2.0, r.center().y);
                let mark = egui::Rect::from_center_size(at, egui::Vec2::splat(11.0));
                widgets::paint_glyph(ui.painter(), mark, RETURN, weak);
            }
        }
        ui.painter().galley(rect.min, galley, color);
    }
}

/// One subtitle row, matching a log commit. Returns true when its hash was clicked.
pub fn commit_line(ui: &mut Ui, c: &Commit, abbrev: usize) -> bool {
    let mut clicked = false;
    const SIZE: f32 = 12.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.spacing_mut().interact_size.y = 0.0;
        let weak = ui.visuals().weak_text_color();
        let bar = || {
            RichText::new("|")
                .size(SIZE)
                .color(weak.gamma_multiply(0.6))
        };
        ui.label(RichText::new("at").weak().size(SIZE));
        clicked = ui
            .link(RichText::new(c.oid.short(abbrev)).monospace().size(SIZE))
            .on_hover_text("Show in the log")
            .clicked();
        ui.label(bar());
        let font = egui::FontId::proportional(SIZE);
        let width = |s: &str| {
            ui.painter()
                .layout_no_wrap(s.to_owned(), font.clone(), weak)
                .size()
                .x
        };
        let right = width("|") * 2.0
            + width(&c.author_name)
            + width(&c.author_date)
            + ui.spacing().item_spacing.x * 5.0;
        let room = (ui.available_width() - right).max(25.0);
        let height = ui.fonts_mut(|f| f.row_height(&font));
        ui.allocate_ui_with_layout(
            vec2(room, height),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.add(egui::Label::new(RichText::new(&c.subject).size(SIZE)).truncate())
                    .on_hover_text(&c.subject);
            },
        );
        ui.label(bar());
        ui.label(RichText::new(&c.author_name).weak().size(SIZE));
        ui.label(bar());
        ui.label(RichText::new(&c.author_date).weak().size(SIZE));
    });
    clicked
}

/// A non-editable dropdown, styled like the dialog's text fields. Returns true for an explicit
/// choice, including picking the same value again.
pub fn choice(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    selected: &mut Option<String>,
    none: &str,
    choices: &[String],
) -> bool {
    ui.scope(|ui| {
        let tones = widgets::tones(ui);
        ui.spacing_mut().button_padding = vec2(8.0, 5.0);
        let w = &mut ui.visuals_mut().widgets;
        for v in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            v.bg_fill = tones.field;
            v.weak_bg_fill = tones.field;
            v.bg_stroke = egui::Stroke::new(1.0, tones.field_line);
            v.corner_radius = egui::CornerRadius::same(7);
            v.expansion = 0.0;
        }
        let mut chosen = false;
        egui::ComboBox::from_id_salt(id)
            .width(ui.available_width())
            .wrap_mode(egui::TextWrapMode::Truncate)
            .selected_text(selected.as_deref().unwrap_or(none))
            .popup_style(menu::style.into())
            .show_ui(ui, |ui| {
                chosen |= ui.selectable_value(selected, None, none).clicked();
                for value in choices {
                    chosen |= ui
                        .selectable_value(selected, Some(value.clone()), value)
                        .clicked();
                }
            });
        chosen
    })
    .inner
}

/// Text input with the native popover button. Picking the current value still marks it changed,
/// so a caller can distinguish an explicit selection from an automatically suggested value.
pub fn editable_choice(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    text: &mut String,
    hint: &str,
    choices: &[String],
) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let width = ui.available_width() - 24.0;
        let mut response = widgets::text_field(ui, text, hint, width);
        let button = ui
            .add_enabled_ui(!choices.is_empty(), |ui| {
                widgets::popover_button(ui, ui.id().with(id), None, false)
            })
            .inner;
        egui::Popup::from_toggle_button_response(&button)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .align(egui::RectAlign::BOTTOM_END)
            .gap(4.0)
            .style(menu::popover_style)
            .show(|ui| {
                ui.set_min_width(240.0);
                egui::ScrollArea::vertical()
                    .max_height(280.0)
                    .show(ui, |ui| {
                        for choice in choices {
                            if ui
                                .add(egui::Button::selectable(text.trim() == choice, choice))
                                .clicked()
                            {
                                *text = choice.clone();
                                response.mark_changed();
                                ui.close();
                            }
                        }
                    });
            });
        response
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_and_escape_cancel_a_destructive_dialog() {
        for key in [egui::Key::Enter, egui::Key::Escape] {
            let ctx = egui::Context::default();
            let input = egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            };
            let mut answer = Answer::Open;
            let mut output = ctx.run_ui(input, |ui| {
                answer = actions(ui, "Delete anyway", true, true, true);
            });
            output.textures_delta.clear();
            assert_eq!(answer, Answer::Cancel, "{key:?}");
        }
    }

    fn key(key: egui::Key, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn input(viewport: ViewportId, events: Vec<egui::Event>, close: bool) -> egui::RawInput {
        let mut info = egui::ViewportInfo::default();
        if close {
            info.events.push(egui::ViewportEvent::Close);
        }
        egui::RawInput {
            viewport_id: viewport,
            viewports: std::iter::once((viewport, info)).collect(),
            events,
            ..Default::default()
        }
    }

    #[test]
    fn a_modal_dialog_locks_the_other_windows() {
        let ctx = egui::Context::default();
        // As in the app: every window its own (screenshot runs embed them, and lock nothing).
        ctx.set_embed_viewports(false);
        let dialog = ViewportId::from_hash_of("dialog");
        let mut lock = ModalLock {
            modal: Some(dialog),
            ..Default::default()
        };
        let click = |pressed| egui::Event::PointerButton {
            pos: egui::pos2(5.0, 5.0),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let events = vec![
            key(egui::Key::Enter, true),
            egui::Event::Text("x".into()),
            egui::Event::PointerMoved(egui::pos2(5.0, 5.0)),
            click(true),
            click(false),
            key(egui::Key::Enter, false),
        ];

        let mut own = input(dialog, events.clone(), true);
        lock.lock(&ctx, &mut own);
        assert_eq!(own.events, events, "the dialog keeps its input");
        assert!(own.viewport().close_requested());
        assert!(!lock.raise);

        let mut main = input(ViewportId::ROOT, events, true);
        lock.lock(&ctx, &mut main);
        assert_eq!(
            main.events,
            vec![
                egui::Event::PointerGone,
                click(false),
                key(egui::Key::Enter, false)
            ],
            "only releases stay, and the pointer leaves"
        );
        assert!(!main.viewport().close_requested(), "the main window stays");
        assert!(lock.raise, "the click brings the dialog forward");
    }

    #[test]
    fn without_a_modal_dialog_nothing_is_locked() {
        let ctx = egui::Context::default();
        let mut lock = ModalLock::default();
        let mut main = input(ViewportId::ROOT, vec![key(egui::Key::Enter, true)], true);
        lock.lock(&ctx, &mut main);
        assert_eq!(main.events, vec![key(egui::Key::Enter, true)]);
        assert!(main.viewport().close_requested());
    }

    /// A window manager for dialogs in windows of their own: it opens a window in the size
    /// asked, then takes the sizes it is told (no less than the least), as a platform would.
    #[derive(Default)]
    struct Windows {
        size: Option<Vec2>,
        least: Vec2,
        time: f64,
        builder: Option<egui::ViewportBuilder>,
    }

    impl Windows {
        fn obey(&mut self, commands: Vec<egui::ViewportCommand>) {
            for command in commands {
                match command {
                    egui::ViewportCommand::InnerSize(size) => self.size = Some(size),
                    egui::ViewportCommand::MinInnerSize(least) => self.least = least,
                    _ => {}
                }
            }
            self.size = self.size.map(|s| s.max(self.least));
        }
    }

    const MONITOR: Vec2 = vec2(1600.0, 1000.0);

    /// A context whose dialogs open in windows of their own, kept by `windows`.
    fn windowed(windows: &std::rc::Rc<std::cell::RefCell<Windows>>) -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let windows = windows.clone();
        egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
            let (size, time) = {
                let mut w = windows.borrow_mut();
                let created = viewport.builder.inner_size.unwrap();
                let size = *w.size.get_or_insert(created);
                let patched = match &mut w.builder {
                    Some(old) => old.patch(viewport.builder.clone()).0,
                    None => {
                        w.builder = Some(viewport.builder.clone());
                        Vec::new()
                    }
                };
                w.obey(patched);
                (w.size.unwrap_or(size), w.time)
            };
            let id = viewport.ids.this;
            let rect = egui::Rect::from_min_size(Pos2::ZERO, size);
            let info = egui::ViewportInfo {
                inner_rect: Some(rect),
                monitor_size: Some(MONITOR),
                ..Default::default()
            };
            let input = egui::RawInput {
                viewport_id: id,
                screen_rect: Some(rect),
                time: Some(time),
                viewports: std::iter::once((id, info)).collect(),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui));
            output.textures_delta.clear();
            let commands = output
                .viewport_output
                .get(&id)
                .map(|o| o.commands.clone())
                .unwrap_or_default();
            windows.borrow_mut().obey(commands);
        });
        ctx
    }

    /// One frame of the main window, showing a resizable dialog of `rows` rows in its fields,
    /// or none.
    fn frame(
        ctx: &egui::Context,
        windows: &std::rc::Rc<std::cell::RefCell<Windows>>,
        rows: Option<usize>,
    ) {
        let time = {
            let mut w = windows.borrow_mut();
            w.time += 1.0 / 60.0;
            w.time
        };
        let info = egui::ViewportInfo {
            monitor_size: Some(MONITOR),
            outer_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, MONITOR)),
            ..Default::default()
        };
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, MONITOR)),
            time: Some(time),
            viewports: std::iter::once((ViewportId::ROOT, info)).collect(),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let Some(rows) = rows else { return };
            Dialog::new("test", "Test")
                .width(400.0)
                .resizable()
                .show(ui.ctx(), |ui| {
                    fields(ui, |ui| {
                        for i in 0..rows {
                            ui.label(format!("row {i}"));
                        }
                    });
                    actions(ui, "OK", true, false, false)
                });
        });
        output.textures_delta.clear();
        if rows.is_none() {
            // Its window is gone.
            *windows.borrow_mut() = Windows {
                time,
                ..Default::default()
            };
        }
    }

    fn frames(
        ctx: &egui::Context,
        windows: &std::rc::Rc<std::cell::RefCell<Windows>>,
        rows: Option<usize>,
        n: usize,
    ) -> Vec2 {
        for _ in 0..n {
            frame(ctx, windows, rows);
        }
        windows.borrow().size.unwrap_or_default()
    }

    /// More than a second: past the moment a dialog keeps the size it opened in.
    const SETTLE: usize = 90;

    #[test]
    fn a_resizable_dialog_opens_as_tall_as_its_content_up_to_most_of_the_screen() {
        let windows = Default::default();
        let ctx = windowed(&windows);
        let short = frames(&ctx, &windows, Some(3), SETTLE);
        assert_eq!(short.x, 400.0 + 2.0 * MARGIN);
        assert!(short.y < 300.0, "{short:?}");
        frames(&ctx, &windows, None, 2);
        let long = frames(&ctx, &windows, Some(200), SETTLE);
        assert_eq!(long.y, (MONITOR.y * 0.85).round(), "{long:?}");
        // The fields scroll down to their least; the buttons stay.
        let least = windows.borrow().least;
        assert!(least.y < short.y + MIN_FIELDS, "{least:?} {short:?}");
    }

    #[test]
    fn the_width_the_user_gives_it_is_remembered() {
        let windows = Default::default();
        let ctx = windowed(&windows);
        let opened = frames(&ctx, &windows, Some(3), SETTLE);
        windows.borrow_mut().size = Some(vec2(700.0, opened.y + 100.0));
        let sized = frames(&ctx, &windows, Some(3), 5);
        assert_eq!(
            sized,
            vec2(700.0, opened.y + 100.0),
            "the user's size stays"
        );
        frames(&ctx, &windows, None, 2);
        let again = frames(&ctx, &windows, Some(3), SETTLE);
        assert_eq!(
            again,
            vec2(700.0, opened.y),
            "its width, and its content's height"
        );
    }

    #[test]
    fn its_height_follows_its_content_until_the_user_sizes_it() {
        let windows = Default::default();
        let ctx = windowed(&windows);
        let three = frames(&ctx, &windows, Some(3), SETTLE);
        let six = frames(&ctx, &windows, Some(6), 10);
        assert!(six.y > three.y, "grows with its content: {three:?} {six:?}");
        let back = frames(&ctx, &windows, Some(3), SETTLE);
        assert_eq!(back, three, "and shrinks with it");
        // Taller than its content, by the user: it keeps that, and grows only when the
        // content needs more.
        windows.borrow_mut().size = Some(vec2(three.x, six.y + 50.0));
        frames(&ctx, &windows, Some(3), 5);
        assert_eq!(frames(&ctx, &windows, Some(6), 10).y, six.y + 50.0);
        let many = frames(&ctx, &windows, Some(12), 10);
        assert!(many.y > six.y + 50.0, "{many:?}");
    }
}
