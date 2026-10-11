//! `build.rs` includes this file, so it must use nothing outside `std`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// The Windows installer, which winget and Chocolatey install too.
    Msi,
    Zip,
    /// The Linux and macOS tarballs.
    Tarball,
    Dmg,
    Deb,
    Rpm,
    /// `cargo install`, and any build without a stamp.
    Cargo,
    Snap,
    Flatpak,
}

impl Channel {
    /// Those a packaging job stamps into the build as `PARTERRE_CHANNEL`
    /// (`.github/workflows/release.yml`).
    pub const STAMPED: [Channel; 6] = [
        Channel::Msi,
        Channel::Zip,
        Channel::Tarball,
        Channel::Dmg,
        Channel::Deb,
        Channel::Rpm,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Channel::Msi => "msi",
            Channel::Zip => "zip",
            Channel::Tarball => "tarball",
            Channel::Dmg => "dmg",
            Channel::Deb => "deb",
            Channel::Rpm => "rpm",
            Channel::Cargo => "cargo",
            Channel::Snap => "snap",
            Channel::Flatpak => "flatpak",
        }
    }

    pub fn stamped(name: &str) -> Option<Channel> {
        Channel::STAMPED.into_iter().find(|c| c.name() == name)
    }
}
