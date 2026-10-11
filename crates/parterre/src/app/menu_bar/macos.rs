//! A menu is built again when its entries change, though not while a menu is open; one about
//! to open is brought up to date first (`menuNeedsUpdate:`). An item chosen is queued as its
//! [`Command`] for the next frame.
//!
//! macOS hands key presses with ⌘ to the menu first, so the menu takes those shortcuts; keys
//! without a modifier (F, H, L, 1 2 3) still reach the window, which acts on them itself.

// AppKit's API is Objective-C, called through objc2: every call is `unsafe` to Rust.
#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};

use eframe::egui::{self, Key};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject, Sel};
use objc2::{AnyThread, ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSColor, NSControlStateValueOff, NSControlStateValueOn, NSEventModifierFlags,
    NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSMenu, NSMenuDelegate,
    NSMenuItem, NSWindow, NSWorkspace,
};
use objc2_foundation::{NSAttributedString, NSDictionary, NSObjectProtocol, NSSize, NSString};

use super::{Command, Entry, Item, Kind, Menu, System};
use crate::keys::Shortcut;

/// An item's command, and whether it is enabled.
type Slot = (Option<Command>, bool);

thread_local! {
    /// The menus as last built by the app, and whether keys without a modifier are left out.
    static LATEST: RefCell<(Vec<Menu>, bool)> = const { RefCell::new((Vec::new(), false)) };
    /// Per menu of the bar, by item tag.
    static TABLE: RefCell<Vec<Vec<Slot>>> = const { RefCell::new(Vec::new()) };
    /// Per menu of the bar, its [`signature`] when last built.
    static BUILT_SIGNATURES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static CHOSEN: RefCell<Vec<Command>> = const { RefCell::new(Vec::new()) };
    static OPEN_MENU_COUNT: Cell<usize> = const { Cell::new(0) };
    static BAR_MENUS: RefCell<Vec<Retained<NSMenu>>> = const { RefCell::new(Vec::new()) };
    // Kept alive here: a menu item holds its target weakly.
    static TARGET: RefCell<Option<Retained<Target>>> = const { RefCell::new(None) };
    static CONTEXT: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
}

/// A tag holds the menu's index in the bar above these bits, the item's in [`TABLE`] below.
const TAG_SHIFT: u32 = 16;

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and Target has no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ParterreMenuTarget"]
    struct Target;

    impl Target {
        #[unsafe(method(itemChosen:))]
        fn item_chosen(&self, item: &NSMenuItem) {
            if let Some((Some(command), true)) = entry_of(item.tag()) {
                CHOSEN.with_borrow_mut(|chosen| chosen.push(command));
                CONTEXT.with_borrow(|ctx| {
                    if let Some(ctx) = ctx {
                        ctx.request_repaint();
                    }
                });
            }
        }

        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            entry_of(item.tag()).is_some_and(|(_, enabled)| enabled)
        }
    }

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSMenuDelegate for Target {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            if let (Some(top), Some(mtm)) = (bar_index(menu), MainThreadMarker::new()) {
                let (menus, omit_plain_keys) = LATEST.with_borrow(|l| (l.0.clone(), l.1));
                if let Some(m) = menus.get(top) {
                    build_top(mtm, self, top, menu, m, omit_plain_keys);
                }
            }
        }

        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, menu: &NSMenu) {
            OPEN_MENU_COUNT.set(OPEN_MENU_COUNT.get() + 1);
            let kind = bar_index(menu)
                .and_then(|top| LATEST.with_borrow(|l| l.0.get(top).map(|m| m.kind)));
            CONTEXT.with_borrow(|ctx| {
                if let (Some(ctx), Some(kind)) = (ctx, kind) {
                    crate::usage::menu(ctx, kind.usage());
                }
            });
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _menu: &NSMenu) {
            OPEN_MENU_COUNT.set(OPEN_MENU_COUNT.get().saturating_sub(1));
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }
}

