//! As in native menu bars, once a menu is open, pointing at another title opens that one. And as
//! there, the keyboard: Alt+letter opens a menu by its underlined letter, and in a menu a letter
//! chooses the item it underlines; F10, or Alt pressed and let go alone, gives the bar the
//! keyboard. The arrow keys move between menus and items, Enter chooses, Esc closes.

use eframe::egui::{self, Color32, Event, Id, Key, Popup, RichText, Ui, WidgetText};

use super::{Command, Entry, Menu, access_keys, access_letter};
use crate::menu;
use crate::widgets;

// As tall as the toolbar's rows were. KDE's is 29, GNOME's 32, WinUI's 36.
#[cfg(not(target_os = "macos"))]
const BAR_HEIGHT: f32 = 29.0;

fn title_id(title: &str) -> Id {
    Id::new(("menu-bar", title.to_lowercase()))
}

pub fn popup_id(title: &str) -> Id {
    title_id(title).with("popup")
}

/// Where the keyboard is in the bar, kept between frames.
#[derive(Clone, Debug, Default)]
struct Nav {
    /// A title is highlighted and the letters are underlined.
    bar_focused: bool,
    /// The menu highlighted, or open.
    menu: usize,
    /// While a menu is open: the entry highlighted in it, then in each submenu the keyboard
    /// opened.
    path: Vec<Option<usize>>,
    alt_held: bool,
    /// Alt went down with nothing else since: let go, it gives the bar the keyboard.
    alt_alone: bool,
}

impl Nav {
    fn has_keyboard(&self) -> bool {
        self.bar_focused || !self.path.is_empty()
    }
}

fn nav_id() -> Id {
    Id::new("menu-bar-nav")
}

/// The entries of the innermost submenu `path` opened.
fn entries_at_cursor<'a>(entries: &'a [Entry], path: &[Option<usize>]) -> &'a [Entry] {
    let mut entries = entries;
    for &index in path.iter().take(path.len().saturating_sub(1)) {
        match index.and_then(|i| entries.get(i)) {
            Some(Entry::Submenu(submenu)) => entries = &submenu.entries,
            _ => break,
        }
    }
    entries
}

fn label_of(entry: &Entry) -> Option<&str> {
    match entry {
        Entry::Item(item) => Some(&item.label),
        Entry::Submenu(submenu) => Some(&submenu.label),
        _ => None,
    }
}

fn choosable(entry: &Entry) -> bool {
    match entry {
        Entry::Item(item) => item.enabled,
        Entry::Submenu(submenu) => submenu.enabled,
        _ => false,
    }
}

fn access_keys_of(entries: &[Entry]) -> Vec<Option<usize>> {
    let labels: Vec<Option<&str>> = entries.iter().map(label_of).collect();
    access_keys(&labels)
}

fn title_access_keys(menus: &[Menu]) -> Vec<Option<usize>> {
    let titles: Vec<Option<&str>> = menus.iter().map(|m| Some(m.title.as_str())).collect();
    access_keys(&titles)
}

/// After `from`, or from the start, wrapping round.
fn next_choosable(entries: &[Entry], from: Option<usize>, forward: bool) -> Option<usize> {
    let n = entries.len();
    (1..=n)
        .map(|k| match (from, forward) {
            (Some(f), true) => (f + k) % n,
            (Some(f), false) => (f + n - k) % n,
            (None, true) => k - 1,
            (None, false) => n - k,
        })
        .find(|&i| choosable(&entries[i]))
}

fn typed_letter(key: Key) -> Option<String> {
    let name = key.name();
    (name.chars().count() == 1 && name.chars().all(char::is_alphanumeric))
        .then(|| name.to_lowercase())
}

