//! The menu bar on Windows and Linux: a row of parterre's own above the toolbar, as tall as
//! the toolbar's rows were (about 29 points; KDE's is 29, GNOME's 32, WinUI's 36), with the
//! menus in parterre's menu look (`crate::menu`). As in native menu bars, once a menu is open,
//! pointing at another title opens that one. A menu taller than the window scrolls.
//!
//! And as there, the keyboard: Alt+letter opens a menu by its underlined letter, and in a menu
//! a letter chooses the item it underlines; F10, or Alt pressed and let go alone, gives the bar
//! the keyboard. The arrow keys move between menus and items, Enter chooses, Esc closes. The
//! letters are underlined while Alt is held or the bar has the keyboard. Alt with Ctrl is
//! AltGr on Windows, typing characters: never the bar's.

use eframe::egui::{self, Color32, Event, Id, Key, Popup, RichText, Ui, WidgetText, vec2};

use super::{Command, Entry, Menu, access_keys, access_letter};
use crate::menu;
use crate::widgets;

/// The row's height.
pub const HEIGHT: f32 = 29.0;

/// The id of the popup of the menu titled `title`, for a script to open (`open menu:file`).
pub fn popup_id(title: &str) -> Id {
    Id::new(("menu-bar", title.to_lowercase())).with("popup")
}

/// Where the keyboard is in the bar, kept between frames.
#[derive(Clone, Debug, Default)]
struct Nav {
    /// The bar has the keyboard: a title is highlighted, and the letters are underlined.
    active: bool,
    /// The title highlighted, or whose menu is open.
    title: usize,
    /// While a menu is open: the entry highlighted in it, then in each submenu the keyboard
    /// opened.
    path: Vec<Option<usize>>,
    /// Alt is held.
    alt: bool,
    /// Alt went down with nothing else since: let go, it gives the bar the keyboard.
    alt_alone: bool,
}

fn nav_id() -> Id {
    Id::new("menu-bar-nav")
}

/// The entries a menu shows at the keyboard's depth: its own, or an open submenu's.
fn level<'a>(entries: &'a [Entry], path: &[Option<usize>]) -> &'a [Entry] {
    let mut entries = entries;
    for &index in path.iter().take(path.len().saturating_sub(1)) {
        match index.and_then(|i| entries.get(i)) {
            Some(Entry::Submenu(submenu)) => entries = &submenu.entries,
            _ => break,
        }
    }
    entries
}

/// An entry's label, if it is one the keyboard can choose.
fn label_of(entry: &Entry) -> Option<&str> {
    match entry {
        Entry::Item(item) => Some(&item.label),
        Entry::Submenu(submenu) => Some(&submenu.label),
        _ => None,
    }
}

/// Whether the keyboard can choose `entry`: an item or a submenu, not greyed out.
fn choosable(entry: &Entry) -> bool {
    match entry {
        Entry::Item(item) => item.enabled,
        Entry::Submenu(submenu) => submenu.enabled,
        _ => false,
    }
}

/// The entries' access keys: the index of each one's underlined character.
fn keys_of(entries: &[Entry]) -> Vec<Option<usize>> {
    let labels: Vec<Option<&str>> = entries.iter().map(label_of).collect();
    access_keys(&labels)
}

/// The next choosable entry after `from` (or the first), going `forward` or back, round.
fn step(entries: &[Entry], from: Option<usize>, forward: bool) -> Option<usize> {
    let n = entries.len();
    (1..=n)
        .map(|k| match (from, forward) {
            (Some(f), true) => (f + k) % n,
            (Some(f), false) => (f + n * 2 - k) % n,
            (None, true) => k - 1,
            (None, false) => n - k,
        })
        .find(|&i| choosable(&entries[i]))
}

/// The letter a key types, for access keys.
fn letter(key: Key) -> Option<String> {
    let name = key.name();
    (name.chars().count() == 1 && name.chars().all(char::is_alphanumeric))
        .then(|| name.to_lowercase())
}

