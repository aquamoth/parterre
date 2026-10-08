//! What the About dialog (`app::about`) and macOS's own About panel (`macos::about_panel`) say:
//! the links, the copyright and the version, and where the third-party notices are (#351).

use std::path::{Path, PathBuf};

/// parterre's own words for itself; it names no other product (#349).
pub const TAGLINE: &str = "A revision graph viewer";
pub const WEBSITE: &str = env!("CARGO_PKG_REPOSITORY");
pub const ISSUES: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/issues");
pub const PRIVACY: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/blob/main/docs/privacy.md");
pub const CONTACT: &str = "parterre@trustfall.se";
pub const MAIL: &str = "mailto:parterre@trustfall.se";
/// NOTICE, with its additional terms, and the GPL's text linked from it.
#[cfg(any(target_os = "macos", test))]
const LICENSE_URL: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/blob/main/NOTICE");

// Paths chosen by build.rs.
pub const NOTICE: &str = include_str!(env!("PARTERRE_NOTICE"));
pub const LICENSE: &str = include_str!(env!("PARTERRE_LICENSE"));

/// The file cargo-about writes, which every release package ships.
const THIRD_PARTY: &str = "THIRD-PARTY-NOTICES.html";

/// NOTICE's copyright line as a person writes it: `© 2026 Trustfall AB`.
pub fn copyright() -> String {
    copyright_of(NOTICE)
}

fn copyright_of(notice: &str) -> String {
    let line = notice
        .lines()
        .find(|line| line.starts_with("Copyright"))
        .unwrap_or_default();
    let holder = line.split(" <").next().unwrap_or_default();
    let holder = holder.trim_start_matches("Copyright").trim_start();
    format!("© {}", holder.trim_start_matches("(C)").trim_start())
}

/// [`crate::VERSION`] as macOS's About panel shows it, `Version 0.7.0 (a1b2c3d)`: the version,
/// and the commit if it is in parentheses.
#[cfg(any(target_os = "macos", test))]
pub fn version_and_commit(version: &str) -> (&str, &str) {
    version
        .strip_suffix(')')
        .and_then(|v| v.split_once(" ("))
        .unwrap_or((version, ""))
}

/// The text under macOS's About panel: the tagline, the links, the licence, and the
/// third-party notices at `notices` (a URL) if they are installed.
#[cfg(any(target_os = "macos", test))]
pub fn credits_html(notices: Option<&str>) -> String {
    let notices = notices
        .map(|url| format!("<br><a href=\"{url}\">Third-party licences</a>"))
        .unwrap_or_default();
    format!(
        "<html><head><meta charset=\"utf-8\"></head>\
         <body style=\"font-family: -apple-system; font-size: 11px; text-align: center\">\
         <p>{TAGLINE}</p>\
         <p><a href=\"{WEBSITE}\">Website</a> · <a href=\"{ISSUES}\">Report a bug</a> · \
         <a href=\"{PRIVACY}\">Privacy</a><br><a href=\"{MAIL}\">{CONTACT}</a></p>\
         <p>Free software under the <a href=\"{LICENSE_URL}\">GNU GPL v3</a>,<br>\
         with absolutely no warranty.{notices}</p></body></html>"
    )
}

/// What a bug report wants to know: parterre's version, git's and the system.
pub fn build_info() -> String {
    let git = parterre_core::git::version().unwrap_or_else(|| "not found".into());
    format!(
        "parterre {}\ngit {git}\n{} {}",
        crate::VERSION,
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// The third-party notices installed with this parterre, if there are any: none when it was
/// built from source (`cargo install`, a checkout).
pub fn third_party_notices() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    beside(&exe).into_iter().find(|path| path.is_file())
}

/// Where packages put the notices, for the program at `exe`: next to it (the archives and the
/// MSI), in the app bundle's Resources (macOS), or in the doc folder of the prefix it is in
/// (`/usr/bin` → `/usr/share/doc/parterre`, the .deb and .rpm).
fn beside(exe: &Path) -> Vec<PathBuf> {
    let Some(dir) = exe.parent() else {
        return Vec::new();
    };
    let mut places = vec![dir.join(THIRD_PARTY)];
    if let Some(up) = dir.parent() {
        places.push(up.join("Resources").join(THIRD_PARTY));
        places.push(up.join("share/doc/parterre").join(THIRD_PARTY));
    }
    places
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copyright_is_notices_without_the_address() {
        let notice = "parterre, a viewer\nCopyright (C) 2026 Trustfall AB <a@example.com>\n";
        assert_eq!(copyright_of(notice), "© 2026 Trustfall AB");
        assert!(copyright().starts_with("© 20"), "{}", copyright());
    }

    #[test]
    fn the_commit_goes_in_parentheses_when_there_is_one() {
        assert_eq!(
            version_and_commit("0.7.0-rc3 (85656da)"),
            ("0.7.0-rc3", "85656da")
        );
        assert_eq!(
            version_and_commit("0.7.1-dev.3+a1b2c3d"),
            ("0.7.1-dev.3+a1b2c3d", "")
        );
    }

    #[test]
    fn the_credits_link_the_notices_only_where_they_are() {
        let url = "file:///Applications/parterre.app/Contents/Resources/THIRD-PARTY-NOTICES.html";
        assert!(credits_html(Some(url)).contains(&format!("<a href=\"{url}\">Third-party")));
        let without = credits_html(None);
        assert!(!without.contains("Third-party"));
        assert!(without.contains(">parterre@trustfall.se</a>") && without.contains(TAGLINE));
    }

    #[test]
    fn notices_are_looked_for_where_each_package_puts_them() {
        let places = beside(Path::new("/usr/bin/parterre"));
        assert!(places.contains(&PathBuf::from("/usr/bin/THIRD-PARTY-NOTICES.html")));
        assert!(places.contains(&PathBuf::from(
            "/usr/share/doc/parterre/THIRD-PARTY-NOTICES.html"
        )));
        let bundle = Path::new("/Applications/parterre.app/Contents/MacOS/parterre");
        assert!(beside(bundle).contains(&PathBuf::from(
            "/Applications/parterre.app/Contents/Resources/THIRD-PARTY-NOTICES.html"
        )));
    }
}