fn bar_index(menu: &NSMenu) -> Option<usize> {
    BAR_MENUS.with_borrow(|menus| menus.iter().position(|m| std::ptr::eq(&**m, menu)))
}

fn entry_of(tag: isize) -> Option<Slot> {
    let tag = usize::try_from(tag).ok()?;
    let (top, index) = (tag >> TAG_SHIFT, tag & ((1 << TAG_SHIFT) - 1));
    TABLE.with_borrow(|table| table.get(top)?.get(index).cloned())
}

#[derive(Debug, Default)]
pub struct NativeMenu {
    installed: bool,
    badge_shown: Option<bool>,
}

impl NativeMenu {
    /// Returns what was chosen since the last call. Keys without a modifier are to be omitted
    /// while a text field has the keyboard: typed there, they are its.
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        menus: Vec<Menu>,
        omit_plain_keys: bool,
        badge: bool,
    ) -> Vec<Command> {
        let Some(mtm) = MainThreadMarker::new() else {
            return Vec::new();
        };
        if !self.installed {
            self.installed = true;
            CONTEXT.with_borrow_mut(|c| *c = Some(ctx.clone()));
            install(mtm, &menus);
        }
        if OPEN_MENU_COUNT.get() == 0 {
            let target = TARGET.with_borrow(|t| t.clone());
            let bar_menus = BAR_MENUS.with_borrow(|t| t.clone());
            if let Some(target) = target {
                for (top, (menu, ns)) in menus.iter().zip(&bar_menus).enumerate() {
                    let wanted = signature(menu, omit_plain_keys);
                    if BUILT_SIGNATURES.with_borrow(|b| b.get(top) != Some(&wanted)) {
                        build_top(mtm, &target, top, ns, menu, omit_plain_keys);
                    }
                }
            }
        }
        LATEST.with_borrow_mut(|l| *l = (menus, omit_plain_keys));
        // eframe names the application menu after the window at start (#327).
        let app = NSApplication::sharedApplication(mtm);
        if let Some(first) = app.mainMenu().and_then(|m| m.itemAtIndex(0))
            && let Some(menu) = first.submenu()
            && menu.title().to_string() != "parterre"
        {
            menu.setTitle(&NSString::from_str("parterre"));
        }
        if self.badge_shown != Some(badge) {
            self.badge_shown = Some(badge);
            let label = badge.then(|| NSString::from_str("1"));
            app.dockTile().setBadgeLabel(label.as_deref());
        }
        CHOSEN.with_borrow_mut(std::mem::take)
    }
}

fn signature(menu: &Menu, omit_plain_keys: bool) -> String {
    format!("{omit_plain_keys}{:?}", menu.entries)
}

/// With an empty menu for each of `menus`, filled as they are built.
fn install(mtm: MainThreadMarker, menus: &[Menu]) {
    let target = Target::new(mtm);
    let app = NSApplication::sharedApplication(mtm);
    let bar = NSMenu::new(mtm);
    let mut bar_menus = Vec::new();
    for menu in menus {
        let title = NSString::from_str(&menu.title);
        let item = NSMenuItem::new(mtm);
        item.setTitle(&title);
        let ns = NSMenu::initWithTitle(NSMenu::alloc(mtm), &title);
        ns.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        item.setSubmenu(Some(&ns));
        bar.addItem(&item);
        match menu.kind {
            Kind::Window => app.setWindowsMenu(Some(&ns)),
            Kind::Help => app.setHelpMenu(Some(&ns)),
            _ => {}
        }
        bar_menus.push(ns);
    }
    app.setMainMenu(Some(&bar));
    // AppKit would add tab items to View and Window, for windows parterre never tabs.
    NSWindow::setAllowsAutomaticWindowTabbing(false, mtm);
    TABLE.with_borrow_mut(|t| *t = vec![Vec::new(); menus.len()]);
    BUILT_SIGNATURES.with_borrow_mut(|b| *b = vec![String::new(); menus.len()]);
    BAR_MENUS.with_borrow_mut(|t| *t = bar_menus);
    TARGET.with_borrow_mut(|t| *t = Some(target));
}

