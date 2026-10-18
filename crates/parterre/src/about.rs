use std::path::{Path, PathBuf};

/// Names no other product (#349).
pub const TAGLINE: &str = "A revision graph viewer";
pub const WEBSITE: &str = env!("CARGO_PKG_REPOSITORY");
pub const ISSUES: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/issues");
pub const PRIVACY: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/blob/main/docs/privacy.md");
pub const CONTACT: &str = "parterre@trustfall.se";
pub const MAIL: &str = "mailto:parterre@trustfall.se";
/// NOTICE rather than LICENSE: it adds terms to the GPL, and links the GPL's text.
#[cfg(any(target_os = "macos", test))]
const LICENSE_URL: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/blob/main/NOTICE");

pub const NOTICE: &str = include_str!(env!("PARTERRE_NOTICE"));
pub const LICENSE: &str = include_str!(env!("PARTERRE_LICENSE"));

/// Written by cargo-about, and shipped in every release package.
const THIRD_PARTY_NOTICES_FILE: &str = "THIRD-PARTY-NOTICES.html";

/// NOTICE's copyright line as a person writes it: `© 2026 Trustfall AB`.
pub fn copyright() -> String {
    copyright_of(NOTICE)
}

fn copyright_of(notice: &str) -> String {
    let line = crate::notice::copyright_line(notice);
    let holder = line.trim_start_matches("Copyright").trim_start();
    format!("© {}", holder.trim_start_matches("(C)").trim_start())
}

/// A published `1.2.3-rc+a1b2c3d` as `("1.2.3-rc", "a1b2c3d")`; a dev build, or a version
/// without a commit, as `(version, "")`.
#[cfg(any(target_os = "macos", test))]
pub fn version_and_commit(version: &str) -> (&str, &str) {
    version
        .split_once('+')
        .filter(|(v, _)| !v.contains("dev"))
        .unwrap_or((version, ""))
}

/// The text under macOS's About panel, linking the third-party notices at the URL `notices`
/// if they are installed.
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

pub fn build_info() -> String {
    let git = parterre_core::git::version().unwrap_or_else(|| "not found".into());
    format!(
        "parterre {}\ngit {git}\n{} {}",
        crate::version(),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// `None` for a build from source (`cargo install`, a checkout), which has no notices.
pub fn third_party_notices() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    third_party_notices_places(&exe)
        .into_iter()
        .find(|path| path.is_file())
}

fn third_party_notices_places(exe: &Path) -> Vec<PathBuf> {
    let Some(dir) = exe.parent() else {
        return Vec::new();
    };
    // Beside the program: the archives and the MSI.
    let mut places = vec![dir.join(THIRD_PARTY_NOTICES_FILE)];
    if let Some(up) = dir.parent() {
        // The macOS app bundle.
        places.push(up.join("Resources").join(THIRD_PARTY_NOTICES_FILE));
        // The .deb and .rpm: `/usr/bin` → `/usr/share/doc/parterre`.
        places.push(up.join("share/doc/parterre").join(THIRD_PARTY_NOTICES_FILE));
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
            version_and_commit("1.2.3-rc+85656da"),
            ("1.2.3-rc", "85656da")
        );
        assert_eq!(version_and_commit("1.2.3+85656da"), ("1.2.3", "85656da"));
        assert_eq!(
            version_and_commit("1.2.3-dev+a1b2c3d"),
            ("1.2.3-dev+a1b2c3d", "")
        );
        assert_eq!(version_and_commit("0.0.0-dev"), ("0.0.0-dev", ""));
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
        let places = third_party_notices_places(Path::new("/usr/bin/parterre"));
        assert!(places.contains(&PathBuf::from("/usr/bin/THIRD-PARTY-NOTICES.html")));
        assert!(places.contains(&PathBuf::from(
            "/usr/share/doc/parterre/THIRD-PARTY-NOTICES.html"
        )));
        let bundle = Path::new("/Applications/parterre.app/Contents/MacOS/parterre");
        assert!(third_party_notices_places(bundle).contains(&PathBuf::from(
            "/Applications/parterre.app/Contents/Resources/THIRD-PARTY-NOTICES.html"
        )));
    }
}
