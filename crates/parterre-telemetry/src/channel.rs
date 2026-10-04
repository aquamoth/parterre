//! The channels parterre is installed through, as far as the update check cares (#258).
//!
//! `build.rs` includes this file to check the stamp a packaging job gives, so it must not use
//! anything outside `std`.

/// Where this copy of parterre came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// The Windows installer, which winget and Chocolatey install too.
    Msi,
    /// The Windows zip.
    Zip,
    /// The Linux and macOS tarballs.
    Tarball,
    Deb,
    Rpm,
    /// `cargo install`, and any build without a stamp.
    Cargo,
    Snap,
    Flatpak,
}

impl Channel {
    /// The channels whose packaging job stamps them into the build, by the name it sets
    /// `PARTERRE_CHANNEL` to (`.github/workflows/release.yml`).
    pub const STAMPED: [Channel; 5] = [
        Channel::Msi,
        Channel::Zip,
        Channel::Tarball,
        Channel::Deb,
        Channel::Rpm,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Channel::Msi => "msi",
            Channel::Zip => "zip",
            Channel::Tarball => "tarball",
            Channel::Deb => "deb",
            Channel::Rpm => "rpm",
            Channel::Cargo => "cargo",
            Channel::Snap => "snap",
            Channel::Flatpak => "flatpak",
        }
    }

    /// The stamped channel named `name`.
    pub fn stamped(name: &str) -> Option<Channel> {
        Channel::STAMPED.into_iter().find(|c| c.name() == name)
    }
}
