//! PROTOTYPE — throwaway. Three About dialogs to choose between, switched with ◀ ▶ (or ← →)
//! in the strip at the bottom of the dialog, or `PARTERRE_ABOUT_VARIANT=A|B|C`. Debug builds
//! only show the strip. Not for main: fold the winner into `about_window` and drop this file.
//!
//! A — Centred: icon, name and version over grouped rows (GNOME's and macOS's About).
//! B — Banner and tabs: a header band, then About · Licence · Third party · Build (KDE, Windows).
//! C — Side panel: a tinted column with a big icon beside a terse key/value sheet.

use eframe::egui::{
    self, Align, Color32, CornerRadius, Layout, RichText, Stroke, TextureHandle, Ui, vec2,
};

use crate::dialogs::{self, Answer};
use crate::widgets;

const NOTICE: &str = include_str!(env!("PARTERRE_NOTICE"));
const LICENSE: &str = include_str!(env!("PARTERRE_LICENSE"));

const TAGLINE: &str = "A TortoiseGit-style revision graph viewer";
const COPYRIGHT: &str = "© 2026 Trustfall AB";
const CONTACT: &str = "parterre@trustfall.se";
const WEBSITE: &str = "https://github.com/aquamoth/parterre";
const ISSUES: &str = "https://github.com/aquamoth/parterre/issues";
const PRIVACY: &str = "https://github.com/aquamoth/parterre/blob/main/docs/privacy.md";

const VARIANTS: [(&str, &str); 3] = [
    ("A", "Centred"),
    ("B", "Banner and tabs"),
    ("C", "Side panel"),
];

#[derive(Clone, Copy, Default, PartialEq)]
enum Tab {
    #[default]
    About,
    Licence,
    ThirdParty,
    Build,
}

#[derive(Clone, Default)]
struct State {
    variant: usize,
    /// A's legal sub-page.
    legal: bool,
    tab: Tab,
    copied: bool,
}

fn state_id() -> egui::Id {
    egui::Id::new("about-prototype")
}

/// Shows the About dialog as the current variant; false once it is closed.
pub fn show(ctx: &egui::Context) -> bool {
    let mut s: State = ctx
        .data(|d| d.get_temp(state_id()))
        .unwrap_or_else(|| State {
            variant: std::env::var("PARTERRE_ABOUT_VARIANT")
                .ok()
                .and_then(|v| VARIANTS.iter().position(|(k, _)| v.eq_ignore_ascii_case(k)))
                .unwrap_or(0),
            ..State::default()
        });
    let icon = icon(ctx);
    let width = [380.0, 560.0, 600.0][s.variant];
    let shown = dialogs::Dialog::new(("about", s.variant), "About parterre")
        .screen(crate::usage::Screen::About)
        .width(width)
        .show(ctx, |ui| {
            let answer = match s.variant {
                0 => centred(ui, &mut s, &icon),
                1 => banner(ui, &mut s, &icon),
                _ => side_panel(ui, &mut s, &icon),
            };
            if cfg!(debug_assertions) {
                switcher(ui, &mut s);
            }
            answer
        });
    let open = shown.inner != Answer::Cancel && !shown.should_close();
    if open {
        ctx.data_mut(|d| d.insert_temp(state_id(), s));
    } else {
        ctx.data_mut(|d| d.remove::<State>(state_id()));
    }
    open
}

fn icon(ctx: &egui::Context) -> TextureHandle {
    let id = egui::Id::new("about-prototype-icon");
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return t;
    }
    let size = 256;
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [size, size],
        &parterre_core::icon::render(size as u32),
    );
    let t = ctx.load_texture("about-icon", image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

fn image(ui: &mut Ui, icon: &TextureHandle, side: f32) {
    ui.add(egui::Image::new((icon.id(), vec2(side, side))));
}

fn git_version() -> String {
    parterre_core::git::version().unwrap_or_else(|| "not found".into())
}

fn system() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

/// What a bug report wants to know.
fn build_info() -> String {
    format!(
        "parterre {}\ngit {}\n{}",
        crate::VERSION,
        git_version(),
        system()
    )
}

fn link(ui: &mut Ui, text: &str, url: &str) {
    if ui.link(text).on_hover_text(url).clicked() {
        let _ = crate::browser::open(url);
    }
}

fn legal_text(ui: &mut Ui) {
    ui.add(egui::Label::new(RichText::new(NOTICE).monospace().size(11.0)).extend());
    ui.collapsing("GNU General Public License, version 3", |ui| {
        ui.add(egui::Label::new(RichText::new(LICENSE).monospace().size(11.0)).extend());
    });
}

/// Rows on a rounded background, as in the settings window.
fn group(ui: &mut Ui, rows: &[(&str, &str, &str)]) -> Option<usize> {
    let t = widgets::tones(ui);
    let line = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let mut clicked = None;
    egui::Frame::new()
        .fill(t.group)
        .stroke(Stroke::new(1.0, line))
        .corner_radius(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            for (i, (label, value, mark)) in rows.iter().enumerate() {
                let rect = ui.available_rect_before_wrap();
                if i > 0 {
                    ui.painter()
                        .hline(rect.x_range(), rect.top(), Stroke::new(1.0, line));
                }
                let row = ui
                    .horizontal(|ui| {
                        ui.set_min_height(40.0);
                        ui.add_space(12.0);
                        ui.label(*label);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add_space(12.0);
                            ui.label(RichText::new(*mark).weak());
                            ui.label(RichText::new(*value).weak());
                        });
                    })
                    .response;
                let row = ui.interact(
                    row.rect,
                    ui.id().with(("about-row", *label)),
                    egui::Sense::click(),
                );
                if row.hovered() {
                    ui.painter()
                        .rect_filled(row.rect, CornerRadius::same(9), t.hover);
                }
                if row.clicked() {
                    clicked = Some(i);
                }
            }
        });
    clicked
}

