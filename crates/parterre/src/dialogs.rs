//! Shared native dialogs: placement, theme, keyboard-safe action rows and command previews.
//! Tools provide their own fields, validation and Git decisions.

use eframe::egui::{self, Align, Color32, Id, Layout, RichText, Ui, vec2};
use parterre_core::{Commit, glyphs::Glyph};

use crate::{menu, widgets};

#[derive(Clone, Copy, Debug)]
struct Placement {
    screen: egui::Vec2,
    top: f32,
    height: f32,
    settled: bool,
    frame: u64,
}

#[derive(Debug)]
pub struct Dialog<'a> {
    id: Id,
    title: &'a str,
    width: f32,
    icon: Option<Glyph>,
    danger: bool,
}

impl<'a> Dialog<'a> {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, title: &'a str) -> Self {
        Self {
            id: Id::new(id),
            title,
            width: 470.0,
            icon: None,
            danger: false,
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

    /// Centre on opening, then keep the top fixed while content grows. Re-centre on resize.
    /// `fields` bounds the scrollable body; callers keep actions outside it.
    pub fn show<R>(
        &self,
        ctx: &egui::Context,
        content: impl FnOnce(&mut Ui) -> R,
    ) -> egui::ModalResponse<R> {
        let mut style = (*ctx.global_style()).clone();
        menu::popover_style(&mut style);
        let screen = ctx.content_rect();
        let key = self.id.with("placement");
        let old: Option<Placement> = ctx.data(|d| d.get_temp(key));
        let area = egui::Modal::default_area(self.id);
        let frame = ctx.cumulative_frame_nr();
        let old = old.filter(|p| p.screen == screen.size() && frame <= p.frame + 1);
        let area = match old.filter(|p| p.settled) {
            Some(p) => area.anchor(egui::Align2::CENTER_TOP, vec2(0.0, p.top)),
            None => area,
        };
        let modal = egui::Modal::new(self.id)
            .area(area)
            .frame(
                egui::Frame::popup(&style)
                    .inner_margin(egui::Margin::same(20))
                    .corner_radius(12),
            )
            .backdrop_color(Color32::from_black_alpha(if style.visuals.dark_mode {
                90
            } else {
                40
            }))
            .show(ctx, |ui| {
                ui.set_style(style.clone());
                ui.set_width(self.width.min((screen.width() - 72.0).max(200.0)));
                ui.spacing_mut().item_spacing.y = 10.0;
                if let Some(glyph) = self.icon {
                    let tones = widgets::tones(ui);
                    let fg = if self.danger {
                        ui.visuals().error_fg_color
                    } else {
                        tones.on_fg
                    };
                    ui.horizontal(|ui| {
                        let (badge, _) =
                            ui.allocate_exact_size(vec2(32.0, 32.0), egui::Sense::hover());
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
                        ui.add(
                            egui::Label::new(RichText::new(self.title).size(16.0).strong()).wrap(),
                        );
                    });
                } else {
                    ui.label(RichText::new(self.title).size(16.0));
                }
                content(ui)
            });
        let rect = modal.response.rect;
        let settled = old.is_some_and(|p| p.settled || (p.height - rect.height()).abs() < 0.5);
        if !settled {
            ctx.request_repaint();
        }
        ctx.data_mut(|d| {
            d.insert_temp(
                key,
                Placement {
                    screen: screen.size(),
                    top: rect.top() - screen.top(),
                    height: rect.height(),
                    settled,
                    frame,
                },
            )
        });
        modal
    }
}

pub fn fields<R>(ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> R {
    let used = ui.cursor().top() - ui.min_rect().top();
    let remaining = (ui.ctx().content_rect().bottom() - ui.cursor().top() - 165.0)
        .min(630.0 - 40.0 - used - 165.0);
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
}
