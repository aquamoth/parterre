//! *Legal* shows the "Appropriate Legal Notices" of GPL-3.0 section 5(d), which NOTICE requires
//! works based on parterre to keep showing.

use std::path::PathBuf;

use eframe::egui::{
    self, Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke, TextureHandle, Ui, Vec2,
};

use parterre_telemetry::Ring;

use crate::about::{self, CONTACT};
use crate::dialogs::{self, Answer, Dialog};
use crate::widgets;

const ICON_SIDE: f32 = 96.0;

#[derive(Default)]
pub struct About {
    open: bool,
    legal_open: bool,
    version_copied: bool,
    icon: Option<TextureHandle>,
    third_party_notices: Option<PathBuf>,
}

impl About {
    pub fn open(&mut self, ctx: &egui::Context) {
        if cfg!(target_os = "macos") {
            crate::usage::screen(ctx, egui::Id::new("about"), crate::usage::Screen::About);
            #[cfg(target_os = "macos")]
            crate::macos::about_panel();
            return;
        }
        if !self.open {
            self.third_party_notices = about::third_party_notices();
        }
        self.open = true;
    }

    /// Returns why a link didn't open. `ring` is what the installation hears of.
    pub fn show(&mut self, ctx: &egui::Context, ring: Ring) -> Option<String> {
        let mut error = None;
        if self.open {
            let icon = self.icon.get_or_insert_with(|| icon(ctx)).clone();
            let shown = Dialog::new("about", "About parterre")
                .screen(crate::usage::Screen::About)
                .width(380.0)
                .show(ctx, |ui| {
                    if let Some(e) = self.content(ui, &icon, ring) {
                        error = Some(e);
                    }
                    ui.separator();
                    dialogs::close_only(ui)
                });
            if shown.answer() == Answer::Cancel {
                self.open = false;
            }
        }
        if self.legal_open {
            self.legal_window(ctx);
        }
        error
    }

    fn content(&mut self, ui: &mut Ui, icon: &TextureHandle, ring: Ring) -> Option<String> {
        ui.vertical_centered(|ui| {
            ui.add_space(8.0);
            let rect = ui
                .add(egui::Image::new((icon.id(), Vec2::splat(ICON_SIDE))))
                .rect;
            // A faint rim, along the tile's own corners (114.5 of 512), sets it off a dark
            // background.
            if ui.visuals().dark_mode {
                ui.painter().rect_stroke(
                    rect,
                    CornerRadius::same((ICON_SIDE * 114.5 / 512.0) as u8),
                    Stroke::new(1.0, Color32::from_white_alpha(46)),
                    egui::StrokeKind::Inside,
                );
            }
            ui.add_space(6.0);
            ui.label(RichText::new("parterre").strong().size(22.0));
            ui.label(RichText::new(about::TAGLINE).weak());
            ui.add_space(6.0);
            self.version(ui);
            if let Some(heard) = hears_of(ring) {
                ui.add_space(2.0);
                ui.label(RichText::new(heard).small().weak());
            }
            ui.add_space(12.0);
        });
        let mut error = None;
        let mut open = |url: &str| error = crate::browser::open(url).err();
        match rows(
            ui,
            &[
                ("Website", ""),
                ("Report a bug", ""),
                ("Privacy", ""),
                ("Contact", CONTACT),
            ],
            "↗",
        ) {
            Some(0) => open(about::WEBSITE),
            Some(1) => open(about::ISSUES),
            Some(2) => open(about::PRIVACY),
            Some(_) => open(about::MAIL),
            None => {}
        }
        ui.add_space(10.0);
        if rows(ui, &[("Legal", "GPL-3.0-only")], "›").is_some() {
            self.legal_open = true;
        }
        if let Some(notices) = &self.third_party_notices {
            ui.add_space(10.0);
            if rows(ui, &[("Third-party licences", "")], "↗").is_some() {
                error = crate::browser::open_file(notices).err();
            }
        }
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            let small = |text: &str| RichText::new(text).small().weak();
            ui.label(small(&about::copyright()));
            ui.label(small("This program comes with absolutely no warranty."));
        });
        ui.add_space(4.0);
        error
    }

    fn version(&mut self, ui: &mut Ui) {
        let t = widgets::tones(ui);
        let text = if self.version_copied {
            "Copied"
        } else {
            crate::version()
        };
        let pill = egui::Button::new(RichText::new(text).color(t.on_fg))
            .fill(t.on_bg)
            .corner_radius(12);
        let response = ui
            .add(pill)
            .on_hover_text("Copy the version, git's version and the system");
        if response.clicked() {
            ui.ctx().copy_text(about::build_info());
            self.version_copied = true;
        } else if !response.hovered() {
            self.version_copied = false;
        }
    }

    fn legal_window(&mut self, ctx: &egui::Context) {
        let font = egui::TextStyle::Monospace.resolve(&ctx.global_style());
        let column = ctx.fonts_mut(|f| f.glyph_width(&font, '0'));
        // As wide as NOTICE's and the GPL's 80 columns.
        let shown = Dialog::new("about-legal", "Legal notices")
            .width((81.0 * column + 16.0).ceil())
            .resizable()
            .show(ctx, |ui| {
                dialogs::fields(ui, |ui| {
                    let text = |text: &str| RichText::new(text).monospace();
                    ui.add(egui::Label::new(text(about::NOTICE)).extend());
                    ui.collapsing("GNU General Public License, version 3", |ui| {
                        ui.add(egui::Label::new(text(about::LICENSE)).extend());
                    });
                });
                ui.separator();
                dialogs::close_only(ui)
            });
        if shown.answer() == Answer::Cancel {
            self.legal_open = false;
        }
    }
}