/// A — GNOME's AdwAboutDialog and macOS's About panel.
fn centred(ui: &mut Ui, s: &mut State, icon: &TextureHandle) -> Answer {
    if s.legal {
        ui.horizontal(|ui| {
            if widgets::text_button(ui, "‹ Back").clicked() {
                s.legal = false;
            }
            ui.add_space(8.0);
            ui.label(RichText::new("Legal").strong().size(15.0));
        });
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .max_height(380.0)
            .show(ui, legal_text);
        ui.separator();
        return dialogs::actions(ui, "", false, false, false);
    }
    ui.vertical_centered(|ui| {
        ui.add_space(8.0);
        let rect = ui.add(egui::Image::new((icon.id(), vec2(96.0, 96.0)))).rect;
        // The tile's own corners (114.5 of 512); a faint rim keeps it off a dark background.
        if ui.visuals().dark_mode {
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same((96.0 * 114.5 / 512.0) as u8),
                Stroke::new(1.0, Color32::from_white_alpha(46)),
                egui::StrokeKind::Inside,
            );
        }
        ui.add_space(6.0);
        ui.label(RichText::new("parterre").strong().size(22.0));
        ui.label(RichText::new(TAGLINE).weak());
        ui.add_space(6.0);
        let t = widgets::tones(ui);
        let pill = egui::Button::new(RichText::new(crate::VERSION).color(t.on_fg))
            .fill(t.on_bg)
            .corner_radius(12);
        if ui
            .add(pill)
            .on_hover_text("Copy version, git and system")
            .clicked()
        {
            ui.ctx().copy_text(build_info());
        }
        ui.add_space(12.0);
    });
    match group(
        ui,
        &[
            ("Website", "", "↗"),
            ("Report a bug", "", "↗"),
            ("Privacy", "", "↗"),
            ("Contact", CONTACT, "↗"),
        ],
    ) {
        Some(0) => drop(crate::browser::open(WEBSITE)),
        Some(1) => drop(crate::browser::open(ISSUES)),
        Some(2) => drop(crate::browser::open(PRIVACY)),
        // PROTOTYPE: copies; the real one opens mailto: (browser.rs allows github.com only).
        Some(_) => ui.ctx().copy_text(CONTACT.to_owned()),
        None => {}
    }
    ui.add_space(10.0);
    match group(
        ui,
        &[
            ("Legal", "GPL-3.0-only", "›"),
            ("Third-party licences", "", "↗"),
        ],
    ) {
        Some(0) => s.legal = true,
        Some(_) => {}
        None => {}
    }
    ui.add_space(10.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(COPYRIGHT).small().weak());
        ui.label(
            RichText::new("This program comes with absolutely no warranty.")
                .small()
                .weak(),
        );
    });
    ui.add_space(4.0);
    ui.separator();
    dialogs::actions(ui, "", false, false, false)
}

