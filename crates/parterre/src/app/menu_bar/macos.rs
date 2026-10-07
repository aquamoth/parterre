//! The menu bar on macOS: the system's, built with AppKit from the menus `menu_bar::build`
//! makes, in place of the menu winit gives every app. A menu is built again when its entries
//! change, though not while a menu is open; one about to open is brought up to date first
//! (`menuNeedsUpdate:`). An item chosen, from the menu or by its key, is queued as its
//! [`Command`] for the next frame. Apple's own items (Services, Hide, Minimize, Enter Full
//! Screen, …) are AppKit's, sent to the window in front.
//!
//! macOS hands key presses with ⌘ to the menu first, so the menu takes those shortcuts; keys
//! without a modifier (F, H, L, 1 2 3) still reach the window, which acts on them itself, and
//! the menu only shows them.

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

thread_local! {
    /// The menus as the app last made them, for a menu about to open; and whether keys without
    /// a modifier are left out (a text field has the keyboard).
    static LATEST: RefCell<(Vec<Menu>, bool)> = const { RefCell::new((Vec::new(), false)) };
    /// Per menu of the bar, the items' commands by tag, and whether each is enabled.
    static TABLE: RefCell<Vec<Vec<(Command, bool)>>> = const { RefCell::new(Vec::new()) };
    /// Per menu of the bar, what it was built from, to build it again only when that changes.
    static BUILT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// What was chosen since the last frame.
    static CHOSEN: RefCell<Vec<Command>> = const { RefCell::new(Vec::new()) };
    /// How many menus are open now.
    static OPEN: Cell<usize> = const { Cell::new(0) };
    /// The menus of the bar, to tell which is about to open.
    static TOPS: RefCell<Vec<Retained<NSMenu>>> = const { RefCell::new(Vec::new()) };
    /// The items' target, alive as long as the menus.
    static TARGET: RefCell<Option<Retained<Target>>> = const { RefCell::new(None) };
    /// For a repaint once something is chosen.
    static CONTEXT: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
}

/// The bits of a tag: the menu of the bar above, the item's index in its table below.
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
            if let Some((command, true)) = entry_of(item.tag()) {
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
            let top = TOPS.with_borrow(|tops| tops.iter().position(|t| std::ptr::eq(&**t, menu)));
            if let (Some(top), Some(mtm)) = (top, MainThreadMarker::new()) {
                let (menus, strip) = LATEST.with_borrow(|l| (l.0.clone(), l.1));
                if let Some(m) = menus.get(top) {
                    build_top(mtm, self, top, menu, m, strip);
                }
            }
        }

        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, menu: &NSMenu) {
            OPEN.set(OPEN.get() + 1);
            // Counted in the usage statistics, as the egui menus are when shown.
            let top = TOPS.with_borrow(|tops| tops.iter().position(|t| std::ptr::eq(&**t, menu)));
            let kind = top.and_then(|top| LATEST.with_borrow(|l| l.0.get(top).map(|m| m.kind)));
            CONTEXT.with_borrow(|ctx| {
                if let (Some(ctx), Some(kind)) = (ctx, kind) {
                    crate::usage::menu(ctx, kind.usage());
                }
            });
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _menu: &NSMenu) {
            OPEN.set(OPEN.get().saturating_sub(1));
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

/// The command and enabled state of the item tagged `tag`.
fn entry_of(tag: isize) -> Option<(Command, bool)> {
    let tag = usize::try_from(tag).ok()?;
    let (top, index) = (tag >> TAG_SHIFT, tag & ((1 << TAG_SHIFT) - 1));
    TABLE.with_borrow(|table| table.get(top)?.get(index).cloned())
}

/// The system menu bar, while parterre runs.
#[derive(Debug, Default)]
pub struct NativeMenu {
    installed: bool,
    badge: Option<bool>,
}

impl NativeMenu {
    /// Brings the menu bar up to date with `menus` (making it the first time), and the Dock
    /// icon's badge with `badge`; returns what was chosen since the last call. `strip` leaves out
    /// the keys without a modifier, while a text field has the keyboard: typed there, they are
    /// its.
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        menus: Vec<Menu>,
        strip: bool,
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
        if OPEN.get() == 0 {
            let target = TARGET.with_borrow(|t| t.clone());
            let tops = TOPS.with_borrow(|t| t.clone());
            if let Some(target) = target {
                for (top, (menu, ns)) in menus.iter().zip(&tops).enumerate() {
                    let built = signature(menu, strip);
                    if BUILT.with_borrow(|b| b.get(top) != Some(&built)) {
                        build_top(mtm, &target, top, ns, menu, strip);
                    }
                }
            }
        }
        LATEST.with_borrow_mut(|l| *l = (menus, strip));
        // eframe names the application menu after the window at start (#327).
        let app = NSApplication::sharedApplication(mtm);
        if let Some(first) = app.mainMenu().and_then(|m| m.itemAtIndex(0))
            && let Some(menu) = first.submenu()
            && menu.title().to_string() != "parterre"
        {
            menu.setTitle(&NSString::from_str("parterre"));
        }
        if self.badge != Some(badge) {
            self.badge = Some(badge);
            let label = badge.then(|| NSString::from_str("1"));
            app.dockTile().setBadgeLabel(label.as_deref());
        }
        CHOSEN.with_borrow_mut(std::mem::take)
    }
}

