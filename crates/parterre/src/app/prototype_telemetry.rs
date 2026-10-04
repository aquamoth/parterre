//! PROTOTYPE for #227, throwaway (branch `prototype/telemetry-prompt`, never main): the
//! first-run prompt about usage statistics and crash reports, the Privacy settings page, and the
//! update marker of #226. Nothing is sent anywhere.
//!
//! `PARTERRE_PROTOTYPE=A|B|C` turns it on and picks the prompt's variant; the bar at the bottom
//! flips between them. `PARTERRE_PROTOTYPE_ANSWERED=1` starts with the prompt answered,
//! `PARTERRE_PROTOTYPE_NO_BAR=1` hides the bar (for screenshots), and `DO_NOT_TRACK=1` shows
//! the Privacy page as the environment variable leaves it.

use eframe::egui::{self, Align, Align2, Color32, CornerRadius, Layout, RichText, Ui, vec2};

use super::ParterreApp;
use crate::widgets;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Dialog,
    Bar,
    Corner,
}

impl Variant {
    const ALL: [Variant; 3] = [Variant::Dialog, Variant::Bar, Variant::Corner];

    fn key(self) -> &'static str {
        match self {
            Variant::Dialog => "A",
            Variant::Bar => "B",
            Variant::Corner => "C",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Variant::Dialog => "Dialog at first start",
            Variant::Bar => "Bar under the toolbar",
            Variant::Corner => "Card in the corner",
        }
    }
}

#[derive(Debug)]
pub struct Prototype {
    pub on: bool,
    pub variant: Variant,
    /// The first-run prompt has been answered (or put off for this run).
    pub answered: bool,
    pub usage: bool,
    pub crashes: bool,
    /// What crash reports were at start: the SDK's panic capture is set then.
    crashes_at_start: bool,
    pub update_check: bool,
    /// A newer release, as the update check would find it.
    pub latest: Option<&'static str>,
    do_not_track: bool,
    bar: bool,
}

impl Default for Prototype {
    fn default() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let chosen = var("PARTERRE_PROTOTYPE");
        let variant = match chosen.as_deref() {
            Some("B" | "b") => Variant::Bar,
            Some("C" | "c") => Variant::Corner,
            _ => Variant::Dialog,
        };
        let on = chosen.is_some();
        Self {
            on,
            variant,
            answered: !on || var("PARTERRE_PROTOTYPE_ANSWERED").is_some(),
            usage: true,
            crashes: false,
            crashes_at_start: false,
            update_check: true,
            latest: on.then_some("0.6.0"),
            do_not_track: var("DO_NOT_TRACK").is_some_and(|v| v != "0"),
            bar: var("PARTERRE_PROTOTYPE_NO_BAR").is_none(),
        }
    }
}

const USAGE: &str = "Usage statistics";
const USAGE_TEXT: &str = "Installs, launches and which features are used, with a random ID for \
                          this installation. Never personal information.";
const CRASHES: &str = "Crash reports";
const CRASHES_TEXT: &str = "What parterre was doing when it crashed. May contain personal \
                            information, such as a file or branch name.";
const WHERE: &str = "Sent to PostHog in the EU. Settings › Privacy changes either later.";
const UPDATES_TEXT: &str = "Asks GitHub once a day whether a newer release is out. Sends \
                            nothing of parterre's own.";

/// A checkbox with its explanation underneath, lined up with its label.
fn choice(ui: &mut Ui, on: &mut bool, label: &str, text: &str, width: f32) {
    ui.checkbox(on, RichText::new(label).strong());
    ui.horizontal(|ui| {
        ui.add_space(ui.spacing().icon_width + ui.spacing().icon_spacing);
        ui.set_max_width(width);
        ui.add(egui::Label::new(RichText::new(text).weak()).wrap());
    });
}

