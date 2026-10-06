//! The window icon: the app icon from `parterre_core::icon`, rasterised at startup.

use std::path::Path;

use eframe::egui::IconData;

const SIZE: u32 = 64;

/// On macOS eframe makes the window icon the Dock's too. Inside `parterre.app` none is set, an
/// empty one to eframe, so the Dock keeps the bundle's `.icns`: sharp at every size, with the
/// margin macOS gives app icons.
pub fn icon() -> IconData {
    if cfg!(target_os = "macos") && std::env::current_exe().is_ok_and(|exe| in_app_bundle(&exe)) {
        return IconData::default();
    }
    IconData {
        rgba: parterre_core::icon::render(SIZE),
        width: SIZE,
        height: SIZE,
    }
}

/// Whether `exe` is the program of an app bundle, `….app/Contents/MacOS/parterre`.
fn in_app_bundle(exe: &Path) -> bool {
    exe.parent()
        .is_some_and(|dir| dir.ends_with("Contents/MacOS"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_has_opaque_and_transparent_pixels() {
        let icon = super::icon();
        assert_eq!(icon.rgba.len(), 64 * 64 * 4);
        let alphas: Vec<u8> = icon.rgba.chunks(4).map(|p| p[3]).collect();
        assert!(alphas.contains(&0) && alphas.contains(&255));
    }

    #[test]
    fn the_program_of_an_app_bundle_is_told_apart() {
        let bundled = std::path::Path::new("/Applications/parterre.app/Contents/MacOS/parterre");
        assert!(super::in_app_bundle(bundled));
        let bare = std::path::Path::new("/usr/local/bin/parterre");
        assert!(!super::in_app_bundle(bare));
    }
}
