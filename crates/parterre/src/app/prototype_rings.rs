//! PROTOTYPE, throwaway (#401): three ways the rings could look in Settings › Privacy, the
//! new-release dialog, the *Download* menu entry and About. Only with
//! `PARTERRE_PROTOTYPE_RINGS=1`; a bar at the bottom switches variant (F8) and scenario (F9),
//! Shift going back. Nothing here is real: the rings, the channel and the newer build are
//! made up per scenario, and choosing a ring changes nothing but this prototype's state.

use std::sync::{LazyLock, Mutex};

use eframe::egui::{self, Align2, Color32, RichText, Ui};

use super::settings_window::{group, title};
use crate::dialogs::{self, Dialog};
use crate::widgets;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ring {
    Stable,
    Rc,
    Nightly,
    Canary,
}

impl Ring {
    const ALL: [Ring; 4] = [Ring::Stable, Ring::Rc, Ring::Nightly, Ring::Canary];

    fn name(self) -> &'static str {
        match self {
            Ring::Stable => "Stable",
            Ring::Rc => "Release candidates",
            Ring::Nightly => "Nightly",
            Ring::Canary => "Canary",
        }
    }

    /// What the builds of the ring are called in a sentence.
    fn builds(self) -> &'static str {
        match self {
            Ring::Stable => "stable releases",
            Ring::Rc => "release candidates",
            Ring::Nightly => "nightly builds",
            Ring::Canary => "canary builds",
        }
    }

    fn one(self) -> &'static str {
        match self {
            Ring::Stable => "release",
            Ring::Rc => "release candidate",
            Ring::Nightly => "nightly build",
            Ring::Canary => "canary build",
        }
    }

    fn explained(self) -> &'static str {
        match self {
            Ring::Stable => "Releases for everyone, with release notes.",
            Ring::Rc => "Try the next release before it's out. Also hears of stable releases.",
            Ring::Nightly => "The newest work on main, built each night it has moved.",
            Ring::Canary => "Every merge to main. For parterre's maintainer.",
        }
    }

    fn badge(self) -> Option<(&'static str, Color32)> {
        match self {
            Ring::Stable => None,
            Ring::Rc => Some(("RC", Color32::from_rgb(0xc7, 0x8a, 0x10))),
            Ring::Nightly => Some(("NIGHTLY", Color32::from_rgb(0x7a, 0x4f, 0xd0))),
            Ring::Canary => Some(("CANARY", Color32::from_rgb(0xd0, 0x6a, 0x1f))),
        }
    }

    fn of(version: &str) -> Ring {
        if version.contains("-canary") {
            Ring::Canary
        } else if version.contains("-nightly") {
            Ring::Nightly
        } else if version.contains("-rc") {
            Ring::Rc
        } else {
            Ring::Stable
        }
    }
}

struct Scenario {
    name: &'static str,
    running: &'static str,
    stored: Ring,
    /// Installed for all users: stable only, whatever the ring.
    machine_wide: bool,
    offered: &'static str,
    built: &'static str,
}

const SCENARIOS: [Scenario; 5] = [
    Scenario {
        name: "Stable copy, a new release",
        running: "1.2.2",
        stored: Ring::Stable,
        machine_wide: false,
        offered: "1.3.0",
        built: "14 Oct 2026",
    },
    Scenario {
        name: "Release-candidate copy, the stable release",
        running: "1.3.0-rc",
        stored: Ring::Rc,
        machine_wide: false,
        offered: "1.3.2",
        built: "16 Oct 2026",
    },
    Scenario {
        name: "Nightly copy, the next nightly",
        running: "1.3.4-nightly.20261016",
        stored: Ring::Nightly,
        machine_wide: false,
        offered: "1.3.6-nightly.20261017",
        built: "17 Oct 2026",
    },
    Scenario {
        name: "Canary copy, the next canary",
        running: "1.3.6-canary",
        stored: Ring::Canary,
        machine_wide: false,
        offered: "1.3.7-canary",
        built: "17 Oct 2026, 14:02",
    },
    Scenario {
        name: "Installed for all users (Chocolatey), ring rc",
        running: "1.2.2",
        stored: Ring::Rc,
        machine_wide: true,
        offered: "1.3.0",
        built: "14 Oct 2026",
    },
];

const VARIANTS: [&str; 3] = [
    "A: a row under Check for updates",
    "B: a section of explained choices",
    "C: the ring shown with the version",
];

struct Proto {
    variant: usize,
    scenario: usize,
    stored: Ring,
    open_dialog: bool,
    open_settings: bool,
    chooser: bool,
}

