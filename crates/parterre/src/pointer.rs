//! The size of the system's mouse pointer, for drawing things beside it without covering it.
//! egui doesn't know it, and it varies: Windows' pointers are bigger than Linux's and grow
//! with the accessibility "pointer size" setting.

use std::sync::OnceLock;

/// The pointer's nominal size in points (the square its image is drawn in), as far as it can be
/// found out: on Windows the accessibility setting, on Linux `XCURSOR_SIZE`, else a usual size.
pub fn size() -> f32 {
    static SIZE: OnceLock<f32> = OnceLock::new();
    #[cfg(windows)]
    {
        // `reg` takes a moment to start, so it is asked once, in the background; until it
        // answers, the default.
        static ASKED: std::sync::Once = std::sync::Once::new();
        ASKED.call_once(|| {
            std::thread::spawn(|| SIZE.set(windows::size().unwrap_or(DEFAULT)));
        });
        SIZE.get().copied().unwrap_or(DEFAULT)
    }
    #[cfg(not(windows))]
    {
        *SIZE.get_or_init(|| {
            std::env::var("XCURSOR_SIZE")
                .ok()
                .and_then(|s| s.trim().parse::<f32>().ok())
                .map_or(DEFAULT, clamp)
        })
    }
}

/// Windows' size at 100 % scaling with the smallest pointers; Linux desktops use 24.
#[cfg(windows)]
const DEFAULT: f32 = 32.0;
#[cfg(not(windows))]
const DEFAULT: f32 = 24.0;

fn clamp(size: f32) -> f32 {
    size.clamp(16.0, 256.0)
}

/// `CursorBaseSize` from the output of `reg query "HKCU\Control Panel\Cursors"`: a line like
/// `    CursorBaseSize    REG_DWORD    0x40`. Windows keeps it in pixels at 96 DPI, and scales
/// the pointer with the display as egui does its points, so it is the size in points.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_base_size(output: &str) -> Option<f32> {
    let line = output
        .lines()
        .find(|l| l.split_whitespace().next() == Some("CursorBaseSize"))?;
    let mut words = line.split_whitespace().skip(1);
    if words.next()? != "REG_DWORD" {
        return None;
    }
    let hex = words.next()?.strip_prefix("0x")?;
    let size = u32::from_str_radix(hex, 16).ok()?;
    Some(clamp(size as f32))
}

#[cfg(windows)]
mod windows {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    /// The pointer size set in Settings (Accessibility, Mouse pointer and touch), if set.
    pub fn size() -> Option<f32> {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let output = Command::new("reg")
            .args([
                "query",
                r"HKCU\Control Panel\Cursors",
                "/v",
                "CursorBaseSize",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        super::parse_base_size(&String::from_utf8_lossy(&output.stdout))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_base_size_from_reg_output() {
        let output = "\r\nHKEY_CURRENT_USER\\Control Panel\\Cursors\r\n    \
                      CursorBaseSize    REG_DWORD    0x40\r\n\r\n";
        assert_eq!(parse_base_size(output), Some(64.0));
        // Out of range: kept to sizes a pointer can have.
        let huge = "    CursorBaseSize    REG_DWORD    0xffff\r\n";
        assert_eq!(parse_base_size(huge), Some(256.0));
        // Not there, or not a number.
        assert_eq!(
            parse_base_size("ERROR: The system was unable to find"),
            None
        );
        let text = "    CursorBaseSize    REG_SZ    big\r\n";
        assert_eq!(parse_base_size(text), None);
    }
}
