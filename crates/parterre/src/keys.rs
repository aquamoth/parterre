//! The keyboard: one binding per action and platform, the one that platform's users expect
//! (#343), labelled as they write it (#334): `⌘O` on macOS, `Ctrl+O` elsewhere.
//!
//! egui's `COMMAND` is ⌘ on macOS and Ctrl elsewhere, so most bindings are the same key
//! everywhere and only their labels differ. The rest (redo, fetch, find next, closing a window)
//! differ by platform, and are functions of [`Platform`].

use eframe::egui::{self, InputState, Key, KeyboardShortcut, ModifierNames, Modifiers};

/// The platform whose conventions apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Windows,
    Linux,
}

impl Platform {
    /// The platform parterre runs on.
    pub const CURRENT: Platform = if cfg!(target_os = "macos") {
        Platform::Mac
    } else if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Linux
    };
}

/// A key with modifiers, e.g. Ctrl+Shift+Z.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl Shortcut {
    pub const fn new(modifiers: Modifiers, key: Key) -> Shortcut {
        Shortcut { modifiers, key }
    }

    /// The key alone, as `F` or `F5`.
    pub const fn plain(key: Key) -> Shortcut {
        Shortcut::new(Modifiers::NONE, key)
    }

    /// ⌘ on macOS, Ctrl elsewhere, with `key`.
    pub const fn command(key: Key) -> Shortcut {
        Shortcut::new(Modifiers::COMMAND, key)
    }

    /// ⇧⌘ on macOS, Ctrl+Shift elsewhere, with `key`.
    pub const fn command_shift(key: Key) -> Shortcut {
        Shortcut::new(
            Modifiers {
                shift: true,
                ..Modifiers::COMMAND
            },
            key,
        )
    }

    /// As written on the platform parterre runs on.
    pub fn label(self) -> String {
        self.label_on(Platform::CURRENT)
    }

    /// As written on `platform`: Apple's symbols in Apple's order on macOS (`⇧⌘Z`, as egui's
    /// `Context::format_shortcut` writes them), names joined by `+` elsewhere (`Ctrl+Shift+Z`).
    pub fn label_on(self, platform: Platform) -> String {
        // egui names the modifiers of `COMMAND` by the platform it is told.
        if platform == Platform::Mac {
            KeyboardShortcut::new(self.modifiers, self.key).format(&ModifierNames::SYMBOLS, true)
        } else {
            let mut label = ModifierNames::NAMES.format(&self.modifiers, false);
            if !label.is_empty() {
                label.push('+');
            }
            // `Ctrl+,` and `Ctrl++` rather than `Ctrl+Comma` and `Ctrl+Plus`, but `Ctrl+Up`.
            label.push_str(match self.key {
                Key::Comma | Key::Plus | Key::Minus | Key::Equals | Key::Period => {
                    self.key.symbol_or_name()
                }
                Key::Escape => "Esc",
                key => key.name(),
            });
            label
        }
    }

    /// Whether it was pressed this frame, taking the press so that nothing else acts on it.
    /// egui ignores a Shift held besides: take the Shift-variants first.
    pub fn consume(self, input: &mut InputState) -> bool {
        input.consume_key(self.modifiers, self.key)
    }
}

pub const OPEN: Shortcut = Shortcut::command(Key::O);
pub const CLOSE: Shortcut = Shortcut::command(Key::W);
pub const SETTINGS: Shortcut = Shortcut::command(Key::Comma);
pub const QUIT: Shortcut = Shortcut::command(Key::Q);
pub const UNDO: Shortcut = Shortcut::command(Key::Z);
pub const CUT: Shortcut = Shortcut::command(Key::X);
pub const PASTE: Shortcut = Shortcut::command(Key::V);
pub const SELECT_ALL: Shortcut = Shortcut::command(Key::A);
pub const COPY: Shortcut = Shortcut::command(Key::C);
pub const FIND: Shortcut = Shortcut::command(Key::F);
pub const ZOOM_IN: Shortcut = Shortcut::command(Key::Plus);
/// ⌘= and Ctrl+=, which is + unshifted on US layouts: the same as [`ZOOM_IN`].
pub const ZOOM_IN_UNSHIFTED: Shortcut = Shortcut::command(Key::Equals);
pub const ZOOM_OUT: Shortcut = Shortcut::command(Key::Minus);
pub const ACTUAL_SIZE: Shortcut = Shortcut::command(Key::Num0);
/// F5 reloads on every platform, besides [`reload`].
pub const RELOAD_F5: Shortcut = Shortcut::plain(Key::F5);
pub const ZOOM_TO_FIT: Shortcut = Shortcut::plain(Key::F);
pub const GO_TO_HEAD: Shortcut = Shortcut::plain(Key::H);
pub const SHOW_LOG: Shortcut = Shortcut::plain(Key::L);
/// The drag modes Adapt, Free and Subtree.
pub const DRAG: [Shortcut; 3] = [
    Shortcut::plain(Key::Num1),
    Shortcut::plain(Key::Num2),
    Shortcut::plain(Key::Num3),
];

