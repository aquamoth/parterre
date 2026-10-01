//! Shared native dialogs: windows of their own, theme, keyboard-safe action rows and command
//! previews. Tools provide their own fields, validation and Git decisions.
//!
//! A dialog is a window beside parterre's others, moved by its title bar. A modeless one leaves
//! them alone. A modal one locks them all, whichever window opened it, until it is answered:
//! [`ModalLock`] drops their input, so a click or a close there brings the dialog forward.

use std::sync::Arc;

use eframe::egui::{self, Align, Color32, Id, Layout, Pos2, RichText, Ui, Vec2, ViewportId, vec2};
use parterre_core::{Commit, glyphs::Glyph};

use crate::{menu, widgets};

/// Space around a dialog's content.
const MARGIN: f32 = 20.0;

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

/// One dialog window, kept from frame to frame while it is shown.
#[derive(Clone, Copy, Debug, Default)]
struct Window {
    /// Measured in the frame before; `None` until the content was laid out once.
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
}

#[derive(Debug)]
pub struct Dialog<'a> {
    id: Id,
    title: &'a str,
    width: f32,
    icon: Option<Glyph>,
    danger: bool,
    modal: bool,
    opener: ViewportId,
    raise: bool,
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
            opener: ViewportId::ROOT,
            raise: false,
        }
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
    /// The window it opens over, where the platform lets parterre place windows (not Wayland).
    pub fn opener(mut self, opener: ViewportId) -> Self {
        self.opener = opener;
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

    /// Shows the dialog in its own window, sized to its content. Call every frame while it is
    /// open; the window goes when the calls stop. `content` may run more than once a frame: the
    /// first time, it is measured before the window opens.
    pub fn show<R>(&self, ctx: &egui::Context, mut content: impl FnMut(&mut Ui) -> R) -> Shown<R> {
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
        let width = self.width.min(monitor.x - 72.0).max(200.0);
        let mut style = (*ctx.global_style()).clone();
        menu::popover_style(&mut style);
        // A new window opens in its size and place, rather than moving and growing in view.
        let size = *window
            .size
            .get_or_insert_with(|| self.measure(ctx, opener.as_ref(), width, &style, &mut content));
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
        // Same size always. On Wayland that is told to the window once it exists (below):
        // told here, winit would set the hints before its title bar exists and leave the bar
        // out of them; the compositor then holds the window to the hints, with the bar outside
        // its frame (above the screen, at the top) and the content cut short by its height.
        if !wayland() {
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
                    if wayland() && window.hinted != Some(size) {
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
                    // and winit takes it. Ask again until the window has it.
                    let off = ui.ctx().content_rect().size() - size;
                    if off.abs().max_elem() > 1.0 {
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
                let (inner, measured) = self.body(ui, width, &style, &mut content);
                (inner, close, measured)
            });
        window.size = Some(measured);
        ctx.data_mut(|d| d.insert_temp(key, window));
        Shown { inner, close }
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
                vec2(width + 2.0 * MARGIN, 100_000.0),
            )),
            viewports: std::iter::once((ViewportId::ROOT, info)).collect(),
            ..Default::default()
        };
        let mut size = Vec2::ZERO;
        let mut output = sizer.run_ui(input, |ui| {
            size = self.body(ui, width, style, content).1;
        });
        // Nothing is painted.
        output.textures_delta.clear();
        size
    }

    /// The dialog's content, as tall as it wants (not the window: a window still too short
    /// would squeeze the fields, and be measured too short again). Returns its answer and the
    /// window size it needs.
    fn body<R>(
        &self,
        ui: &mut Ui,
        width: f32,
        style: &egui::Style,
        content: &mut impl FnMut(&mut Ui) -> R,
    ) -> (R, Vec2) {
        let frame = egui::Frame::new()
            .fill(style.visuals.window_fill)
            .inner_margin(egui::Margin::same(MARGIN as i8));
        egui::CentralPanel::default()
            .frame(frame)
            .show(ui, |ui| {
                ui.set_style(style.clone());
                let room = egui::Rect::from_min_size(ui.max_rect().min, vec2(width, 100_000.0));
                let builder = egui::UiBuilder::new()
                    .max_rect(room)
                    .layout(Layout::top_down(Align::Min));
                let shown = ui.scope_builder(builder, |ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing.y = 10.0;
                    self.heading(ui);
                    content(ui)
                });
                let height = shown.response.rect.height();
                let size = (vec2(width, height) + Vec2::splat(2.0 * MARGIN)).round();
                (shown.inner, size)
            })
            .inner
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

/// The dialog's fields, scrolling once the window would be taller than most of the screen.
/// Callers keep the actions outside it, always in view.
pub fn fields<R>(ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> R {
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
}
