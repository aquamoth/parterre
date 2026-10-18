//! Runs the platform's opener rather than using eframe's `links` feature, whose `webbrowser`
//! crate brings a dozen crates for URL parsing (`docs/research/github-forks-and-pull-requests.md`,
//! §14).

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::about::MAIL;

const ALLOWED_PREFIXES: [&str; 2] = ["https://github.com/", "https://dev.azure.com/"];

/// Opens `url` in the default browser, or [`MAIL`] in the mail app. Any other URL is an error.
pub fn open(url: &str) -> Result<(), String> {
    if !is_allowed(url) {
        return Err(format!(
            "not opening {url}: only {} pages are opened",
            ALLOWED_PREFIXES.join(" and ")
        ));
    }
    spawn_detached(opener(url)).map_err(|e| format!("could not open {url}: {e}"))
}

/// Opens the file at `path` in the app the system opens its kind with.
pub fn open_file(path: &Path) -> Result<(), String> {
    spawn_detached(opener(path)).map_err(|e| format!("could not open {}: {e}", path.display()))
}

/// Starts `cmd` with no input or output, without waiting: an opener may run as long as what it
/// opens.
pub fn spawn_detached(mut cmd: Command) -> std::io::Result<()> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    // Reaped, so no zombie is left behind.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Nothing in an allowed URL is something an opener or a browser could take for more than a URL.
fn is_allowed(url: &str) -> bool {
    url == MAIL
        || ALLOWED_PREFIXES
            .iter()
            .any(|prefix| url.starts_with(prefix))
            && url
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~/:?#=&%".contains(&b))
}

#[cfg(target_os = "macos")]
fn opener(url: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new("open");
    cmd.arg(url);
    cmd
}

#[cfg(windows)]
fn opener(url: impl AsRef<OsStr>) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // Explorer hands a URL to the default browser. Not `cmd /c start`, which would read `&`
    // and `^` in it as commands.
    let mut cmd = Command::new("explorer.exe");
    cmd.arg(url);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(not(any(windows, target_os = "macos")))]
fn opener(url: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new("xdg-open");
    cmd.arg(url);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_forge_pages_and_the_contact_address_are_opened() {
        assert!(is_allowed("https://github.com/aquamoth/parterre/pull/12"));
        assert!(is_allowed(
            "https://github.com/my-org/some.repo_name/pull/3"
        ));
        assert!(is_allowed(
            "https://dev.azure.com/cosmojpn/CJP%20Work/_git/Apps/pullrequest/7"
        ));
        assert!(is_allowed(MAIL));
        for url in [
            "http://github.com/aquamoth/parterre/pull/12",
            "https://github.com.evil.example/x",
            "https://gist.github.com/x",
            "https://dev.azure.com.evil.example/x",
            "https://cosmojpn.visualstudio.com/Apps/_git/Apps/pullrequest/7",
            "https://dev.azure.com/o/My Project/_git/r/pullrequest/1",
            "file:///etc/passwd",
            "mailto:someone@example.com",
            "mailto:parterre@trustfall.se?subject=x",
            "https://github.com/a/b/pull/1 --new-window",
            "https://github.com/a/b/pull/1\"",
            "https://github.com/a/b/pull/1;rm",
            "https://github.com/a/b/pull/1^",
            "",
        ] {
            assert!(!is_allowed(url), "{url}");
        }
        assert!(open("file:///etc/passwd").is_err());
    }
}
