//! The menu bar on Windows and Linux: a row of parterre's own above the toolbar, as tall as
//! the toolbar's rows were (about 29 points; KDE's is 29, GNOME's 32, WinUI's 36), with the
//! menus in parterre's menu look (`crate::menu`). As in native menu bars, once a menu is open,
//! pointing at another title opens that one. A menu taller than the window scrolls.

use eframe::egui::{self, Id, Popup, RichText, Ui, vec2};

use super::{Command, Entry, Menu};
use crate::menu;
use crate::widgets;

/// The row's height.
pub const HEIGHT: f32 = 29.0;

/// The id of the popup of the menu titled `title`, for a script to open (`open menu:file`).
pub fn popup_id(title: &str) -> Id {
    Id::new(("menu-bar", title.to_lowercase())).with("popup")
}

/// Draws the bar; returns the command chosen, if any.
pub fn show(ui: &mut Ui, menus: &[Menu]) -> Option<Command> {
    let mut chosen = None;
    let open = menus
        .iter()
        .any(|m| Popup::is_id_open(ui.ctx(), popup_id(&m.title)));
    ui.horizontal(|ui| {
        ui.set_height(HEIGHT);
        ui.spacing_mut().item_spacing.x = 0.0;
        for m in menus {
            let id = popup_id(&m.title);
            let is_open = Popup::is_id_open(ui.ctx(), id);
            let response = title(ui, m, is_open);
            // Pointing at another title while one is open opens it instead.
            if open && !is_open && response.hovered() {
                Popup::open_id(ui.ctx(), id);
            }
            Popup::menu(&response)
                .id(id)
                .style(menu::style)
                .gap(2.0)
                .show(|ui| {
                    crate::usage::menu(ui.ctx(), m.kind.usage());
                    menu::fit_window(ui, |ui| {
                        ui.set_min_width(menu::MIN_WIDTH);
                        entries(ui, &m.entries, &mut chosen);
                    });
                });
        }
    });
    chosen
}

