//! How the main window first appears on Windows (#309).
//!
//! eframe creates the window hidden and shows it once its first frame is painted, but winit
//! cannot keep a maximized window out of sight that long: maximizing a hidden window is
//! `ShowWindow(SW_MAXIMIZE)` followed by `SW_HIDE`, and showing it later is
//! `SW_SHOWNOACTIVATE`, which restores a maximized window, followed by `SW_MAXIMIZE` again.
//! Each shows the window, with its maximize animation, before anything is painted in it:
//! a title bar over a transparent client area, then white, then the animation.
//!
//! So on Windows the window is created unmaximized and cloaked from the desktop compositor
//! (DWM), which composes a cloaked window but does not display it. It is shown and maximized
//! while cloaked, and uncloaked once a frame at its final size has been painted: the first
//! thing on screen is that frame.

use eframe::egui;

/// Elsewhere eframe's own way, shown once the first frame is painted, does: nothing to reveal.
#[cfg(not(windows))]
pub enum Reveal {}

#[cfg(not(windows))]
impl Reveal {
    pub fn finish(&mut self, _ctx: &egui::Context) -> bool {
        match *self {}
    }
}

/// The window, cloaked until a frame at its final size has been painted.
#[cfg(windows)]
pub struct Reveal {
    hwnd: isize,
    maximized: bool,
    /// The screen rect of the last frame painted in the final state, if any.
    painted: Option<egui::Rect>,
}

#[cfg(windows)]
impl Reveal {
    /// Cloaks the newly created, still hidden window and asks for it to be shown, maximized if
    /// `maximized`. None when the window handle is not a Win32 one.
    pub fn start(cc: &eframe::CreationContext<'_>, maximized: bool) -> Option<Reveal> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let RawWindowHandle::Win32(handle) = cc.window_handle().ok()?.as_raw() else {
            return None;
        };
        let hwnd = handle.hwnd.get();
        cloak(hwnd, true);
        cc.egui_ctx
            .send_viewport_cmd(egui::ViewportCommand::Visible(true));
        if maximized {
            cc.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
        Some(Reveal {
            hwnd,
            maximized,
            painted: None,
        })
    }

    /// Called every frame: uncloaks the window once the frame before was painted at the final
    /// size, and returns whether that is done.
    pub fn finish(&mut self, ctx: &egui::Context) -> bool {
        let rect = ctx.content_rect();
        let settled = !self.maximized || ctx.input(|i| i.viewport().maximized == Some(true));
        if settled && self.painted == Some(rect) {
            cloak(self.hwnd, false);
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            return true;
        }
        self.painted = settled.then_some(rect);
        ctx.request_repaint();
        false
    }
}

/// Cloaks the window from the compositor, or uncloaks it.
#[cfg(windows)]
fn cloak(hwnd: isize, on: bool) {
    win32::cloak(hwnd, on);
}

/// The one dwmapi function this needs, declared here as `console` declares its kernel32 ones.
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