/// What a menu is built from: its entries, and whether keys without a modifier are left out.
fn signature(menu: &Menu, strip: bool) -> String {
    format!("{strip}{:?}", menu.entries)
}

/// The menu bar, with an empty menu for each of `menus`, filled as they are built.
fn install(mtm: MainThreadMarker, menus: &[Menu]) {
    let target = Target::new(mtm);
    let app = NSApplication::sharedApplication(mtm);
    let bar = NSMenu::new(mtm);
    let mut tops = Vec::new();
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
        tops.push(ns);
    }
    app.setMainMenu(Some(&bar));
    // AppKit would add tab items to View and Window, for windows parterre never tabs.
    NSWindow::setAllowsAutomaticWindowTabbing(false, mtm);
    TABLE.with_borrow_mut(|t| *t = vec![Vec::new(); menus.len()]);
    BUILT.with_borrow_mut(|b| *b = vec![String::new(); menus.len()]);
    TOPS.with_borrow_mut(|t| *t = tops);
    TARGET.with_borrow_mut(|t| *t = Some(target));
}

/// Builds menu `top` of the bar, `ns`, from `menu`.
fn build_top(
    mtm: MainThreadMarker,
    target: &Target,
    top: usize,
    ns: &NSMenu,
    menu: &Menu,
    strip: bool,
) {
    let mut table = Vec::new();
    let builder = Builder {
        mtm,
        target,
        top,
        strip,
    };
    builder.fill(ns, &menu.entries, &mut table);
    TABLE.with_borrow_mut(|t| {
        if let Some(slot) = t.get_mut(top) {
            *slot = table;
        }
    });
    BUILT.with_borrow_mut(|b| {
        if let Some(slot) = b.get_mut(top) {
            *slot = signature(menu, strip);
        }
    });
}

struct Builder<'a> {
    mtm: MainThreadMarker,
    target: &'a Target,
    top: usize,
    strip: bool,
}

impl Builder<'_> {
    fn fill(&self, ns: &NSMenu, entries: &[Entry], table: &mut Vec<(Command, bool)>) {
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
                    if let Some(why) = &submenu.why {
                        item.setToolTip(Some(&NSString::from_str(why)));
                    }
                    ns.addItem(&item);
                }
                Entry::System(system) => ns.addItem(&system_item(mtm, *system)),
            }
        }
    }

    fn item(&self, item: &Item, table: &mut Vec<(Command, bool)>) -> Retained<NSMenuItem> {
        let mtm = self.mtm;
        let tag = (self.top << TAG_SHIFT) | table.len();
        table.push((item.command.clone(), item.enabled));
        let shortcut = item
            .shortcut
            .filter(|s| !(self.strip && s.modifiers.is_none()));
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
        if let Some(tip) = item.tip.as_ref().or(item.why.as_ref()) {
            ns.setToolTip(Some(&NSString::from_str(tip)));
        }
        if item.accent {
            ns.setAttributedTitle(Some(&accented(&item.label)));
        }
        if let Some(folder) = &item.icon {
            let workspace = NSWorkspace::sharedWorkspace();
            let image = workspace.iconForFile(&NSString::from_str(&folder.to_string_lossy()));
            image.setSize(NSSize::new(16.0, 16.0));
            ns.setImage(Some(&image));
        }
        ns
    }
}

/// `label` bold, in the system's accent colour: *Download*.
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

/// An item AppKit carries out, sent to the first responder.
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

/// The key equivalent and modifiers AppKit takes for `shortcut`.
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
        Key::F3 => "\u{F706}".to_owned(),
        Key::F5 => "\u{F708}".to_owned(),
        key => key.name().to_lowercase(),
    };
    (key, flags)
}
