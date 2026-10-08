//! *About parterre* on Windows and Linux (#351): the icon, the version, the links, and
//! *Legal*, the "Appropriate Legal Notices" of GPL-3.0 section 5(d) in a window of their own.
//! NOTICE requires works based on parterre to keep showing them. macOS shows its own About
//! panel instead (`crate::macos::about_panel`).

use std::path::PathBuf;

use eframe::egui::{
    self, Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke, TextureHandle, Ui, vec2,
};

use crate::about::{self, CONTACT};
use crate::dialogs::{self, Answer, Dialog};
use crate::widgets;

/// Side of the icon, in points.
const ICON: f32 = 96.0;

/// The About dialog and its *Legal* window, while open.
#[derive(Default)]
pub struct About {
    open: bool,
    legal: bool,
    /// The version was copied, until the pointer leaves it.
    copied: bool,
    icon: Option<TextureHandle>,
    /// The installed third-party notices, looked for as the dialog opens.
    notices: Option<PathBuf>,
}

impl About {
    /// Opens the dialog; on macOS, the system's About panel.
    pub fn open(&mut self, ctx: &egui::Context) {
        if cfg!(target_os = "macos") {
            crate::usage::screen(ctx, egui::Id::new("about"), crate::usage::Screen::About);
            #[cfg(target_os = "macos")]
            crate::macos::about_panel();
            return;
        }
        if !self.open {
            self.notices = about::third_party_notices();
        }
        self.open = true;
    }

    /// Shows what is open. Returns why a link didn't open.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<String> {
        let mut error = None;
        if self.open {
            let icon = self.icon.get_or_insert_with(|| icon(ctx)).clone();
            let shown = Dialog::new("about", "About parterre")
                .screen(crate::usage::Screen::About)
                .width(380.0)
                .show(ctx, |ui| {
                    if let Some(e) = self.content(ui, &icon) {
                        error = Some(e);
                    }
                    ui.separator();
                    dialogs::actions(ui, "", false, false, false)
                });
            if shown.inner == Answer::Cancel || shown.should_close() {
                self.open = false;
            }
        }
        if self.legal {
            self.legal_window(ctx);
        }
        error
    }

    fn content(&mut self, ui: &mut Ui, icon: &TextureHandle) -> Option<String> {
        ui.vertical_centered(|ui| {
            ui.add_space(8.0);
            let rect = ui.add(egui::Image::new((icon.id(), vec2(ICON, ICON)))).rect;
            // The tile's own corners (114.5 of 512): a faint rim keeps it off a dark background.
            if ui.visuals().dark_mode {
                ui.painter().rect_stroke(
                    rect,
                    CornerRadius::same((ICON * 114.5 / 512.0) as u8),
                    Stroke::new(1.0, Color32::from_white_alpha(46)),
                    egui::StrokeKind::Inside,
                );
            }
            ui.add_space(6.0);
            ui.label(RichText::new("parterre").strong().size(22.0));
            ui.label(RichText::new(about::TAGLINE).weak());
            ui.add_space(6.0);
            self.version(ui);
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
            self.legal = true;
        }
        if let Some(notices) = &self.notices {
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

    /// The version as a pill, which copies what a bug report wants to know.
    fn version(&mut self, ui: &mut Ui) {
        let t = widgets::tones(ui);
        let text = if self.copied {
            "Copied"
        } else {
            crate::VERSION
        };
        let pill = egui::Button::new(RichText::new(text).color(t.on_fg))
            .fill(t.on_bg)
            .corner_radius(12);
        let response = ui
            .add(pill)
            .on_hover_text("Copy the version, git's version and the system");
        if response.clicked() {
            ui.ctx().copy_text(about::build_info());
            self.copied = true;
        } else if !response.hovered() {
            self.copied = false;
        }
    }

    /// NOTICE and the GPL, as wide as their 80 columns.
    fn legal_window(&mut self, ctx: &egui::Context) {
        let font = egui::TextStyle::Monospace.resolve(&ctx.global_style());
        let column = ctx.fonts_mut(|f| f.glyph_width(&font, '0'));
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
                dialogs::actions(ui, "", false, false, false)
            });
        if shown.inner == Answer::Cancel || shown.should_close() {
            self.legal = false;
        }
    }
}

/// The app icon, sharp up to twice [`ICON`]'s pixels.
fn icon(ctx: &egui::Context) -> TextureHandle {
    let size = 256;
    let rgba = parterre_core::icon::render(size as u32);
    let image = egui::ColorImage::from_rgba_unmultiplied([size, size], &rgba);
    ctx.load_texture("about-icon", image, egui::TextureOptions::LINEAR)
}

/// Rows on a rounded background, as in the settings window: a label, a value and `mark` on the
/// right. Returns the clicked one.
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
                // Painted once the row knows it is hovered, under its text.
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
    use eframe::egui::{self, Event, Pos2, Rect};

    use super::About;
    use crate::app::tool_harness::collect;

    struct Screen {
        ctx: egui::Context,
        about: About,
        time: f64,
        events: Vec<Event>,
        texts: Vec<(String, Rect)>,
    }

    impl Screen {
        fn new() -> Screen {
            let ctx = egui::Context::default();
            let mut about = About::default();
            about.open(&ctx);
            let mut screen = Screen {
                ctx,
                about,
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
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
                time: Some(self.time),
                events: std::mem::take(&mut self.events),
                ..Default::default()
            };
            let about = &mut self.about;
            let mut output = self.ctx.run_ui(input, |ui| {
                assert_eq!(about.show(ui.ctx()), None);
            });
            output.textures_delta.clear();
            self.texts.clear();
            for clipped in &output.shapes {
                collect(&clipped.shape, &mut self.texts);
            }
            let clashes: Vec<_> = self
                .texts
                .iter()
                // The licence's own text says "use of".
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
            let at = self
                .texts
                .iter()
                .find(|(t, _)| t == text)
                .unwrap_or_else(|| panic!("no {text:?} on screen: {:?}", self.texts))
                .1
                .center();
            for pressed in [true, false] {
                self.events.push(Event::PointerMoved(at));
                self.events.push(Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
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
            crate::VERSION,
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
        // Unfolded, in the window's scrolled view.
        for _ in 0..30 {
            s.frame();
        }
        let licence = |t: &String| t.contains("GNU GENERAL PUBLIC LICENSE");
        assert!(s.texts.iter().any(|(t, _)| licence(t)));
    }

    #[test]
    fn the_version_copies_what_a_bug_report_wants() {
        let mut s = Screen::new();
        s.click(crate::VERSION);
        assert!(s.shows("Copied"), "{:?}", s.texts);
    }
}
