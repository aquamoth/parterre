//! Works around <https://github.com/emilk/egui/issues/5145>: with vsync on, Mesa's (and
//! NVIDIA's) EGL swap waits for the compositor's frame callback, which never comes for a
//! minimized or hidden window. All viewports share one thread, so minimizing one stops the whole
//! app. On Wayland vsync is off, and [`FrameLimiter`] caps the frame rate instead with a sleep:
//! `request_repaint_after` caps nothing while other code calls `request_repaint`.
//!
//! Remove the cap and turn vsync back on once eframe avoids the blocking swap itself
//! (<https://github.com/emilk/egui/pull/8631>).

use std::ffi::OsString;
use std::time::{Duration, Instant};

const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(8);

pub fn wayland_session() -> bool {
    cfg!(all(unix, not(target_os = "macos"))) && is_wayland_session(|name| std::env::var_os(name))
}

/// winit picks Wayland when `WAYLAND_DISPLAY` or `WAYLAND_SOCKET` is set and not empty.
/// `XDG_SESSION_TYPE=wayland` counts too: X11 forced inside a Wayland session (XWayland) also
/// crawls while a window is hidden.
fn is_wayland_session(var: impl Fn(&str) -> Option<OsString>) -> bool {
    let set = |name| var(name).is_some_and(|value| !value.is_empty());
    set("WAYLAND_DISPLAY")
        || set("WAYLAND_SOCKET")
        || var("XDG_SESSION_TYPE").is_some_and(|value| value == "wayland")
}

#[derive(Debug, Default)]
pub struct FrameLimiter {
    last_frame: Option<Instant>,
}

impl FrameLimiter {
    pub fn wait(&mut self) {
        if let Some(last) = self.last_frame {
            std::thread::sleep(MIN_FRAME_INTERVAL.saturating_sub(last.elapsed()));
        }
        self.last_frame = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(vars: &[(&str, &str)]) -> bool {
        is_wayland_session(|name| {
            vars.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| OsString::from(v))
        })
    }

    #[test]
    fn wayland_display_or_socket_means_wayland() {
        assert!(session(&[("WAYLAND_DISPLAY", "wayland-0")]));
        assert!(session(&[("WAYLAND_SOCKET", "3")]));
        assert!(session(&[
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("DISPLAY", ":0")
        ]));
    }

    #[test]
    fn empty_wayland_variables_do_not_count() {
        assert!(!session(&[("WAYLAND_DISPLAY", ""), ("DISPLAY", ":0")]));
        assert!(!session(&[("WAYLAND_SOCKET", "")]));
    }

    #[test]
    fn x11_forced_in_a_wayland_session_counts() {
        assert!(session(&[
            ("WAYLAND_DISPLAY", ""),
            ("DISPLAY", ":0"),
            ("XDG_SESSION_TYPE", "wayland"),
        ]));
    }

    #[test]
    fn x11_and_unknown_sessions_are_not_wayland() {
        assert!(!session(&[]));
        assert!(!session(&[("DISPLAY", ":0"), ("XDG_SESSION_TYPE", "x11")]));
        assert!(!session(&[("XDG_SESSION_TYPE", "tty")]));
    }

    #[test]
    fn limiter_sleeps_only_the_remainder() {
        let mut limiter = FrameLimiter::default();
        let start = Instant::now();
        limiter.wait();
        assert!(
            start.elapsed() < MIN_FRAME_INTERVAL,
            "the first frame waits for nothing"
        );
        limiter.wait();
        assert!(start.elapsed() >= MIN_FRAME_INTERVAL);

        limiter.last_frame = Some(Instant::now() - 2 * MIN_FRAME_INTERVAL);
        let late = Instant::now();
        limiter.wait();
        assert!(
            late.elapsed() < MIN_FRAME_INTERVAL,
            "a late frame waits for nothing"
        );
    }
}