/// Call before anything else reads the input: the keys the bar takes are removed from it.
pub fn keys(ctx: &egui::Context, menus: &[Menu]) -> Option<Command> {
    if menus.is_empty() {
        return None;
    }
    let mut nav: Nav = ctx.data(|d| d.get_temp(nav_id())).unwrap_or_default();
    // Follow a menu opened or closed with the pointer.
    match menus
        .iter()
        .position(|m| Popup::is_id_open(ctx, popup_id(&m.title)))
    {
        Some(i) if i != nav.menu || nav.path.is_empty() => {
            nav.menu = i;
            nav.path = vec![None];
        }
        None if !nav.path.is_empty() => nav.path.clear(),
        _ => {}
    }
    let n = menus.len();
    let title_keys = title_access_keys(menus);
    let menu_of_letter = |typed: &str| {
        (0..n).find(|&i| access_letter(&menus[i].title, title_keys[i]).as_deref() == Some(typed))
    };
    let open_menu = |nav: &mut Nav, menu: usize| {
        nav.menu = menu;
        nav.path = vec![next_choosable(&menus[menu].entries, None, true)];
        nav.bar_focused = true;
        Popup::open_id(ctx, popup_id(&menus[menu].title));
    };
    let close_menu = |nav: &mut Nav| {
        Popup::close_id(ctx, popup_id(&menus[nav.menu].title));
        nav.path.clear();
    };
    let release_keyboard = |nav: &mut Nav| {
        if !nav.path.is_empty() {
            close_menu(nav);
        }
        nav.bar_focused = false;
    };
    let toggle_keyboard = |nav: &mut Nav| {
        if nav.has_keyboard() {
            release_keyboard(nav);
        } else {
            nav.bar_focused = true;
            nav.menu = 0;
        }
    };
    let mut chosen = None;
    let (events, modifiers) = ctx.input(|i| (i.events.clone(), i.modifiers));
    let mut taken = Vec::new();
    let mut other_input = false;
    for (index, event) in events.iter().enumerate() {
        if matches!(
            event,
            Event::Key { pressed: true, .. } | Event::Text(_) | Event::PointerButton { .. }
        ) {
            other_input = true;
        }
        match event {
            Event::Key {
                key,
                pressed: true,
                modifiers: m,
                ..
            } => {
                nav.alt_alone = false;
                // Ctrl+Alt is AltGr on Windows, typing characters.
                if m.ctrl || m.command || m.mac_cmd {
                    continue;
                }
                if *key == Key::F10 && !m.any() {
                    toggle_keyboard(&mut nav);
                    taken.push(index);
                    continue;
                }
                if !nav.has_keyboard() {
                    if m.alt
                        && let Some(menu) = typed_letter(*key).as_deref().and_then(menu_of_letter)
                    {
                        open_menu(&mut nav, menu);
                        taken.push(index);
                    }
                    continue;
                }
                let depth = nav.path.len();
                let here = entries_at_cursor(&menus[nav.menu].entries, &nav.path);
                let cursor = nav.path.last().copied().flatten();
                let under = cursor.and_then(|c| here.get(c));
                match key {
                    Key::Escape if depth > 1 => {
                        nav.path.pop();
                    }
                    Key::Escape if depth == 1 => close_menu(&mut nav),
                    Key::Escape => nav.bar_focused = false,
                    Key::ArrowLeft if depth > 1 => {
                        nav.path.pop();
                    }
                    Key::ArrowRight if matches!(under, Some(Entry::Submenu(s)) if s.enabled) => {
                        if let Some(Entry::Submenu(s)) = under {
                            nav.path.push(next_choosable(&s.entries, None, true));
                        }
                    }
                    Key::ArrowLeft | Key::ArrowRight => {
                        let forward = *key == Key::ArrowRight;
                        let menu = (nav.menu + if forward { 1 } else { n - 1 }) % n;
                        if depth > 0 {
                            close_menu(&mut nav);
                            open_menu(&mut nav, menu);
                        } else {
                            nav.menu = menu;
                        }
                    }
                    Key::ArrowDown | Key::ArrowUp | Key::Enter | Key::Space if depth == 0 => {
                        let menu = nav.menu;
                        open_menu(&mut nav, menu);
                    }
                    Key::ArrowDown | Key::ArrowUp => {
                        let forward = *key == Key::ArrowDown;
                        if let Some(last) = nav.path.last_mut() {
                            *last = next_choosable(here, cursor, forward);
                        }
                    }
                    Key::Enter | Key::Space => match under {
                        Some(Entry::Item(item)) if item.enabled => {
                            chosen = Some(item.command.clone());
                            release_keyboard(&mut nav);
                        }
                        Some(Entry::Submenu(s)) if s.enabled => {
                            nav.path.push(next_choosable(&s.entries, None, true));
                        }
                        _ => {}
                    },
                    key => {
                        let Some(typed) = typed_letter(*key) else {
                            continue;
                        };
                        if depth == 0 {
                            if let Some(menu) = menu_of_letter(&typed) {
                                open_menu(&mut nav, menu);
                            }
                        } else {
                            let keys = access_keys_of(here);
                            let found = (0..here.len()).find(|&i| {
                                label_of(&here[i])
                                    .and_then(|l| access_letter(l, keys[i]))
                                    .as_deref()
                                    == Some(typed.as_str())
                            });
                            match found.map(|i| (i, &here[i])) {
                                Some((_, Entry::Item(item))) if item.enabled => {
                                    chosen = Some(item.command.clone());
                                    release_keyboard(&mut nav);
                                }
                                Some((i, Entry::Submenu(s))) if s.enabled => {
                                    if let Some(last) = nav.path.last_mut() {
                                        *last = Some(i);
                                    }
                                    nav.path.push(next_choosable(&s.entries, None, true));
                                }
                                _ => {}
                            }
                        }
                    }
                }
                taken.push(index);
            }
            Event::Text(_) => {
                nav.alt_alone = false;
                if nav.has_keyboard() {
                    taken.push(index);
                }
            }
            Event::PointerButton { pressed: true, .. } => {
                nav.alt_alone = false;
                if nav.path.is_empty() {
                    nav.bar_focused = false;
                }
            }
            _ => {}
        }
    }
    let alt_held = modifiers.alt && !modifiers.ctrl && !modifiers.command;
    if alt_held && !nav.alt_held {
        nav.alt_alone = !other_input;
    } else if !alt_held && nav.alt_held && nav.alt_alone && !other_input {
        toggle_keyboard(&mut nav);
    }
    if !alt_held {
        nav.alt_alone = false;
    }
    nav.alt_held = alt_held;
    if !taken.is_empty() {
        ctx.input_mut(|i| {
            let mut index = 0;
            i.events.retain(|_| {
                let keep = !taken.contains(&index);
                index += 1;
                keep
            });
        });
    }
    ctx.data_mut(|d| d.insert_temp(nav_id(), nav));
    chosen
}

