//! How this copy of parterre was installed. Each platform has one release binary, and a package
//! says it wrapped it with a one-word file named `channel` at a fixed place beside it (#410).

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// The Windows installer, installed for the user: winget installs it so too.
    Msi,
    /// The Windows installer, installed for all users, into Program Files: Chocolatey's, or
    /// winget's machine-wide install. Takes stable releases only, since Windows Installer never
    /// upgrades it with a per-user installer (#393).
    MsiMachine,
    Zip,
    /// The Linux and macOS tarballs.
    Tarball,
    Dmg,
    Deb,
    Rpm,
    /// `cargo install`, and any build whose version wasn't written by the pipeline.
    Cargo,
    Snap,
    Flatpak,
}

impl Channel {
    pub fn name(self) -> &'static str {
        match self {
            Channel::Msi => "msi",
            Channel::MsiMachine => "msi-machine",
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

    /// Offered stable releases only, whatever the installation's ring: crates.io has nothing
    /// else, and a machine-wide MSI can't take a GitHub build (#398).
    pub fn is_stable_only(self) -> bool {
        matches!(self, Channel::MsiMachine | Channel::Cargo)
    }

    /// From the package's marker (`channel`'s word), whether the pipeline wrote this build's
    /// version (`published`), the environment, and where the binary runs from.
    pub(crate) fn of(
        marker: Option<&str>,
        published: bool,
        var: impl Fn(&str) -> Option<String>,
        exe: Option<&Path>,
    ) -> Channel {
        // Snap and Flatpak set the name of the app they run, and leak it to what is started
        // from a terminal there, so only parterre's own counts.
        if var("SNAP_NAME").as_deref() == Some("parterre") {
            return Channel::Snap;
        }
        if var("FLATPAK_ID").as_deref() == Some("se.trustfall.parterre") {
            return Channel::Flatpak;
        }
        match marker.map(str::trim) {
            Some("msi") if exe.is_some_and(|exe| in_program_files(exe, &var)) => {
                Channel::MsiMachine
            }
            Some("msi") => Channel::Msi,
            Some("dmg") => Channel::Dmg,
            Some("deb") => Channel::Deb,
            Some("rpm") => Channel::Rpm,
            // A published binary on its own is the archive of its platform: a copy taken out of
            // a package too, which is the right offer for a loose copy.
            _ if published && cfg!(windows) => Channel::Zip,
            _ if published => Channel::Tarball,
            _ => Channel::Cargo,
        }
    }
}

/// Where a package puts its marker, from the binary: beside `parterre.exe`, in the app
/// bundle's `Resources`, or in `/usr/lib/parterre` for `/usr/bin/parterre`.
pub(crate) fn marker_path(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    Some(if cfg!(windows) {
        dir.join("channel")
    } else if cfg!(target_os = "macos") {
        dir.parent()?.join("Resources/channel")
    } else {
        dir.parent()?.join("lib/parterre/channel")
    })
}

/// The marker's word; a file of more than a word is no marker.
pub(crate) fn read_marker(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > 64 {
        return None;
    }
    Some(std::fs::read_to_string(path).ok()?.trim().to_owned())
}

/// Under `%ProgramFiles%` (or `%ProgramW6432%`, the 64-bit one), where a machine-wide MSI
/// installs; a per-user one installs under `%LOCALAPPDATA%\Programs`.
fn in_program_files(exe: &Path, var: impl Fn(&str) -> Option<String>) -> bool {
    let exe = exe.to_string_lossy().to_lowercase();
    ["ProgramFiles", "ProgramW6432"]
        .into_iter()
        .filter_map(var)
        .filter(|dir| !dir.is_empty())
        .any(|dir| {
            let dir = dir.trim_end_matches(['\\', '/']).to_lowercase();
            exe.strip_prefix(&dir)
                .is_some_and(|rest| rest.starts_with(['\\', '/']))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'a {
        |name| {
            vars.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn the_marker_names_the_package() {
        for (word, channel) in [
            ("msi", Channel::Msi),
            ("dmg", Channel::Dmg),
            ("deb", Channel::Deb),
            ("rpm", Channel::Rpm),
            ("deb\n", Channel::Deb),
        ] {
            assert_eq!(Channel::of(Some(word), true, env(&[]), None), channel);
        }
    }

    #[test]
    fn without_a_marker_a_published_build_is_the_archive_and_any_other_cargo() {
        let archive = if cfg!(windows) {
            Channel::Zip
        } else {
            Channel::Tarball
        };
        assert_eq!(Channel::of(None, true, env(&[]), None), archive);
        assert_eq!(Channel::of(Some("unknown"), true, env(&[]), None), archive);
        assert_eq!(Channel::of(None, false, env(&[]), None), Channel::Cargo);
    }

    #[test]
    fn an_msi_in_program_files_is_installed_for_all_users() {
        let vars = [
            ("ProgramFiles", r"C:\Program Files"),
            ("LOCALAPPDATA", r"C:\Users\me\AppData\Local"),
        ];
        let at = |exe: &str| Channel::of(Some("msi"), true, env(&vars), Some(Path::new(exe)));
        assert_eq!(
            at(r"C:\Program Files\parterre\parterre.exe"),
            Channel::MsiMachine
        );
        assert_eq!(
            at(r"c:\program files\parterre\parterre.exe"),
            Channel::MsiMachine
        );
        assert_eq!(
            at(r"C:\Users\me\AppData\Local\Programs\parterre\parterre.exe"),
            Channel::Msi
        );
        assert_eq!(
            at(r"C:\Program Files (x86)\parterre\parterre.exe"),
            Channel::Msi
        );
        assert!(Channel::MsiMachine.is_stable_only() && Channel::Cargo.is_stable_only());
        assert!(!Channel::Msi.is_stable_only());
    }

    #[test]
    fn snap_and_flatpak_are_told_by_their_own_app() {
        let snap = [("SNAP_NAME", "parterre")];
        assert_eq!(Channel::of(None, false, env(&snap), None), Channel::Snap);
        let flatpak = [("FLATPAK_ID", "se.trustfall.parterre")];
        assert_eq!(
            Channel::of(None, false, env(&flatpak), None),
            Channel::Flatpak
        );
        // Started from a terminal in another app's snap or flatpak.
        let code = [
            ("SNAP_NAME", "code"),
            ("FLATPAK_ID", "com.visualstudio.code"),
        ];
        assert_eq!(Channel::of(None, false, env(&code), None), Channel::Cargo);
        assert_eq!(
            Channel::of(Some("deb"), true, env(&code), None),
            Channel::Deb
        );
    }

    #[test]
    fn the_marker_is_a_word_in_a_small_file() {
        let dir = std::env::temp_dir().join(format!("parterre-marker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("channel");
        std::fs::write(&marker, "rpm\n").unwrap();
        assert_eq!(read_marker(&marker).as_deref(), Some("rpm"));
        std::fs::write(&marker, "x".repeat(100)).unwrap();
        assert_eq!(read_marker(&marker), None);
        assert_eq!(read_marker(&dir.join("none")), None);
        assert_eq!(read_marker(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
        let exe = Path::new("/opt/x/bin/parterre");
        let expected = if cfg!(windows) {
            "/opt/x/bin/channel"
        } else if cfg!(target_os = "macos") {
            "/opt/x/Resources/channel"
        } else {
            "/opt/x/lib/parterre/channel"
        };
        assert_eq!(marker_path(exe), Some(PathBuf::from(expected)));
    }
}