/// Takes the keyboard's input for the bar this frame, before anything else sees it: what opens
/// and moves in the menus, and the command chosen, if any. Keys the bar doesn't want are left
/// for the window.
pub fn keys(ctx: &egui::Context, menus: &[Menu]) -> Option<Command> {
    if menus.is_empty() {
        return None;
    }
    let mut nav: Nav = ctx.data(|d| d.get_temp(nav_id())).unwrap_or_default();
    let open = menus
        .iter()
        .position(|m| Popup::is_id_open(ctx, popup_id(&m.title)));
    // A menu opened or closed with the pointer.
    match open {
        Some(i) if i != nav.title || nav.path.is_empty() => {
            nav.title = i;
            nav.path = vec![None];
        }
        None if !nav.path.is_empty() => nav.path.clear(),
        _ => {}
    }
    let n = menus.len();
    let title_keys = access_keys(
        &menus
            .iter()
            .map(|m| Some(m.title.as_str()))
            .collect::<Vec<_>>(),
    );
    let title_of = |typed: &str| {
        (0..n).find(|&i| access_letter(&menus[i].title, title_keys[i]).as_deref() == Some(typed))
    };
    let open_menu = |nav: &mut Nav, title: usize| {
        nav.title = title;
        nav.path = vec![step(&menus[title].entries, None, true)];
        nav.active = true;
        Popup::open_id(ctx, popup_id(&menus[title].title));
    };
    let close_menu = |nav: &mut Nav| {
        Popup::close_id(ctx, popup_id(&menus[nav.title].title));
        nav.path.clear();
    };
    let mut chosen = None;
    let (events, modifiers) = ctx.input(|i| (i.events.clone(), i.modifiers));
    let mut taken = Vec::new();
    // Anything else this frame: Alt wasn't alone.
    let mut other = false;
    for (index, event) in events.iter().enumerate() {
        if matches!(
            event,
            Event::Key { pressed: true, .. } | Event::Text(_) | Event::PointerButton { .. }
        ) {
            other = true;
        }
        match event {
            Event::Key {
                key,
                pressed: true,
                modifiers: m,
                ..
            } => {
                nav.alt_alone = false;
                if m.ctrl || m.command || m.mac_cmd {
                    continue;
                }
                let has_keyboard = nav.active || !nav.path.is_empty();
                if *key == Key::F10 && !m.any() {
                    if has_keyboard {
                        if !nav.path.is_empty() {
                            close_menu(&mut nav);
                        }
                        nav.active = false;
                    } else {
                        nav.active = true;
                        nav.title = 0;
                    }
                    taken.push(index);
                    continue;
                }
                if !has_keyboard {
                    // Alt+letter opens a menu by its letter.
                    if m.alt
                        && let Some(title) = letter(*key).as_deref().and_then(title_of)
                    {
                        open_menu(&mut nav, title);
                        taken.push(index);
                    }
                    continue;
                }
                let entries = &menus[nav.title].entries;
                let depth = nav.path.len();
                let here = level(entries, &nav.path);
                let cursor = nav.path.last().copied().flatten();
                let under = cursor.and_then(|c| here.get(c));
                match key {
                    Key::Escape if depth > 1 => {
                        nav.path.pop();
                    }
                    Key::Escape if depth == 1 => close_menu(&mut nav),
                    Key::Escape => nav.active = false,
                    Key::ArrowLeft if depth > 1 => {
                        nav.path.pop();
                    }
                    Key::ArrowRight if matches!(under, Some(Entry::Submenu(s)) if s.enabled) => {
                        if let Some(Entry::Submenu(s)) = under {
                            nav.path.push(step(&s.entries, None, true));
                        }
                    }
                    Key::ArrowLeft | Key::ArrowRight => {
                        let forward = *key == Key::ArrowRight;
                        let title = (nav.title + if forward { 1 } else { n - 1 }) % n;
                        if depth > 0 {
                            close_menu(&mut nav);
                            open_menu(&mut nav, title);
                        } else {
                            nav.title = title;
                        }
                    }
                    Key::ArrowDown | Key::ArrowUp | Key::Enter | Key::Space if depth == 0 => {
                        let title = nav.title;
                        open_menu(&mut nav, title);
                    }
                    Key::ArrowDown | Key::ArrowUp => {
                        let forward = *key == Key::ArrowDown;
                        if let Some(last) = nav.path.last_mut() {
                            *last = step(here, cursor, forward);
                        }
                    }
                    Key::Enter | Key::Space => match under {
                        Some(Entry::Item(item)) if item.enabled => {
                            chosen = Some(item.command.clone());
                            close_menu(&mut nav);
                            nav.active = false;
                        }
                        Some(Entry::Submenu(s)) if s.enabled => {
                            nav.path.push(step(&s.entries, None, true));
                        }
                        _ => {}
                    },
                    key => {
                        let Some(typed) = letter(*key) else { continue };
                        if depth == 0 {
                            if let Some(title) = title_of(&typed) {
                                open_menu(&mut nav, title);
                            }
                        } else {
                            let keys = keys_of(here);
                            let found = (0..here.len()).find(|&i| {
                                label_of(&here[i])
                                    .and_then(|l| access_letter(l, keys[i]))
                                    .as_deref()
                                    == Some(typed.as_str())
                            });
                            match found.map(|i| (i, &here[i])) {
                                Some((_, Entry::Item(item))) if item.enabled => {
                                    chosen = Some(item.command.clone());
                                    close_menu(&mut nav);
                                    nav.active = false;
                                }
                                Some((i, Entry::Submenu(s))) if s.enabled => {
                                    if let Some(last) = nav.path.last_mut() {
                                        *last = Some(i);
                                    }
                                    nav.path.push(step(&s.entries, None, true));
                                }
                                _ => {}
                            }
                        }
                    }
                }
                taken.push(index);
            }
            // What the keys typed is the bar's while it has the keyboard.
            Event::Text(_) => {
                nav.alt_alone = false;
                if nav.active || !nav.path.is_empty() {
                    taken.push(index);
                }
            }
            Event::PointerButton { pressed: true, .. } => {
                nav.alt_alone = false;
                if nav.path.is_empty() {
                    nav.active = false;
                }
            }
            _ => {}
        }
    }
    // Alt pressed and let go alone.
    let alt = modifiers.alt && !modifiers.ctrl && !modifiers.command;
    if alt && !nav.alt {
        nav.alt_alone = !other;
    } else if !alt && nav.alt && nav.alt_alone && !other {
        if nav.active || !nav.path.is_empty() {
            if !nav.path.is_empty() {
                close_menu(&mut nav);
            }
            nav.active = false;
        } else {
            nav.active = true;
            nav.title = 0;
        }
    }
    if !alt {
        nav.alt_alone = false;
    }
    nav.alt = alt;
    if !taken.is_empty() {
        ctx.input_mut(|i| {
            let mut k = 0;
            i.events.retain(|_| {
                let keep = !taken.contains(&k);
                k += 1;
                keep
            });
        });
    }
    ctx.data_mut(|d| d.insert_temp(nav_id(), nav));
    chosen
}