fn build_top(
    mtm: MainThreadMarker,
    target: &Target,
    top: usize,
    ns: &NSMenu,
    menu: &Menu,
    omit_plain_keys: bool,
) {
    let mut table = Vec::new();
    let builder = Builder {
        mtm,
        target,
        top,
        omit_plain_keys,
    };
    builder.fill(ns, &menu.entries, &mut table);
    TABLE.with_borrow_mut(|t| {
        if let Some(slot) = t.get_mut(top) {
            *slot = table;
        }
    });
    BUILT_SIGNATURES.with_borrow_mut(|b| {
        if let Some(slot) = b.get_mut(top) {
            *slot = signature(menu, omit_plain_keys);
        }
    });
}

struct Builder<'a> {
    mtm: MainThreadMarker,
    target: &'a Target,
    top: usize,
    omit_plain_keys: bool,
}

impl Builder<'_> {
    fn fill(&self, ns: &NSMenu, entries: &[Entry], table: &mut Vec<Slot>) {
        let mtm = self.mtm;
        ns.removeAllItems();
        for entry in entries {
            match entry {
                Entry::Item(item) => ns.addItem(&self.item(item, table)),
                Entry::Separator => ns.addItem(&NSMenuItem::separatorItem(mtm)),
                Entry::Heading(title) => ns.addItem(&heading(mtm, title)),
                Entry::Submenu(submenu) => {
                    let title = NSString::from_str(&submenu.label);
                    let child = NSMenu::initWithTitle(NSMenu::alloc(mtm), &title);
                    self.fill(&child, &submenu.entries, table);
                    let item = NSMenuItem::new(mtm);
                    item.setTitle(&title);
                    item.setSubmenu(Some(&child));
                    if let Some(why) = &submenu.why_disabled {
                        item.setToolTip(Some(&NSString::from_str(why.text())));
                    }
                    ns.addItem(&item);
                }
                Entry::System(system) => ns.addItem(&system_item(mtm, *system)),
            }
        }
    }

    fn item(&self, item: &Item, table: &mut Vec<Slot>) -> Retained<NSMenuItem> {
        let mtm = self.mtm;
        let tag = (self.top << TAG_SHIFT) | table.len();
        table.push((item.command.clone(), item.enabled));
        let shortcut = item
            .shortcut
            .filter(|s| !(self.omit_plain_keys && s.modifiers.is_none()));
        let (key, modifiers) = shortcut.map_or(
            (String::new(), NSEventModifierFlags::empty()),
            key_equivalent,
        );
        // SAFETY: itemChosen: is Target's, taking the item.
        let ns = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(&item.label),
                Some(sel!(itemChosen:)),
                &NSString::from_str(&key),
            )
        };
        ns.setKeyEquivalentModifierMask(modifiers);
        // SAFETY: the target outlives the menus (TARGET).
        unsafe { ns.setTarget(Some(self.target)) };
        ns.setTag(tag as isize);
        ns.setEnabled(item.enabled);
        match item.mark {
            crate::menu::Mark::Check(on) | crate::menu::Mark::Radio(on) => ns.setState(if on {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            }),
            crate::menu::Mark::None => {}
        }
        let why_disabled = item.why_disabled.as_ref().map(super::Why::text);
        if let Some(tip) = item.tip.as_deref().or(why_disabled) {
            ns.setToolTip(Some(&NSString::from_str(tip)));
        }
        if item.accent {
            ns.setAttributedTitle(Some(&accented(&item.label)));
        }
        if let Some(folder) = &item.folder_icon {
            let workspace = NSWorkspace::sharedWorkspace();
            let image = workspace.iconForFile(&NSString::from_str(&folder.to_string_lossy()));
            image.setSize(NSSize::new(16.0, 16.0));
            ns.setImage(Some(&image));
        }
        ns
    }
}