/// The line under the version: nothing for stable releases, which everyone hears of.
fn hears_of(ring: Ring) -> Option<String> {
    (ring != Ring::Stable).then(|| format!("Hears of {}", super::updates::builds(ring)))
}

fn icon(ctx: &egui::Context) -> TextureHandle {
    const SIDE: u32 = 256;
    let rgba = parterre_core::icon::render(SIDE);
    let side = SIDE as usize;
    let image = egui::ColorImage::from_rgba_unmultiplied([side, side], &rgba);
    ctx.load_texture("about-icon", image, egui::TextureOptions::LINEAR)
}

/// Returns the index of the row clicked.
fn rows(ui: &mut Ui, items: &[(&str, &str)], mark: &str) -> Option<usize> {
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
            for (i, (label, value)) in items.iter().enumerate() {
                let top = ui.available_rect_before_wrap().top();
                // A placeholder under the text, for the hover fill known only afterwards.
                let background = ui.painter().add(egui::Shape::Noop);
                let row = ui.horizontal(|ui| {
                    ui.set_min_height(40.0);
                    ui.add_space(12.0);
                    ui.label(*label);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(12.0);
                        ui.label(RichText::new(mark).weak());
                        ui.label(RichText::new(*value).weak());
                    });
                });
                let rect = row.response.rect;
                if i > 0 {
                    ui.painter()
                        .hline(rect.x_range(), top, Stroke::new(1.0, line));
                }
                let response =
                    ui.interact(rect, ui.id().with(("about-row", *label)), Sense::click());
                if response.hovered() {
                    let fill = egui::Shape::rect_filled(rect, CornerRadius::same(9), t.hover);
                    ui.painter().set(background, fill);
                }
                if response.clicked() {
                    clicked = Some(i);
                }
            }
        });
    clicked
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use eframe::egui::{self, Event, Modifiers, PointerButton, Rect, vec2};

    use parterre_telemetry::Ring;

    use super::{About, hears_of};
    use crate::app::tool_harness::{click_events, raw_input, run_frame, text_at};

    struct Screen {
        ctx: egui::Context,
        about: About,
        ring: Ring,
        time: f64,
        events: Vec<Event>,
        texts: Vec<(String, Rect)>,
    }

    impl Screen {
        fn new() -> Screen {
            Screen::hearing_of(Ring::Stable)
        }

        fn hearing_of(ring: Ring) -> Screen {
            let ctx = egui::Context::default();
            let mut about = About::default();
            about.open(&ctx);
            let mut screen = Screen {
                ctx,
                about,
                ring,
                time: 0.0,
                events: Vec::new(),
                texts: Vec::new(),
            };
            screen.frame();
            screen.frame();
            screen
        }

        fn frame(&mut self) {
            self.time += 1.0 / 60.0;
            let input = egui::RawInput {
                time: Some(self.time),
                ..raw_input(vec2(1600.0, 1000.0), std::mem::take(&mut self.events))
            };
            let (about, ring) = (&mut self.about, self.ring);
            self.texts = run_frame(&self.ctx, input, |ui| {
                assert_eq!(about.show(ui.ctx(), ring), None);
            });
            let clashes: Vec<_> = self
                .texts
                .iter()
                // Not the licence's own "use of".
                .filter(|(t, _)| {
                    ["First", "Second", "Double"]
                        .iter()
                        .any(|w| t.starts_with(&format!("{w} use of")))
                        || t.contains("is above this")
                })
                .collect();
            assert!(clashes.is_empty(), "egui reports id clashes: {clashes:?}");
        }

        fn shows(&self, text: &str) -> bool {
            self.texts.iter().any(|(t, _)| t == text)
        }

        fn click(&mut self, text: &str) {
            let at = text_at(&self.texts, text);
            for events in click_events(at, PointerButton::Primary, Modifiers::NONE) {
                self.events.extend(events);
                self.frame();
            }
            self.frame();
        }
    }

    #[test]
    fn about_shows_the_links_and_the_copyright_and_legal_opens_the_notices() {
        let mut s = Screen::new();
        for text in [
            "parterre",
            crate::about::TAGLINE,
            crate::version(),
            "Website",
            "Report a bug",
            "Privacy",
            "parterre@trustfall.se",
            "GPL-3.0-only",
            "This program comes with absolutely no warranty.",
        ] {
            assert!(s.shows(text), "{text}: {:?}", s.texts);
        }
        assert!(s.shows(&crate::about::copyright()));
        // A test binary has no third-party notices beside it.
        assert!(!s.shows("Third-party licences"));
        assert!(!s.texts.iter().any(|(t, _)| t.contains("TortoiseGit")));

        s.click("Legal");
        assert!(s.shows("Legal notices"), "{:?}", s.texts);
        assert!(s.texts.iter().any(|(t, _)| t.contains("WITHOUT")));
        s.click("GNU General Public License, version 3");
        for _ in 0..30 {
            s.frame();
        }
        let licence = |t: &String| t.contains("GNU GENERAL PUBLIC LICENSE");
        assert!(s.texts.iter().any(|(t, _)| licence(t)));
    }

    #[test]
    fn under_the_version_is_what_the_installation_hears_of_unless_stable() {
        assert_eq!(hears_of(Ring::Stable), None);
        assert_eq!(hears_of(Ring::Rc).unwrap(), "Hears of release candidates");
        assert_eq!(hears_of(Ring::Canary).unwrap(), "Hears of canary builds");
        assert!(
            !Screen::new()
                .texts
                .iter()
                .any(|(t, _)| t.starts_with("Hears of"))
        );
        assert!(Screen::hearing_of(Ring::Nightly).shows("Hears of nightly builds"));
    }

    #[test]
    fn the_version_copies_what_a_bug_report_wants() {
        let mut s = Screen::new();
        s.click(crate::version());
        assert!(s.shows("Copied"), "{:?}", s.texts);
    }
}