/// Redo a move: ⇧⌘Z on macOS, Ctrl+Y on Windows, Ctrl+Shift+Z on Linux.
pub fn redo() -> Shortcut {
    redo_on(Platform::CURRENT)
}

pub fn redo_on(platform: Platform) -> Shortcut {
    match platform {
        Platform::Windows => Shortcut::command(Key::Y),
        Platform::Mac | Platform::Linux => Shortcut::command_shift(Key::Z),
    }
}

/// Reload: ⌘R on macOS and Ctrl+R elsewhere (#340), besides F5.
pub const RELOAD: Shortcut = Shortcut::command(Key::R);

/// Fetch every remote: ⇧⌘F on macOS, where F5 needs fn; Ctrl+F5 elsewhere.
pub fn fetch() -> Shortcut {
    fetch_on(Platform::CURRENT)
}

pub fn fetch_on(platform: Platform) -> Shortcut {
    match platform {
        Platform::Mac => Shortcut::command_shift(Key::F),
        Platform::Windows | Platform::Linux => Shortcut::command(Key::F5),
    }
}

/// The next find result: ⌘G on macOS, F3 elsewhere.
pub fn find_next() -> Shortcut {
    find_next_on(Platform::CURRENT)
}

pub fn find_next_on(platform: Platform) -> Shortcut {
    match platform {
        Platform::Mac => Shortcut::command(Key::G),
        Platform::Windows | Platform::Linux => Shortcut::plain(Key::F3),
    }
}

/// The previous find result: ⇧⌘G on macOS, Shift+F3 elsewhere.
pub fn find_previous() -> Shortcut {
    find_previous_on(Platform::CURRENT)
}

pub fn find_previous_on(platform: Platform) -> Shortcut {
    match platform {
        Platform::Mac => Shortcut::command_shift(Key::G),
        Platform::Windows | Platform::Linux => Shortcut::new(Modifiers::SHIFT, Key::F3),
    }
}

/// Close the log, diff, blame, compare or settings window: ⌘W on macOS, Escape elsewhere.
pub fn close_window() -> Shortcut {
    close_window_on(Platform::CURRENT)
}

pub fn close_window_on(platform: Platform) -> Shortcut {
    match platform {
        Platform::Mac => CLOSE,
        Platform::Windows | Platform::Linux => Shortcut::plain(Key::Escape),
    }
}

/// The blame window's Go to line: Ctrl+G, as in TortoiseGitBlame; ⌃G on macOS, where ⌘G is
/// the next find result.
pub fn go_to_line() -> Shortcut {
    match Platform::CURRENT {
        Platform::Mac => Shortcut::new(Modifiers::CTRL, Key::G),
        Platform::Windows | Platform::Linux => Shortcut::command(Key::G),
    }
}

/// Reload and fetch pressed in a window this frame. Fetch first: on macOS ⇧⌘F would otherwise
/// pass for a window's ⌘F. Held down, F5 reloads once.
pub fn reload_and_fetch(input: &mut InputState) -> (bool, bool) {
    let fetch = fetch().consume(input);
    let f5 = input.events.iter().any(|e| {
        matches!(
            e,
            egui::Event::Key {
                key: Key::F5,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } if modifiers.is_none()
        )
    });
    let reload = f5 || RELOAD.consume(input);
    (reload, fetch)
}

/// The reload key named in menus and tips: ⌘R on macOS, where F5 needs fn; F5 elsewhere,
/// where it is the familiar one.
pub fn reload_shown_on(platform: Platform) -> Shortcut {
    match platform {
        Platform::Mac => RELOAD,
        Platform::Windows | Platform::Linux => RELOAD_F5,
    }
}

/// [`reload_shown_on`] this platform, as written.
pub fn reload_label() -> String {
    reload_shown_on(Platform::CURRENT).label()
}

