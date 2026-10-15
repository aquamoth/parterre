//! The context-menu key (Menu, or Application) opens the context menu of what has the keyboard,
//! as Shift+F10 ([`keys::CONTEXT_MENU`]) does (#387). egui knows no such key and egui-winit
//! drops it, so on Windows a hook on the messages of the thread the windows run on notes it, and
//! [`ContextKey`] hands it to the window in front as Shift+F10. Elsewhere Shift+F10 alone does.

use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;

use crate::keys;

/// The context-menu key was pressed, and no window has been handed it yet.
static PRESSED: AtomicBool = AtomicBool::new(false);

/// Hands the context-menu key to the window in front as Shift+F10. Registered once, with
/// [`egui::Context::add_plugin`], after [`install`].
#[derive(Debug, Default)]
pub struct ContextKey;

impl egui::Plugin for ContextKey {
    fn debug_name(&self) -> &'static str {
        "parterre-context-key"
    }

    fn input_hook(&mut self, _ctx: &egui::Context, input: &mut egui::RawInput) {
        // Kept for the window in front, which may run after the one that wakes first.
        if input.focused && PRESSED.swap(false, Ordering::Relaxed) {
            hand(input);
        }
    }
}

/// The key, as Shift+F10, pressed and let go in `input`.
fn hand(input: &mut egui::RawInput) {
    let shortcut = keys::CONTEXT_MENU;
    input
        .events
        .extend([true, false].map(|pressed| egui::Event::Key {
            key: shortcut.key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: shortcut.modifiers,
        }));
}

/// Starts noting the context-menu key, for every window of this thread (Windows).
#[cfg(windows)]
pub fn install(ctx: &egui::Context) {
    win32::install(ctx);
}

#[cfg(not(windows))]
pub fn install(_ctx: &egui::Context) {}

/// A `WH_GETMESSAGE` hook on this thread, declared here as `console` declares its kernel32
/// functions.
#[cfg(windows)]
#[allow(unsafe_code)]
mod win32 {
    use std::sync::OnceLock;
    use std::sync::atomic::Ordering;

    use eframe::egui;

    /// Woken when the key is pressed, so that the window in front runs a frame.
    static CONTEXT: OnceLock<egui::Context> = OnceLock::new();

    const WH_GETMESSAGE: i32 = 3;
    const PM_REMOVE: usize = 1;
    const WM_KEYDOWN: u32 = 0x0100;
    const WM_SYSKEYDOWN: u32 = 0x0104;
    const VK_APPS: usize = 0x5D;

    /// The start of Win32's `MSG`, as far as this reads it.
    #[repr(C)]
    struct Msg {
        hwnd: isize,
        message: u32,
        w_param: usize,
        l_param: isize,
    }

    type HookProc = unsafe extern "system" fn(i32, usize, isize) -> isize;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowsHookExW(id: i32, hook: HookProc, module: isize, thread: u32) -> isize;
        fn CallNextHookEx(hook: isize, code: i32, w_param: usize, l_param: isize) -> isize;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        safe fn GetCurrentThreadId() -> u32;
    }

    pub fn install(ctx: &egui::Context) {
        if CONTEXT.set(ctx.clone()).is_err() {
            return;
        }
        // SAFETY: `hook` has the signature a WH_GETMESSAGE hook needs and lives as long as the
        // process; a hook on this thread alone takes no module. A failure (0) leaves Shift+F10.
        unsafe {
            SetWindowsHookExW(WH_GETMESSAGE, hook, 0, GetCurrentThreadId());
        }
    }

    /// Notes the key as its message is taken off the queue (not when only looked at).
    unsafe extern "system" fn hook(code: i32, w_param: usize, l_param: isize) -> isize {
        if code >= 0 && w_param == PM_REMOVE {
            // SAFETY: for WH_GETMESSAGE, `l_param` points at the MSG being taken off the queue,
            // whose start `Msg` is.
            let msg = unsafe { &*(l_param as *const Msg) };
            if matches!(msg.message, WM_KEYDOWN | WM_SYSKEYDOWN) && msg.w_param == VK_APPS {
                super::PRESSED.store(true, Ordering::Relaxed);
                if let Some(ctx) = CONTEXT.get() {
                    ctx.request_repaint();
                }
            }
        }
        // SAFETY: passes the hook's own arguments on, as every hook must.
        unsafe { CallNextHookEx(0, code, w_param, l_param) }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use eframe::egui::{self, Plugin};

    use super::{ContextKey, PRESSED};

    /// The key reaches the window in front as Shift+F10, once; one behind it doesn't take it.
    #[test]
    fn the_key_reaches_the_window_in_front_as_shift_f10() {
        let ctx = egui::Context::default();
        PRESSED.store(true, Ordering::Relaxed);
        let mut behind = egui::RawInput {
            focused: false,
            ..Default::default()
        };
        ContextKey.input_hook(&ctx, &mut behind);
        assert!(behind.events.is_empty());
        let mut front = egui::RawInput::default();
        ContextKey.input_hook(&ctx, &mut front);
        let shift_f10 = |e: &egui::Event| {
            matches!(e, egui::Event::Key { key: egui::Key::F10, pressed: true, modifiers, .. }
                if modifiers.shift_only())
        };
        assert!(front.events.iter().any(shift_f10), "{:?}", front.events);
        let mut again = egui::RawInput::default();
        ContextKey.input_hook(&ctx, &mut again);
        assert!(again.events.is_empty(), "once");
    }
}