fn accented(label: &str) -> Retained<NSAttributedString> {
    let color = NSColor::controlAccentColor();
    let font = NSFont::boldSystemFontOfSize(NSFont::systemFontSize());
    // SAFETY: the keys are AppKit's attribute names, with values of their types.
    unsafe {
        let keys = [NSForegroundColorAttributeName, NSFontAttributeName];
        let values: [&AnyObject; 2] = [&color, &font];
        let attributes = NSDictionary::from_slices(&keys, &values);
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(label),
            Some(&attributes),
        )
    }
}

/// A small title over a group of items: AppKit's section header from macOS 14 on, a greyed
/// item before.
fn heading(mtm: MainThreadMarker, title: &str) -> Retained<NSMenuItem> {
    let title = NSString::from_str(title);
    if NSMenuItem::class()
        .class_method(sel!(sectionHeaderWithTitle:))
        .is_some()
    {
        return NSMenuItem::sectionHeaderWithTitle(&title, mtm);
    }
    let item = NSMenuItem::new(mtm);
    item.setTitle(&title);
    item.setEnabled(false);
    item
}

/// Without a target, AppKit sends the action to the first responder.
fn system_item(mtm: MainThreadMarker, system: System) -> Retained<NSMenuItem> {
    let command = NSEventModifierFlags::Command;
    let (title, action, key, modifiers): (&str, Option<Sel>, &str, NSEventModifierFlags) =
        match system {
            System::Services => ("Services", None, "", NSEventModifierFlags::empty()),
            System::Hide => ("Hide parterre", Some(sel!(hide:)), "h", command),
            System::HideOthers => (
                "Hide Others",
                Some(sel!(hideOtherApplications:)),
                "h",
                command | NSEventModifierFlags::Option,
            ),
            System::ShowAll => (
                "Show All",
                Some(sel!(unhideAllApplications:)),
                "",
                NSEventModifierFlags::empty(),
            ),
            System::Minimize => ("Minimize", Some(sel!(performMiniaturize:)), "m", command),
            System::Zoom => (
                "Zoom",
                Some(sel!(performZoom:)),
                "",
                NSEventModifierFlags::empty(),
            ),
            System::BringAllToFront => (
                "Bring All to Front",
                Some(sel!(arrangeInFront:)),
                "",
                NSEventModifierFlags::empty(),
            ),
            System::FullScreen => (
                "Enter Full Screen",
                Some(sel!(toggleFullScreen:)),
                "f",
                command | NSEventModifierFlags::Control,
            ),
        };
    // SAFETY: AppKit's own actions, sent along the responder chain.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    };
    item.setKeyEquivalentModifierMask(modifiers);
    if system == System::Services {
        let services = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
        item.setSubmenu(Some(&services));
        NSApplication::sharedApplication(mtm).setServicesMenu(Some(&services));
    }
    item
}

fn key_equivalent(shortcut: Shortcut) -> (String, NSEventModifierFlags) {
    let m = shortcut.modifiers;
    let mut flags = NSEventModifierFlags::empty();
    if m.command || m.mac_cmd {
        flags |= NSEventModifierFlags::Command;
    }
    if m.shift {
        flags |= NSEventModifierFlags::Shift;
    }
    if m.alt {
        flags |= NSEventModifierFlags::Option;
    }
    if m.ctrl {
        flags |= NSEventModifierFlags::Control;
    }
    let key = match shortcut.key {
        Key::Comma => ",".to_owned(),
        Key::Plus => "+".to_owned(),
        Key::Minus => "-".to_owned(),
        Key::Equals => "=".to_owned(),
        Key::Escape => "\u{1b}".to_owned(),
        // NSF3FunctionKey and NSF5FunctionKey.
        Key::F3 => "\u{F706}".to_owned(),
        Key::F5 => "\u{F708}".to_owned(),
        key => key.name().to_lowercase(),
    };
    (key, flags)
}