/// A menu's title in the bar: framed while it is open or pointed at.
fn title(ui: &mut Ui, m: &Menu, open: bool) -> egui::Response {
    let t = widgets::tones(ui);
    let text = if m.accent {
        RichText::new(&m.title).color(t.on_fg).strong()
    } else {
        RichText::new(&m.title)
    };
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    let size = vec2(galley.size().x + 20.0, HEIGHT - 4.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let id = Id::new(("menu-bar", m.title.to_lowercase()));
    let response = ui.interact(rect, id, egui::Sense::click());
    if open || response.hovered() {
        let fill = if open { t.press } else { t.hover };
        ui.painter().rect_filled(rect, 6.0, fill);
    }
    let color = if m.accent {
        t.on_fg
    } else {
        ui.visuals().text_color()
    };
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    response
}

/// A menu's entries; sets `chosen` to the command of the item clicked.
fn entries(ui: &mut Ui, entries: &[Entry], chosen: &mut Option<Command>) {
    for entry in entries {
        match entry {
            Entry::Item(item) => {
                let t = widgets::tones(ui);
                let label = if item.accent {
                    RichText::new(&item.label).color(t.on_fg).strong()
                } else {
                    RichText::new(&item.label)
                };
                let shortcut = match (&item.shortcut, &item.detail) {
                    (Some(s), _) => s.label(),
                    (None, Some(detail)) => detail.clone(),
                    (None, None) => String::new(),
                };
                let response = ui
                    .add_enabled_ui(item.enabled, |ui| {
                        menu::item(ui, label, &shortcut, item.mark)
                    })
                    .inner;
                let response = match &item.tip {
                    Some(tip) => response.on_hover_text(tip),
                    None => response,
                };
                let response = match &item.why {
                    Some(why) => response.on_disabled_hover_text(why),
                    None => response,
                };
                if response.clicked() {
                    *chosen = Some(item.command.clone());
                    ui.close();
                }
            }
            Entry::Separator => menu::separator(ui),
            Entry::Heading(text) => heading(ui, text),
            Entry::Submenu(submenu) => {
                let shown = ui.add_enabled_ui(submenu.enabled, |ui| {
                    menu::submenu(ui, &submenu.label, |ui| {
                        ui.set_min_width(menu::MIN_WIDTH);
                        self::entries(ui, &submenu.entries, chosen);
                    });
                });
                if let Some(why) = &submenu.why {
                    shown.response.on_disabled_hover_text(why);
                }
            }
            Entry::System(_) => {}
        }
    }
}

/// A small title over a group of items, lined up with their labels.
fn heading(ui: &mut Ui, text: &str) {
    let padding = ui.spacing().button_padding;
    ui.horizontal(|ui| {
        ui.add_space(padding.x + menu::MARK + ui.spacing().icon_spacing);
        ui.label(RichText::new(text).small().weak());
    });
    ui.add_space(2.0);
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, Event, Pos2, Rect};

    use super::super::{Command, Entry, Item, Kind, Menu, Submenu};
    use super::show;
    use crate::app::tool_harness::collect;

    struct Bar {
        ctx: egui::Context,
        menus: Vec<Menu>,
        texts: Vec<(String, Rect)>,
        chosen: Option<Command>,
        events: Vec<Event>,
    }

    impl Bar {
        fn frame(&mut self) {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 600.0))),
                events: std::mem::take(&mut self.events),
                ..Default::default()
            };
            let menus = &self.menus;
            let mut chosen = None;
            let mut output = self.ctx.run_ui(input, |ui| {
                egui::Panel::top("bar").show(ui, |ui| chosen = show(ui, menus));
            });
            output.textures_delta.clear();
            if chosen.is_some() {
                self.chosen = chosen;
            }
            self.texts.clear();
            for clipped in &output.shapes {
                collect(&clipped.shape, &mut self.texts);
            }
            let clashes: Vec<_> = self
                .texts
                .iter()
                .filter(|(t, _)| t.contains("use of") || t.contains("is above this"))
                .collect();
            assert!(clashes.is_empty(), "egui reports id clashes: {clashes:?}");
        }

        fn at(&self, text: &str) -> Pos2 {
            self.texts
                .iter()
                .find(|(t, _)| t == text)
                .unwrap_or_else(|| panic!("no {text:?} on screen: {:?}", self.texts))
                .1
                .center()
        }

        fn hover(&mut self, text: &str) {
            let at = self.at(text);
            for _ in 0..3 {
                self.events.push(Event::PointerMoved(at));
                self.frame();
            }
        }

        fn click(&mut self, text: &str) {
            let at = self.at(text);
            self.events.push(Event::PointerMoved(at));
            self.frame();
            for pressed in [true, false] {
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

        fn shows(&self, text: &str) -> bool {
            self.texts.iter().any(|(t, _)| t == text)
        }
    }

    fn bar() -> Bar {
        let menu = |kind, title: &str, entries| Menu {
            kind,
            title: title.to_owned(),
            entries,
            accent: false,
        };
        let mut bar = Bar {
            ctx: egui::Context::default(),
            menus: vec![
                menu(
                    Kind::File,
                    "File",
                    vec![
                        Item::new("Open folder…", Command::OpenFolder).into(),
                        Entry::Separator,
                        Submenu::new(
                            "Open recent",
                            vec![Item::new("Clear recent folders", Command::ClearRecent).into()],
                        )
                        .into(),
                    ],
                ),
                menu(
                    Kind::View,
                    "View",
                    vec![
                        Entry::Heading("Detail".into()),
                        Item::new("Zoom to fit", Command::ZoomToFit).into(),
                        Item::new("Go to HEAD", Command::GoToHead)
                            .enabled(false)
                            .into(),
                    ],
                ),
            ],
            texts: Vec::new(),
            chosen: None,
            events: Vec::new(),
        };
        bar.frame();
        bar
    }

    #[test]
    fn a_click_opens_a_menu_and_an_item_gives_its_command() {
        let mut b = bar();
        assert!(!b.shows("Open folder…"));
        b.click("File");
        assert!(b.shows("Open folder…"));
        b.click("Open folder…");
        assert!(matches!(b.chosen, Some(Command::OpenFolder)));
        assert!(!b.shows("Open folder…"), "the menu closes");
    }

    #[test]
    fn pointing_at_another_title_opens_its_menu() {
        let mut b = bar();
        b.click("File");
        b.hover("View");
        assert!(b.shows("Zoom to fit"));
        assert!(b.shows("Detail"));
        assert!(!b.shows("Open folder…"));
        // Greyed out: nothing.
        b.click("Go to HEAD");
        assert!(b.chosen.is_none());
    }

    #[test]
    fn submenus_open_on_hover() {
        let mut b = bar();
        b.click("File");
        b.hover("Open recent");
        b.click("Clear recent folders");
        assert!(matches!(b.chosen, Some(Command::ClearRecent)));
    }
}
