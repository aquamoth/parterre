//! Shared native dialogs: placement, theme, keyboard-safe action rows and command previews.
//! Tools provide their own fields, validation and Git decisions.

use eframe::egui::{self, Align, Color32, Id, Layout, RichText, Ui, vec2};
use parterre_core::{Commit, glyphs::Glyph};

use crate::{menu, widgets};

#[derive(Clone, Copy, Debug)]
struct Placement {
    screen: egui::Vec2,
    top: f32,
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
        let area = match old.filter(|p| p.screen == screen.size() && frame <= p.frame + 1) {
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
        ctx.data_mut(|d| {
            d.insert_temp(
                key,
                Placement {
                    screen: screen.size(),
                    top: rect.top() - screen.top(),
                    frame,
                },
            )
        });
        modal
    }
}

pub fn fields<R>(ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> R {
    let remaining = ui.ctx().content_rect().bottom() - ui.cursor().top() - 165.0;
    egui::ScrollArea::vertical()
        .id_salt("dialog-fields")
        .max_height(remaining.clamp(70.0, 560.0))
        .auto_shrink([false, true])
        .show(ui, content)
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
    ui.separator();
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
    let key = Id::new("git-command-expanded");
    let open = ui
        .data_mut(|d| d.get_persisted::<bool>(key))
        .unwrap_or(false);
    let response = egui::CollapsingHeader::new("Git command")
        .open(Some(open))
        .show(ui, |ui| {
            let text = commands.join("\n");
            let tones = widgets::tones(ui);
            egui::Frame::new()
                .fill(tones.seg_bg)
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        let width = (ui.available_width() - 30.0).max(50.0);
                        egui::ScrollArea::vertical()
                            .id_salt("command-lines")
                            .max_height(65.0)
                            .show(ui, |ui| {
                                ui.set_width(width);
                                ui.add(
                                    egui::Label::new(RichText::new(&text).monospace().size(13.0))
                                        .wrap(),
                                );
                                ui.set_min_height(28.0);
                            });
                        let now = ui.input(|i| i.time);
                        let copied_key = ui.id().with("copied-command");
                        let copied = ui
                            .data(|d| d.get_temp::<f64>(copied_key))
                            .is_some_and(|t| now - t < 1.5);
                        if copied {
                            ui.ctx()
                                .request_repaint_after(std::time::Duration::from_millis(250));
                        }
                        if widgets::copy_button(ui, copied, tones.accent)
                            .on_hover_text("Copy the command")
                            .clicked()
                        {
                            ui.ctx().copy_text(text);
                            ui.data_mut(|d| d.insert_temp(copied_key, now));
                        }
                    });
                });
        });
    if response.header_response.clicked() {
        ui.data_mut(|d| d.insert_persisted(key, !open));
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

/// Text input with the native popover button; empty is an explicit no-selection choice.
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
        let response = widgets::text_field(ui, text, hint, width);
        let button = widgets::popover_button(ui, ui.id().with(id), None, false);
        egui::Popup::from_toggle_button_response(&button)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .align(egui::RectAlign::BOTTOM_END)
            .gap(4.0)
            .style(menu::popover_style)
            .show(|ui| {
                ui.set_min_width(240.0);
                if ui
                    .add(egui::Button::selectable(text.is_empty(), hint))
                    .clicked()
                {
                    text.clear();
                    ui.close();
                }
                egui::ScrollArea::vertical()
                    .max_height(280.0)
                    .show(ui, |ui| {
                        for choice in choices {
                            if ui
                                .add(egui::Button::selectable(text.trim() == choice, choice))
                                .clicked()
                            {
                                *text = choice.clone();
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