/// Draws the bar; returns the command chosen, if any.
pub fn show(ui: &mut Ui, menus: &[Menu]) -> Option<Command> {
    let mut chosen = None;
    let nav: Nav = ui.data(|d| d.get_temp(nav_id())).unwrap_or_default();
    let underline = nav.active || nav.alt;
    let open = menus
        .iter()
        .any(|m| Popup::is_id_open(ui.ctx(), popup_id(&m.title)));
    let title_keys = access_keys(
        &menus
            .iter()
            .map(|m| Some(m.title.as_str()))
            .collect::<Vec<_>>(),
    );
    ui.horizontal(|ui| {
        ui.set_height(HEIGHT);
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, m) in menus.iter().enumerate() {
            let id = popup_id(&m.title);
            let is_open = Popup::is_id_open(ui.ctx(), id);
            let highlighted = nav.active && nav.path.is_empty() && nav.title == i;
            let key = underline.then_some(title_keys[i]).flatten();
            let response = title(ui, m, is_open || highlighted, key);
            // Pointing at another title while one is open opens it instead.
            if open && !is_open && response.hovered() {
                Popup::open_id(ui.ctx(), id);
            }
            // The keyboard's place in this menu, while it is the open one.
            let path: &[Option<usize>] = if nav.title == i { &nav.path } else { &[] };
            Popup::menu(&response)
                .id(id)
                .style(menu::style)
                .gap(2.0)
                .show(|ui| {
                    crate::usage::menu(ui.ctx(), m.kind.usage());
                    menu::fit_window(ui, |ui| {
                        ui.set_min_width(menu::MIN_WIDTH);
                        let keyboard = Keyboard {
                            path,
                            underline,
                            steering: nav.active,
                            plain: false,
                        };
                        entries(ui, &m.entries, &keyboard, &mut chosen);
                    });
                });
        }
    });
    chosen
}

/// What the keyboard shows in a menu: the entry highlighted at each depth, and the letters.
struct Keyboard<'a> {
    path: &'a [Option<usize>],
    underline: bool,
    /// The keyboard moves in the menus: its submenus are open, and only those.
    steering: bool,
    /// A right-click menu: no column for check marks, so the items line up with plain buttons.
    plain: bool,
}

/// `list` as a right-click menu, in a popup already open; the command of the item clicked.
pub fn context(ui: &mut Ui, list: &[Entry]) -> Option<Command> {
    let mut chosen = None;
    let keyboard = Keyboard {
        path: &[],
        underline: false,
        steering: false,
        plain: true,
    };
    entries(ui, list, &keyboard, &mut chosen);
    chosen
}