/// The keys of a find field's previous, next and clear buttons: Shift+Enter or the previous
/// find result's key, Enter or the next one's, and Esc.
pub fn find_field_keys() -> [String; 3] {
    [
        format!("Shift+Enter, {}", find_previous().label()),
        format!("Enter, {}", find_next().label()),
        "Esc".to_owned(),
    ]
}

/// `action` with ⌘ or Ctrl held, as written: `⌘-click`, `Ctrl+click`.
pub fn with_command(action: &str) -> String {
    with_command_on(Platform::CURRENT, action)
}

pub fn with_command_on(platform: Platform, action: &str) -> String {
    match platform {
        Platform::Mac => format!("⌘-{action}"),
        Platform::Windows | Platform::Linux => format!("Ctrl+{action}"),
    }
}

/// `action` with Shift held, as written: `⇧-click`, `Shift+click`.
pub fn with_shift(action: &str) -> String {
    match Platform::CURRENT {
        Platform::Mac => format!("⇧-{action}"),
        Platform::Windows | Platform::Linux => format!("Shift+{action}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_follow_the_platform() {
        let cases = [
            (OPEN, "⌘O", "Ctrl+O"),
            (CLOSE, "⌘W", "Ctrl+W"),
            (SETTINGS, "⌘,", "Ctrl+,"),
            (FIND, "⌘F", "Ctrl+F"),
            (ZOOM_IN, "⌘+", "Ctrl++"),
            (ACTUAL_SIZE, "⌘0", "Ctrl+0"),
            (Shortcut::command_shift(Key::Z), "⇧⌘Z", "Ctrl+Shift+Z"),
            (ZOOM_TO_FIT, "F", "F"),
            (RELOAD_F5, "F5", "F5"),
        ];
        for (shortcut, mac, other) in cases {
            assert_eq!(shortcut.label_on(Platform::Mac), mac);
            assert_eq!(shortcut.label_on(Platform::Windows), other);
            assert_eq!(shortcut.label_on(Platform::Linux), other);
        }
    }

    #[test]
    fn one_binding_per_platform() {
        assert_eq!(redo_on(Platform::Mac).label_on(Platform::Mac), "⇧⌘Z");
        assert_eq!(
            redo_on(Platform::Windows).label_on(Platform::Windows),
            "Ctrl+Y"
        );
        assert_eq!(
            redo_on(Platform::Linux).label_on(Platform::Linux),
            "Ctrl+Shift+Z"
        );
        assert_eq!(fetch_on(Platform::Mac).label_on(Platform::Mac), "⇧⌘F");
        assert_eq!(
            fetch_on(Platform::Linux).label_on(Platform::Linux),
            "Ctrl+F5"
        );
        assert_eq!(find_next_on(Platform::Mac).label_on(Platform::Mac), "⌘G");
        assert_eq!(
            find_previous_on(Platform::Mac).label_on(Platform::Mac),
            "⇧⌘G"
        );
        assert_eq!(
            find_next_on(Platform::Windows).label_on(Platform::Windows),
            "F3"
        );
        assert_eq!(
            find_previous_on(Platform::Windows).label_on(Platform::Windows),
            "Shift+F3"
        );
        assert_eq!(close_window_on(Platform::Mac).label_on(Platform::Mac), "⌘W");
        assert_eq!(
            close_window_on(Platform::Linux).label_on(Platform::Linux),
            "Esc"
        );
        assert_eq!(RELOAD.label_on(Platform::Mac), "⌘R");
        assert_eq!(RELOAD.label_on(Platform::Windows), "Ctrl+R");
        assert_eq!(with_command_on(Platform::Mac, "click"), "⌘-click");
        assert_eq!(with_command_on(Platform::Linux, "click"), "Ctrl+click");
    }

    fn event(s: Shortcut) -> egui::Event {
        egui::Event::Key {
            key: s.key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: s.modifiers,
        }
    }

    fn input(events: Vec<egui::Event>) -> InputState {
        let raw = egui::RawInput {
            events,
            ..Default::default()
        };
        InputState::default().begin_pass(raw, false, 1.0, egui::InputOptions::default())
    }

    #[test]
    fn fetch_is_taken_before_find() {
        let mut i = input(vec![event(fetch())]);
        assert_eq!(reload_and_fetch(&mut i), (false, true));
        // Gone: a window's own ⌘F (Ctrl+F) no longer sees it.
        assert!(!FIND.consume(&mut i));
        let mut i = input(vec![event(RELOAD_F5)]);
        assert_eq!(reload_and_fetch(&mut i), (true, false));
        let mut i = input(vec![event(RELOAD)]);
        assert_eq!(reload_and_fetch(&mut i), (true, false));
    }
}