/// B — KDE's KAboutApplicationDialog and the Windows About box with tabs.
fn banner(ui: &mut Ui, s: &mut State, icon: &TextureHandle) -> Answer {
    let t = widgets::tones(ui);
    egui::Frame::new()
        .fill(t.on_bg)
        .corner_radius(10)
        .inner_margin(14)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                image(ui, icon, 64.0);
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("parterre").strong().size(22.0).color(t.on_fg));
                        ui.label(RichText::new(crate::VERSION).color(t.on_fg));
                    });
                    ui.label(TAGLINE);
                });
            });
        });
    ui.add_space(10.0);
    widgets::text_segmented(
        ui,
        &mut s.tab,
        &[
            (Tab::About, "About"),
            (Tab::Licence, "Licence"),
            (Tab::ThirdParty, "Third party"),
            (Tab::Build, "Build"),
        ],
    );
    ui.add_space(10.0);
    egui::ScrollArea::vertical()
        .max_height(250.0)
        .min_scrolled_height(250.0)
        .auto_shrink([false, false])
        .show(ui, |ui| match s.tab {
            Tab::About => {
                ui.label(
                    "Shows a git repository's history as TortoiseGit's revision graph does: \
                     branches, tags and merges, without the commits in between.",
                );
                ui.add_space(10.0);
                ui.label(COPYRIGHT);
                ui.label("Licensed under the GNU GPL, version 3 only, with no warranty.");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    link(ui, "Website", WEBSITE);
                    ui.label("·");
                    link(ui, "Report a bug", ISSUES);
                    ui.label("·");
                    link(ui, "Privacy", PRIVACY);
                });
            }
            Tab::Licence => legal_text(ui),
            Tab::ThirdParty => {
                ui.label(
                    "parterre is built on open-source components, each under its own licence.",
                );
                ui.add_space(8.0);
                let _ = widgets::text_button(ui, "Open THIRD-PARTY-NOTICES.html");
            }
            Tab::Build => {
                egui::Grid::new("about-build")
                    .num_columns(2)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        for (k, v) in [
                            ("parterre", crate::VERSION.to_owned()),
                            ("git", git_version()),
                            ("System", system()),
                        ] {
                            ui.label(RichText::new(k).weak());
                            ui.label(RichText::new(v).monospace());
                            ui.end_row();
                        }
                    });
                ui.add_space(8.0);
                if widgets::text_button(ui, "Copy").clicked() {
                    ui.ctx().copy_text(build_info());
                }
            }
        });
    ui.separator();
    dialogs::actions(ui, "", false, false, false)
}

/// C — a tinted column with the icon, like the About boxes of JetBrains' IDEs and installers.
fn side_panel(ui: &mut Ui, s: &mut State, icon: &TextureHandle) -> Answer {
    let t = widgets::tones(ui);
    ui.horizontal_top(|ui| {
        egui::Frame::new()
            .fill(t.on_bg)
            .corner_radius(10)
            .show(ui, |ui| {
                ui.set_width(170.0);
                ui.set_height(262.0);
                ui.vertical_centered(|ui| {
                    ui.add_space(75.0);
                    image(ui, icon, 112.0);
                });
            });
        ui.add_space(16.0);
        ui.vertical(|ui| {
            ui.add_space(2.0);
            ui.label(RichText::new("parterre").strong().size(26.0));
            ui.label(RichText::new(TAGLINE).weak());
            ui.add_space(12.0);
            egui::Grid::new("about-sheet")
                .num_columns(2)
                .spacing([14.0, 5.0])
                .show(ui, |ui| {
                    for (k, v) in [
                        ("Version", crate::VERSION.to_owned()),
                        ("git", git_version()),
                        ("System", system()),
                        ("Licence", "GPL-3.0-only".to_owned()),
                    ] {
                        ui.label(RichText::new(k).weak());
                        ui.horizontal(|ui| {
                            ui.label(v);
                            if k == "Version"
                                && widgets::copy_button(ui, s.copied, t.accent)
                                    .on_hover_text("Copy version, git and system")
                                    .clicked()
                            {
                                ui.ctx().copy_text(build_info());
                                s.copied = true;
                            }
                        });
                        ui.end_row();
                    }
                });
            ui.add_space(10.0);
            ui.label(COPYRIGHT);
            ui.label(RichText::new("This program comes with absolutely no warranty.").weak());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                link(ui, "Website", WEBSITE);
                ui.label("·");
                link(ui, "Report a bug", ISSUES);
                ui.label("·");
                link(ui, "Privacy", PRIVACY);
                ui.label("·");
                if ui.link("Third-party licences").clicked() {}
            });
        });
    });
    ui.add_space(8.0);
    ui.collapsing("Licence and notices", |ui| {
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .show(ui, legal_text);
    });
    ui.separator();
    dialogs::actions(ui, "", false, false, false)
}

/// The prototype's own switcher, not part of any variant.
fn switcher(ui: &mut Ui, s: &mut State) {
    let n = VARIANTS.len();
    let (left, right) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::ArrowLeft),
            i.key_pressed(egui::Key::ArrowRight),
        )
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space((ui.available_width() - 260.0).max(0.0) / 2.0);
        egui::Frame::new()
            .fill(Color32::from_rgb(0xe0, 0x1b, 0x84))
            .corner_radius(14)
            .inner_margin(egui::Margin::symmetric(10, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let white = |t: &str| RichText::new(t).color(Color32::WHITE).strong();
                    let prev = ui.add(egui::Button::new(white("◀")).frame(false)).clicked();
                    let (k, name) = VARIANTS[s.variant];
                    ui.label(white(&format!("PROTOTYPE  {k} — {name}")));
                    let next = ui.add(egui::Button::new(white("▶")).frame(false)).clicked();
                    if prev || left {
                        s.variant = (s.variant + n - 1) % n;
                    }
                    if next || right {
                        s.variant = (s.variant + 1) % n;
                    }
                });
            });
    });
}