/// `text` with its access key `key` underlined, in `color` (the widget's own if `None`).
fn underlined(
    ui: &Ui,
    text: &str,
    key: Option<usize>,
    color: Option<Color32>,
    strong: bool,
) -> WidgetText {
    let Some(key) = key else {
        let mut rich = RichText::new(text);
        if let Some(color) = color {
            rich = rich.color(color);
        }
        if strong {
            rich = rich.strong();
        }
        return rich.into();
    };
    let font = egui::TextStyle::Button.resolve(ui.style());
    let color = color.unwrap_or(if strong {
        ui.visuals().strong_text_color()
    } else {
        Color32::PLACEHOLDER
    });
    let plain = egui::TextFormat::simple(font, color);
    let marked = egui::TextFormat {
        underline: egui::Stroke::new(1.0, color),
        ..plain.clone()
    };
    let start = text.char_indices().nth(key).map_or(text.len(), |(b, _)| b);
    let end = text[start..]
        .chars()
        .next()
        .map_or(start, |c| start + c.len_utf8());
    let mut job = egui::text::LayoutJob::default();
    job.append(&text[..start], 0.0, plain.clone());
    job.append(&text[start..end], 0.0, marked);
    job.append(&text[end..], 0.0, plain);
    job.into()
}

/// A menu's title in the bar: framed while it is open, pointed at or highlighted.
fn title(ui: &mut Ui, m: &Menu, lit: bool, key: Option<usize>) -> egui::Response {
    let t = widgets::tones(ui);
    let color = m.accent.then_some(t.on_fg);
    let text = underlined(ui, &m.title, key, color, m.accent);
    let galley = text.into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    let size = vec2(galley.size().x + 20.0, HEIGHT - 4.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let id = Id::new(("menu-bar", m.title.to_lowercase()));
    let response = ui.interact(rect, id, egui::Sense::click());
    if lit || response.hovered() {
        let fill = if lit { t.press } else { t.hover };
        ui.painter().rect_filled(rect, 6.0, fill);
    }
    let color = color.unwrap_or(ui.visuals().text_color());
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    response
}

/// A menu's entries; sets `chosen` to the command of the item clicked.
fn entries(ui: &mut Ui, entries: &[Entry], keyboard: &Keyboard, chosen: &mut Option<Command>) {
    let keys = keys_of(entries);
    let cursor = keyboard.path.first().copied().flatten();
    for (index, entry) in entries.iter().enumerate() {
        let key = keyboard.underline.then_some(keys[index]).flatten();
        let highlighted = cursor == Some(index);
        match entry {
            Entry::Item(item) => {
                let t = widgets::tones(ui);
                let color = item.accent.then_some(t.on_fg);
                let label = underlined(ui, &item.label, key, color, item.accent);
                let shortcut = match (&item.shortcut, &item.detail) {
                    (Some(s), _) => s.label(),
                    (None, Some(detail)) => detail.clone(),
                    (None, None) => String::new(),
                };
                let response = ui
                    .add_enabled_ui(item.enabled, |ui| {
                        if highlighted {
                            lit(ui);
                        }
                        if keyboard.plain {
                            ui.add(egui::Button::new(label).shortcut_text(shortcut))
                        } else {
                            menu::item(ui, label, &shortcut, item.mark)
                        }
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
            Entry::Heading(text) => heading(ui, text, keyboard.plain),
            Entry::Submenu(submenu) => {
                // Open while the keyboard is in it; closed when the keyboard moves elsewhere.
                let inside = highlighted && keyboard.path.len() > 1;
                let open = keyboard.steering.then_some(inside);
                let deeper = Keyboard {
                    path: if inside { &keyboard.path[1..] } else { &[] },
                    underline: keyboard.underline,
                    steering: keyboard.steering,
                    plain: keyboard.plain,
                };
                let label = underlined(ui, &submenu.label, key, None, false);
                let shown = ui.add_enabled_ui(submenu.enabled, |ui| {
                    if highlighted {
                        lit(ui);
                    }
                    let content = |ui: &mut Ui| {
                        ui.set_min_width(menu::MIN_WIDTH);
                        self::entries(ui, &submenu.entries, &deeper, chosen);
                    };
                    if keyboard.plain {
                        menu::plain_submenu(ui, &submenu.label, content);
                    } else {
                        menu::submenu(ui, label, open, content);
                    }
                });
                if let Some(why) = &submenu.why {
                    shown.response.on_disabled_hover_text(why);
                }
            }
            Entry::System(_) => {}
        }
    }
}

/// The look of the entry the keyboard is on: as if pointed at.
fn lit(ui: &mut Ui) {
    let w = &mut ui.visuals_mut().widgets;
    w.inactive.weak_bg_fill = w.hovered.weak_bg_fill;
    w.inactive.corner_radius = w.hovered.corner_radius;
}

/// A small title over a group of items, lined up with their labels.
fn heading(ui: &mut Ui, text: &str, plain: bool) {
    let padding = ui.spacing().button_padding;
    ui.horizontal(|ui| {
        let mark = if plain {
            0.0
        } else {
            menu::MARK + ui.spacing().icon_spacing
        };
        ui.add_space(padding.x + mark);
        ui.label(RichText::new(text).small().weak());
    });
    ui.add_space(2.0);
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, Event, Pos2, Rect};

    use super::super::{Command, Entry, Item, Kind, Menu, Submenu};
    use super::{keys, show};
    use crate::app::tool_harness::collect;

    struct Bar {
        ctx: egui::Context,
        menus: Vec<Menu>,
        texts: Vec<(String, Rect)>,
        chosen: Option<Command>,
        events: Vec<Event>,
        modifiers: egui::Modifiers,
    }

    impl Bar {
        fn frame(&mut self) {
            let mut events = std::mem::take(&mut self.events);
            events.insert(0, Event::ModifiersChanged(self.modifiers));
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 600.0))),
                events,
                ..Default::default()
            };
            let menus = &self.menus;
            let mut chosen = None;
            let mut output = self.ctx.run_ui(input, |ui| {
                let typed = keys(ui.ctx(), menus);
                egui::Panel::top("bar").show(ui, |ui| chosen = show(ui, menus));
                chosen = chosen.take().or(typed);
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

        /// `key` pressed with `modifiers`, then two frames for what follows.
        fn key(&mut self, modifiers: egui::Modifiers, key: egui::Key) {
            self.events.push(Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            });
            self.frame();
            self.frame();
        }

        fn press(&mut self, key: egui::Key) {
            self.key(egui::Modifiers::NONE, key);
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
            modifiers: egui::Modifiers::NONE,
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

    #[test]
    fn alt_and_a_letter_open_a_menu_and_a_letter_chooses_in_it() {
        use egui::{Key, Modifiers};
        let mut b = bar();
        b.key(Modifiers::ALT, Key::F);
        assert!(b.shows("Open folder…"), "Alt+F opens File");
        b.press(Key::O);
        assert!(matches!(b.chosen, Some(Command::OpenFolder)));
        assert!(!b.shows("Open folder…"));
    }

    #[test]
    fn arrows_move_through_menus_and_submenus() {
        use egui::{Key, Modifiers};
        let mut b = bar();
        b.key(Modifiers::ALT, Key::F);
        // Past the separator, into the submenu, and its item.
        b.press(Key::ArrowDown);
        b.press(Key::ArrowRight);
        assert!(b.shows("Clear recent folders"));
        b.press(Key::ArrowLeft);
        b.frame();
        assert!(!b.shows("Clear recent folders"), "Left closes the submenu");
        b.press(Key::ArrowRight);
        b.press(Key::Enter);
        assert!(matches!(b.chosen, Some(Command::ClearRecent)));
    }

    #[test]
    fn f10_gives_the_bar_the_keyboard_and_esc_takes_it_back() {
        use egui::Key;
        let mut b = bar();
        b.press(Key::F10);
        b.press(Key::ArrowRight);
        b.press(Key::ArrowDown);
        assert!(b.shows("Zoom to fit"), "View opens");
        // Greyed out items are passed over: Down comes round to Zoom to fit again.
        b.press(Key::ArrowDown);
        b.press(Key::Enter);
        assert!(matches!(b.chosen, Some(Command::ZoomToFit)));
        b.chosen = None;
        b.press(Key::F10);
        b.press(Key::Enter);
        assert!(b.shows("Open folder…"));
        b.press(Key::Escape);
        assert!(!b.shows("Open folder…"));
        b.press(Key::Escape);
        // The bar has let go: a letter is no longer its.
        b.press(Key::F);
        assert!(!b.shows("Open folder…"));
    }

    #[test]
    fn alt_alone_gives_the_bar_the_keyboard_and_altgr_never_does() {
        use egui::{Key, Modifiers};
        let mut b = bar();
        b.modifiers = Modifiers::ALT;
        b.frame();
        b.modifiers = Modifiers::NONE;
        b.frame();
        b.press(Key::ArrowDown);
        assert!(
            b.shows("Open folder…"),
            "Alt let go alone, then Down opens File"
        );
        b.press(Key::Escape);
        b.press(Key::Escape);
        // AltGr (Ctrl+Alt on Windows) types characters.
        b.key(Modifiers::ALT | Modifiers::CTRL, Key::F);
        assert!(!b.shows("Open folder…"));
    }
}
