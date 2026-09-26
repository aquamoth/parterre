//! Opening a web page in the default browser, by running the platform's opener with the URL as
//! its argument (no shell). eframe's `links` feature would do it through the `webbrowser` crate,
//! which brings a dozen crates for URL parsing; see the research on GitHub pull requests (§14).

use std::process::{Command, Stdio};

/// The only pages parterre opens.
const ALLOWED: &str = "https://github.com/";

/// Opens `url` in the default browser. Only github.com pages are opened; anything else is an
/// error, as is an opener that can't be started.
pub fn open(url: &str) -> Result<(), String> {
    if !is_allowed(url) {
        return Err(format!(
            "not opening {url}: only {ALLOWED} pages are opened"
        ));
    }
    let mut cmd = opener(url);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not open {url}: {e}"))?;
    // Reaped in the background, so no zombie is left behind.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// A github.com page, with nothing in it that an opener or a browser could take for more
/// than a URL.
fn is_allowed(url: &str) -> bool {
    url.starts_with(ALLOWED)
        && url
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~/:?#=&%".contains(&b))
}

#[cfg(target_os = "macos")]
fn opener(url: &str) -> Command {
    let mut cmd = Command::new("open");
    cmd.arg(url);
    cmd
}

#[cfg(windows)]
fn opener(url: &str) -> Command {
    use std::os::windows::process::CommandExt;
    // Explorer hands a URL to the default browser. Not `cmd /c start`, which would read `&`
    // and `^` in it as commands.
    let mut cmd = Command::new("explorer.exe");
    cmd.arg(url);
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(not(any(windows, target_os = "macos")))]
fn opener(url: &str) -> Command {
    let mut cmd = Command::new("xdg-open");
    cmd.arg(url);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_github_pages_are_opened() {
        assert!(is_allowed("https://github.com/aquamoth/parterre/pull/12"));
        assert!(is_allowed(
            "https://github.com/my-org/some.repo_name/pull/3"
        ));
        for url in [
            "http://github.com/aquamoth/parterre/pull/12",
            "https://github.com.evil.example/x",
            "https://gist.github.com/x",
            "file:///etc/passwd",
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