impl ParterreApp {
    /// The bar under the toolbar (variant B). Call between the toolbar and the central panel.
    pub(super) fn prototype_bar(&mut self, ui: &mut Ui) {
        let p = &mut self.prototype;
        if !p.on || p.answered || p.variant != Variant::Bar {
            return;
        }
        let t = widgets::tones(ui);
        let mut open_settings = false;
        egui::Panel::top("telemetry-bar")
            .frame(
                egui::Frame::new()
                    .fill(t.on_bg)
                    .inner_margin(egui::Margin::symmetric(12, 6)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("parterre sends").strong());
                    ui.checkbox(&mut p.usage, USAGE).on_hover_text(USAGE_TEXT);
                    ui.weak("(never personal information)");
                    ui.add_space(8.0);
                    ui.checkbox(&mut p.crashes, CRASHES)
                        .on_hover_text(CRASHES_TEXT);
                    ui.weak("(may contain personal information)");
                    ui.add_space(8.0);
                    ui.weak("to PostHog, EU.");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::primary_button(ui, "OK", 60.0).clicked() {
                            p.answered = true;
                            p.crashes_at_start = p.crashes;
                        }
                        if widgets::text_button(ui, "Settings…").clicked() {
                            open_settings = true;
                        }
                    });
                });
            });
        if open_settings {
            self.open_settings(super::SettingsPage::Privacy);
        }
    }

    /// The prompt of variants A and C, and the variant switcher. Call after the panels.
    pub(super) fn prototype_windows(&mut self, ctx: &egui::Context) {
        if !self.prototype.on {
            return;
        }
        if !self.prototype.answered {
            match self.prototype.variant {
                Variant::Dialog => self.prototype_dialog(ctx),
                Variant::Corner => self.prototype_corner(ctx),
                Variant::Bar => {}
            }
        }
        if self.prototype.bar {
            self.prototype_switcher(ctx);
        }
    }

    fn prototype_dialog(&mut self, ctx: &egui::Context) {
        let p = &mut self.prototype;
        let shown =
            crate::dialogs::Dialog::new("telemetry-prompt", "Usage statistics and crash reports")
                .width(430.0)
                .modal()
                .show(ctx, |ui| {
                    choice(ui, &mut p.usage, USAGE, USAGE_TEXT, 380.0);
                    ui.add_space(10.0);
                    choice(ui, &mut p.crashes, CRASHES, CRASHES_TEXT, 380.0);
                    ui.add_space(12.0);
                    ui.weak(WHERE);
                    ui.add_space(12.0);
                    ui.separator();
                    let mut ok = false;
                    ui.allocate_ui_with_layout(
                        vec2(ui.available_width(), 34.0),
                        Layout::right_to_left(Align::Center),
                        |ui| ok = widgets::primary_button(ui, "Continue", 90.0).clicked(),
                    );
                    ok
                });
        if shown.inner {
            p.answered = true;
            p.crashes_at_start = p.crashes;
        } else if shown.should_close() {
            // Asked again at the next start; nothing is sent meanwhile.
            p.answered = true;
            p.usage = false;
            p.crashes = false;
        }
    }

    fn prototype_corner(&mut self, ctx: &egui::Context) {
        let p = &mut self.prototype;
        let status = if self.settings.show_status_bar {
            34.0
        } else {
            12.0
        };
        let mut open_settings = false;
        egui::Area::new(egui::Id::new("telemetry-corner"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-14.0, -status))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(14)
                    .show(ui, |ui| {
                        ui.set_width(330.0);
                        ui.label(RichText::new("Usage statistics and crash reports").strong());
                        ui.add_space(8.0);
                        choice(ui, &mut p.usage, USAGE, USAGE_TEXT, 300.0);
                        ui.add_space(6.0);
                        choice(ui, &mut p.crashes, CRASHES, CRASHES_TEXT, 300.0);
                        ui.add_space(8.0);
                        ui.weak("Sent to PostHog in the EU.");
                        ui.add_space(6.0);
                        ui.allocate_ui_with_layout(
                            vec2(ui.available_width(), 32.0),
                            Layout::right_to_left(Align::Center),
                            |ui| {
                                if widgets::primary_button(ui, "OK", 60.0).clicked() {
                                    p.answered = true;
                                    p.crashes_at_start = p.crashes;
                                }
                                if widgets::text_button(ui, "Settings…").clicked() {
                                    open_settings = true;
                                }
                            },
                        );
                    });
            });
        if open_settings {
            self.open_settings(super::SettingsPage::Privacy);
        }
    }

    fn prototype_switcher(&mut self, ctx: &egui::Context) {
        let p = &mut self.prototype;
        egui::Area::new(egui::Id::new("prototype-switcher"))
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -40.0))
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x30, 0x10, 0x50))
                    .corner_radius(CornerRadius::same(16))
                    .inner_margin(egui::Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.visuals_mut().override_text_color = Some(Color32::WHITE);
                        ui.horizontal(|ui| {
                            let i = Variant::ALL
                                .iter()
                                .position(|v| *v == p.variant)
                                .unwrap_or(0);
                            if ui.button("◀").clicked() {
                                p.variant = Variant::ALL[(i + 2) % 3];
                                p.answered = false;
                            }
                            ui.label(format!(
                                "PROTOTYPE #227 · {} — {}",
                                p.variant.key(),
                                p.variant.name()
                            ));
                            if ui.button("▶").clicked() {
                                p.variant = Variant::ALL[(i + 1) % 3];
                                p.answered = false;
                            }
                            if ui.button("Ask again").clicked() {
                                p.answered = false;
                            }
                        });
                    });
            });
    }

    /// Settings › Privacy.
    pub(super) fn privacy_page(p: &mut Prototype, ui: &mut Ui) {
        super::settings_window::group(ui, |rows| {
            rows.switch("Check for updates", UPDATES_TEXT, &mut p.update_check);
        });
        ui.add_space(14.0);
        super::settings_window::title(ui, "Sent to PostHog");
        let locked = p.do_not_track;
        let crashes_changed = p.crashes != p.crashes_at_start;
        super::settings_window::group(ui, |rows| {
            rows.row(USAGE, USAGE_TEXT, |ui| {
                let mut off = false;
                let on = if locked { &mut off } else { &mut p.usage };
                ui.add_enabled_ui(!locked, |ui| widgets::switch(ui, on));
            });
            rows.row(CRASHES, CRASHES_TEXT, |ui| {
                let mut off = false;
                let on = if locked { &mut off } else { &mut p.crashes };
                ui.add_enabled_ui(!locked, |ui| widgets::switch(ui, on));
                if crashes_changed {
                    ui.weak("from the next start");
                }
            });
            rows.row("Install ID", "Sent with the usage statistics, never with a crash report. Quote it to have its data deleted.", |ui| {
                if widgets::text_button(ui, "Copy").clicked() {
                    ui.ctx().copy_text("0198f3c2-7a41-7c3e-9f1d-2b6a5e8c4d17".into());
                }
                ui.label(RichText::new("0198f3c2-7a41-7c3e-9f1d-2b6a5e8c4d17").monospace().weak());
            });
        });
        ui.add_space(8.0);
        if locked {
            ui.weak("Off: DO_NOT_TRACK is set.");
        } else {
            ui.weak("Usage statistics never contain personal information. Crash reports may.");
        }
        ui.hyperlink_to(
            "What parterre sends",
            "https://github.com/aquamoth/parterre/blob/main/docs/privacy.md",
        );
    }
}