static PROTO: LazyLock<Option<Mutex<Proto>>> = LazyLock::new(|| {
    std::env::var_os("PARTERRE_PROTOTYPE_RINGS").map(|v| {
        let variant = match v.to_str() {
            Some("B" | "b") => 1,
            Some("C" | "c") => 2,
            _ => 0,
        };
        let scenario = std::env::var("PARTERRE_PROTOTYPE_SCENARIO")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .map_or(0, |n| n.saturating_sub(1).min(SCENARIOS.len() - 1));
        Mutex::new(Proto {
            variant,
            scenario,
            stored: SCENARIOS[scenario].stored,
            open_dialog: false,
            open_settings: false,
            chooser: false,
        })
    })
});

fn with<R>(f: impl FnOnce(&mut Proto) -> R) -> Option<R> {
    PROTO.as_ref().map(|m| f(&mut m.lock().unwrap()))
}

pub fn active() -> bool {
    PROTO.is_some()
}

fn scenario() -> &'static Scenario {
    &SCENARIOS[with(|p| p.scenario).unwrap_or(0)]
}

fn variant() -> usize {
    with(|p| p.variant).unwrap_or(0)
}

/// The newer build this scenario offers, as `--newer-release` would.
pub fn newer() -> Option<parterre_telemetry::Update> {
    active()
        .then(|| parterre_telemetry::Update::to(scenario().offered))
        .flatten()
}

/// The running version this scenario pretends to be.
pub fn running() -> Option<&'static str> {
    active().then(|| scenario().running)
}

pub fn take_open_dialog() -> bool {
    with(|p| std::mem::take(&mut p.open_dialog)).unwrap_or(false)
}

pub fn take_open_settings() -> bool {
    with(|p| std::mem::take(&mut p.open_settings)).unwrap_or(false)
}

/// The floating bar that switches variant and scenario.
pub fn bar(ctx: &egui::Context) {
    if !active() {
        return;
    }
    let (f8, f9, shift) = ctx.input(|i| {
        (
            i.key_pressed(egui::Key::F8),
            i.key_pressed(egui::Key::F9),
            i.modifiers.shift,
        )
    });
    let step = |n: usize, len: usize, back: bool| if back { (n + len - 1) % len } else { (n + 1) % len };
    with(|p| {
        if f8 {
            p.variant = step(p.variant, VARIANTS.len(), shift);
        }
        if f9 {
            p.scenario = step(p.scenario, SCENARIOS.len(), shift);
            p.stored = SCENARIOS[p.scenario].stored;
        }
    });
    egui::Area::new(egui::Id::new("prototype-rings-bar"))
        .anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -14.0))
        .order(egui::Order::Tooltip)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgb(0x10, 0x10, 0x10))
                .corner_radius(18)
                .inner_margin(egui::Margin::symmetric(14, 8))
                .shadow(egui::Shadow {
                    offset: [0, 4],
                    blur: 16,
                    spread: 0,
                    color: Color32::from_black_alpha(90),
                })
                .show(ui, |ui| {
                    let white = |t: &str| RichText::new(t).color(Color32::WHITE);
                    ui.horizontal(|ui| {
                        ui.label(white("PROTOTYPE").small().strong());
                        ui.separator();
                        if ui.button("◀").clicked() {
                            with(|p| p.variant = step(p.variant, VARIANTS.len(), true));
                        }
                        ui.label(white(VARIANTS[variant()]));
                        if ui.button("▶").clicked() {
                            with(|p| p.variant = step(p.variant, VARIANTS.len(), false));
                        }
                        ui.separator();
                        let n = with(|p| p.scenario).unwrap_or(0);
                        if ui.button("◀").clicked() {
                            with(|p| {
                                p.scenario = step(p.scenario, SCENARIOS.len(), true);
                                p.stored = SCENARIOS[p.scenario].stored;
                            });
                        }
                        ui.label(white(&format!("{}. {}", n + 1, scenario().name)));
                        if ui.button("▶").clicked() {
                            with(|p| {
                                p.scenario = step(p.scenario, SCENARIOS.len(), false);
                                p.stored = SCENARIOS[p.scenario].stored;
                            });
                        }
                        ui.separator();
                        if ui.button("New-release dialog").clicked() {
                            with(|p| p.open_dialog = true);
                        }
                        if ui.button("Settings").clicked() {
                            with(|p| p.open_settings = true);
                        }
                        ui.label(white("F8 / F9").small().weak());
                    });
                    ui.label(
                        white(&format!(
                            "running {} · ring {} · {}",
                            scenario().running,
                            with(|p| p.stored.name()).unwrap_or(""),
                            if scenario().machine_wide {
                                "machine-wide MSI: stable only"
                            } else {
                                "per-user install"
                            }
                        ))
                        .small(),
                    );
                });
        });
    ring_chooser(ctx);
}