#[cfg(not(target_os = "macos"))]
pub fn show(ui: &mut Ui, menus: &[Menu]) -> Option<Command> {
    let mut chosen = None;
    let nav: Nav = ui.data(|d| d.get_temp(nav_id())).unwrap_or_default();
    let underline = nav.bar_focused || nav.alt_held;
    let any_open = menus
        .iter()
        .any(|m| Popup::is_id_open(ui.ctx(), popup_id(&m.title)));
    let title_keys = title_access_keys(menus);
    ui.horizontal(|ui| {
        ui.set_height(BAR_HEIGHT);
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, m) in menus.iter().enumerate() {
            let id = popup_id(&m.title);
            let is_open = Popup::is_id_open(ui.ctx(), id);
            let highlighted = nav.bar_focused && nav.path.is_empty() && nav.menu == i;
            let key = underline.then_some(title_keys[i]).flatten();
            let response = title(ui, m, is_open || highlighted, key);
            if any_open && !is_open && response.hovered() {
                Popup::open_id(ui.ctx(), id);
            }
            let path: &[Option<usize>] = if nav.menu == i { &nav.path } else { &[] };
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
                            steering: nav.bar_focused,
                            context_menu: false,
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
    context_menu: bool,
}

/// `list` in a popup already open.
pub fn context(ui: &mut Ui, list: &[Entry]) -> Option<Command> {
    let mut chosen = None;
    let keyboard = Keyboard {
        path: &[],
        underline: false,
        steering: false,
        context_menu: true,
    };
    entries(ui, list, &keyboard, &mut chosen);
    chosen
}

/// `color` `None` is the widget's own.
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

#[cfg(not(target_os = "macos"))]
fn title(ui: &mut Ui, m: &Menu, highlighted: bool, key: Option<usize>) -> egui::Response {
    let t = widgets::tones(ui);
    let color = m.accent.then_some(t.on_fg);
    let text = underlined(ui, &m.title, key, color, m.accent);
    let galley = text.into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    let size = egui::vec2(galley.size().x + 20.0, BAR_HEIGHT - 4.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let response = ui.interact(rect, title_id(&m.title), egui::Sense::click());
    if highlighted || response.hovered() {
        let fill = if highlighted { t.press } else { t.hover };
        ui.painter().rect_filled(rect, 6.0, fill);
    }
    let color = color.unwrap_or(ui.visuals().text_color());
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    response
}

fn entries(ui: &mut Ui, entries: &[Entry], keyboard: &Keyboard, chosen: &mut Option<Command>) {
    let keys = access_keys_of(entries);
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
                            look_hovered(ui);
                        }
                        if keyboard.context_menu {
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
                let response = match &item.why_disabled {
                    Some(why) => response.on_disabled_hover_text(why),
                    None => response,
                };
                if response.clicked() {
                    *chosen = Some(item.command.clone());
                    ui.close();
                }
            }
            Entry::Separator => menu::separator(ui),
            Entry::Heading(text) => heading(ui, text, keyboard.context_menu),
            Entry::Submenu(submenu) => {
                let inside = highlighted && keyboard.path.len() > 1;
                let open = keyboard.steering.then_some(inside);
                let deeper = Keyboard {
                    path: if inside { &keyboard.path[1..] } else { &[] },
                    ..*keyboard
                };
                let label = underlined(ui, &submenu.label, key, None, false);
                let shown = ui.add_enabled_ui(submenu.enabled, |ui| {
                    if highlighted {
                        look_hovered(ui);
                    }
                    let content = |ui: &mut Ui| {
                        ui.set_min_width(menu::MIN_WIDTH);
                        self::entries(ui, &submenu.entries, &deeper, chosen);
                    };
                    if keyboard.context_menu {
                        menu::plain_submenu(ui, &submenu.label, content);
                    } else {
                        menu::submenu(ui, label, open, content);
                    }
                });
                if let Some(why) = &submenu.why_disabled {
                    shown.response.on_disabled_hover_text(why);
                }
            }
            Entry::System(_) => {}
        }
    }
}

fn look_hovered(ui: &mut Ui) {
    let w = &mut ui.visuals_mut().widgets;
    w.inactive.weak_bg_fill = w.hovered.weak_bg_fill;
    w.inactive.corner_radius = w.hovered.corner_radius;
}

/// Lined up with the items' labels.
fn heading(ui: &mut Ui, text: &str, context_menu: bool) {
    let padding = ui.spacing().button_padding;
    ui.horizontal(|ui| {
        let mark = if context_menu {
            0.0
        } else {
            menu::MARK_WIDTH + ui.spacing().icon_spacing
        };
        ui.add_space(padding.x + mark);
        ui.label(RichText::new(text).small().weak());
    });
    ui.add_space(2.0);
}

#[cfg(all(test, not(target_os = "macos")))]
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
        // A letter is no longer the bar's.
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
