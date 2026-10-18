//! How the main window first appears on Windows (#309).
//!
//! eframe creates the window hidden and shows it once its first frame is painted, but winit
//! can't keep a maximized window out of sight that long: maximizing a hidden window is
//! `ShowWindow(SW_MAXIMIZE)` then `SW_HIDE`, and showing it later is `SW_SHOWNOACTIVATE`, which
//! restores it, then `SW_MAXIMIZE` again. Each shows the window, with its maximize animation,
//! before anything is painted in it.
//!
//! So the window is created unmaximized and cloaked from the compositor (DWM), shown and
//! maximized while cloaked, and uncloaked once a frame at its final size has been painted.

use eframe::egui;

/// Elsewhere eframe's own reveal suffices, so there is never one.
#[cfg(not(windows))]
pub enum Reveal {}

#[cfg(not(windows))]
impl Reveal {
    pub fn finish(&mut self, _ctx: &egui::Context) -> bool {
        match *self {}
    }
}

#[cfg(windows)]
pub struct Reveal {
    hwnd: isize,
    maximized: bool,
    last_settled_rect: Option<egui::Rect>,
}

#[cfg(windows)]
impl Reveal {
    /// Cloaks the new, still hidden window and asks for it to be shown. `None` when the window
    /// handle isn't a Win32 one.
    pub fn start(cc: &eframe::CreationContext<'_>, maximized: bool) -> Option<Reveal> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let RawWindowHandle::Win32(handle) = cc.window_handle().ok()?.as_raw() else {
            return None;
        };
        let hwnd = handle.hwnd.get();
        win32::cloak(hwnd, true);
        cc.egui_ctx
            .send_viewport_cmd(egui::ViewportCommand::Visible(true));
        if maximized {
            cc.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
        Some(Reveal {
            hwnd,
            maximized,
            last_settled_rect: None,
        })
    }

    /// Call every frame: uncloaks the window once the frame before was painted at the final
    /// size, and returns whether it did.
    pub fn finish(&mut self, ctx: &egui::Context) -> bool {
        let rect = ctx.content_rect();
        let settled = !self.maximized || ctx.input(|i| i.viewport().maximized == Some(true));
        if settled && self.last_settled_rect == Some(rect) {
            win32::cloak(self.hwnd, false);
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            return true;
        }
        self.last_settled_rect = settled.then_some(rect);
        ctx.request_repaint();
        false
    }
}

/// Declared by hand, as `console` declares its kernel32 functions.
#[cfg(windows)]
#[allow(unsafe_code)]
mod win32 {
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: isize,
            attribute: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }

    const DWMWA_CLOAK: u32 = 13;

    pub fn cloak(hwnd: isize, on: bool) {
        let value: i32 = on.into();
        // SAFETY: the attribute takes a BOOL (i32), which `value` is and outlives the call; the
        // handle is the window's own. The result is a status code, not needed.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_CLOAK,
                std::ptr::from_ref(&value).cast(),
                std::mem::size_of::<i32>() as u32,
            );
        }
    }
}