/// Settings › Privacy's update rows. Returns false when the prototype isn't on, so the real
/// rows show.
pub fn updates_section(ui: &mut Ui, check: &mut bool) -> bool {
    if !active() {
        return false;
    }
    let s = scenario();
    let mut stored = with(|p| p.stored).unwrap();
    match variant() {
        0 => {
            group(ui, |rows| {
                rows.switch("Check for updates", "", check);
                rows.row("Tell me about", "", |ui| {
                    ui.add_enabled_ui(!s.machine_wide && *check, |ui| {
                        let shown = if s.machine_wide { Ring::Stable } else { stored };
                        egui::ComboBox::from_id_salt("proto-ring")
                            .selected_text(shown.builds())
                            .width(190.0)
                            .show_ui(ui, |ui| {
                                // Canary is only listed once chosen, or with Shift held.
                                let shift = ui.input(|i| i.modifiers.shift);
                                for ring in Ring::ALL {
                                    if ring != Ring::Canary || stored == Ring::Canary || shift {
                                        ui.selectable_value(&mut stored, ring, ring.builds());
                                    }
                                }
                            });
                    });
                });
            });
            ui.add_space(6.0);
            if s.machine_wide {
                ui.label(
                    RichText::new(
                        "Installed for all users, so this copy takes stable releases only.",
                    )
                    .weak(),
                );
            } else if stored != Ring::Stable {
                ui.label(RichText::new(stored.explained()).weak());
            }
        }
        1 => {
            group(ui, |rows| rows.switch("Check for updates", "", check));
            ui.add_space(14.0);
            title(ui, "Builds to hear of");
            if s.machine_wide {
                ui.label(
                    RichText::new(
                        "Installed for all users (Chocolatey, winget for all users), so \
                         this copy takes stable releases only. Install it for yourself to \
                         try other builds.",
                    )
                    .weak(),
                );
                ui.add_space(6.0);
            }
            let this = Ring::of(s.running);
            ui.add_enabled_ui(!s.machine_wide && *check, |ui| {
                let t = widgets::tones(ui);
                let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
                egui::Frame::new()
                    .fill(t.group)
                    .stroke(egui::Stroke::new(1.0, stroke.color))
                    .corner_radius(10)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        for ring in Ring::ALL {
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut stored, ring, RichText::new(ring.name()).strong());
                                if ring == this {
                                    ui.label(
                                        RichText::new(" this copy ")
                                            .small()
                                            .color(t.on_fg)
                                            .background_color(t.on_bg),
                                    );
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.add_space(26.0);
                                ui.label(RichText::new(ring.explained()).weak());
                            });
                            ui.add_space(6.0);
                        }
                    });
            });
        }
        _ => {
            group(ui, |rows| {
                rows.switch("Check for updates", "", check);
                let shown = if s.machine_wide { Ring::Stable } else { stored };
                rows.row("Hearing of", "", |ui| {
                    if !s.machine_wide && widgets::text_button(ui, "Change…").clicked() {
                        with(|p| p.chooser = true);
                    }
                    ui.label(RichText::new(shown.builds()).strong());
                    badge(ui, shown);
                });
            });
            if s.machine_wide {
                ui.add_space(6.0);
                ui.label(RichText::new("Installed for all users: stable releases only.").weak());
            }
        }
    }
    with(|p| p.stored = stored);
    ui.add_space(14.0);
    true
}

fn badge(ui: &mut Ui, ring: Ring) {
    if let Some((text, colour)) = ring.badge() {
        ui.label(
            RichText::new(format!(" {text} "))
                .small()
                .strong()
                .color(Color32::WHITE)
                .background_color(colour),
        );
    }
}

/// Variant C's chooser, a small dialog of its own.
fn ring_chooser(ctx: &egui::Context) {
    if !with(|p| p.chooser).unwrap_or(false) {
        return;
    }
    let mut stored = with(|p| p.stored).unwrap();
    let mut close = false;
    let shown = Dialog::new("proto-ring-chooser", "Which builds to hear of")
        .width(380.0)
        .show(ctx, |ui| {
            dialogs::fields(ui, |ui| {
                for ring in [Ring::Stable, Ring::Rc, Ring::Nightly] {
                    ui.radio_value(&mut stored, ring, RichText::new(ring.name()).strong());
                    ui.label(RichText::new(ring.explained()).weak());
                    ui.add_space(4.0);
                }
                let mut canary = stored == Ring::Canary;
                if ui.checkbox(&mut canary, "Every merge (canary), for the maintainer").changed() {
                    stored = if canary { Ring::Canary } else { Ring::Nightly };
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Running a build from another ring moves you to it. Going back to an \
                         earlier ring waits for a build newer than this one.",
                    )
                    .weak()
                    .small(),
                );
            });
            ui.separator();
            let widths = [widgets::filled_button_width(ui, "Done", 90.0)];
            dialogs::button_row(ui, &widths, |ui| {
                if widgets::primary_button(ui, "Done", 90.0).clicked() {
                    close = true;
                }
            });
        });
    with(|p| {
        p.stored = stored;
        if close || shown.should_close() {
            p.chooser = false;
        }
    });
}

/// The new-release dialog's content per variant: `Some(true)` for *Download*, `Some(false)`
/// to close, `None` while open. Called only when [`active`].
pub fn release_dialog(ctx: &egui::Context) -> Option<bool> {
    let s = scenario();
    let ring = Ring::of(s.offered);
    let stored = with(|p| p.stored).unwrap();
    let title = match variant() {
        0 => "New release".to_owned(),
        1 => format!("New {}", ring.one()),
        _ => "Update".to_owned(),
    };
    let mut answer = None;
    let shown = Dialog::new("proto-new-release", &title)
        .width(380.0)
        .show(ctx, |ui| {
            dialogs::fields(ui, |ui| match variant() {
                0 => {
                    ui.label(
                        RichText::new(format!("parterre {} is out", s.offered))
                            .size(16.0)
                            .strong(),
                    );
                    ui.label(format!("This is {}.", s.running));
                    if ring != Ring::Stable {
                        ui.label(RichText::new(format!("A {}.", ring.one())).weak());
                    }
                    let _ = ui.link("Release notes");
                }
                1 => {
                    ui.label(RichText::new(s.offered).size(20.0).strong());
                    ui.label(RichText::new(format!("Built {} from abcd1005", s.built)).weak());
                    ui.label(format!("You run {}.", s.running));
                    let _ = ui.link("What's new since yours");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("You hear of {}.", stored.builds())).weak(),
                        );
                        if ui.link("Change").clicked() {
                            with(|p| p.open_settings = true);
                        }
                    });
                }
                _ => {
                    ui.horizontal(|ui| {
                        badge(ui, ring);
                        ui.label(RichText::new(s.offered).size(16.0).strong());
                    });
                    ui.label(RichText::new(format!("Newer than yours, {}.", s.running)).weak());
                }
            });
            ui.separator();
            let (close, download) = match variant() {
                2 => ("Later", "Download"),
                _ => ("Close", "Download"),
            };
            let widths = [
                widgets::text_button_width(ui, close),
                widgets::filled_button_width(ui, download, 90.0),
            ];
            dialogs::button_row(ui, &widths, |ui| {
                if widgets::text_button(ui, close).clicked() {
                    answer = Some(false);
                }
                if widgets::primary_button(ui, download, 90.0).clicked() {
                    answer = Some(true);
                }
            });
        });
    if shown.should_close() {
        Some(false)
    } else {
        answer
    }
}

/// The *Download* entry's label for `version`.
pub fn download_label(version: &str) -> String {
    if !active() {
        return format!("Download {version}");
    }
    let ring = Ring::of(version);
    match variant() {
        0 => format!("Download {version}"),
        1 => match ring {
            Ring::Nightly => format!("Download nightly ({})", scenario().built),
            Ring::Canary => "Download canary (latest merge)".to_owned(),
            _ => format!("Download {version}"),
        },
        _ => format!("Download the newer {}", ring.one()),
    }
}

/// Under the version pill in About.
pub fn about(ui: &mut Ui) {
    if !active() {
        return;
    }
    let s = scenario();
    let stored = with(|p| p.stored).unwrap();
    match variant() {
        0 => {
            if stored != Ring::Stable {
                ui.label(RichText::new(format!("Hears of {}", stored.builds())).small().weak());
            }
        }
        1 => {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Ring: {}", stored.name())).small());
                if ui.link(RichText::new("Change").small()).clicked() {
                    with(|p| p.open_settings = true);
                }
            });
        }
        _ => {
            ui.horizontal(|ui| {
                badge(ui, Ring::of(s.running));
                if stored != Ring::of(s.running) {
                    ui.label(RichText::new(format!("hearing of {}", stored.builds())).small().weak());
                }
            });
        }
    }
}
