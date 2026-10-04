//! Crash reports (#263): whether panics are sent to PostHog, and the home folder scrubbed from
//! what is sent. No network here, so all of it is tested without one; the sending is in
//! `posthog.rs`.

use crate::{Build, Choices};

/// Whether panics are sent as crash reports: a release build with `send`, the first-run prompt
/// answered (`choices`) with crash reports ticked, and `DO_NOT_TRACK` not set. Usage statistics
/// don't matter: crash reports are a switch of their own (#225).
pub fn sends_crash_reports(build: Build, choices: Option<Choices>, do_not_track: bool) -> bool {
    build.send && !build.debug && !do_not_track && choices.is_some_and(|c| c.crash_reports)
}

/// The user's home folders as paths name them: the home folder, and where it really is when a
/// symbolic link leads there. In a snap, the real one, not the snap's own.
#[cfg_attr(not(feature = "send"), allow(dead_code))]
pub(crate) fn homes() -> Vec<String> {
    let home = std::env::var_os("SNAP_REAL_HOME")
        .filter(|h| !h.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(std::env::home_dir);
    let Some(home) = home else {
        return Vec::new();
    };
    let mut homes = vec![home.to_string_lossy().into_owned()];
    // On Windows that would be a `\\?\` path, which already contains the home folder.
    if cfg!(unix)
        && let Ok(real) = home.canonicalize()
    {
        let real = real.to_string_lossy().into_owned();
        if !homes.contains(&real) {
            homes.push(real);
        }
    }
    homes
}

/// `text` with every path into one of the `homes` (or the home folder itself) starting with
/// `~` instead: `/home/x/repo` becomes `~/repo`. Separators may be either slash, or doubled as
/// in a debug-printed Windows path (`C:\\Users\\x`), and letters may differ in case, as they
/// may on Windows and macOS. `/home/xavier` is not in `/home/x`.
#[cfg_attr(not(feature = "send"), allow(dead_code))]
pub(crate) fn scrub(text: &str, homes: &[String]) -> String {
    let homes: Vec<Home> = homes.iter().filter_map(|h| Home::parse(h)).collect();
    let mut scrubbed = String::with_capacity(text.len());
    let mut previous = None;
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        let at_start = !previous.is_some_and(is_name);
        let found = at_start
            .then(|| homes.iter().filter_map(|h| h.prefix_of(rest)).max())
            .flatten();
        if let Some(length) = found {
            scrubbed.push('~');
            previous = rest[..length].chars().next_back();
            rest = &rest[length..];
        } else {
            scrubbed.push(c);
            previous = Some(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    scrubbed
}

/// A home folder, as the folder names along its path.
struct Home<'a> {
    /// It starts at a separator: `/home/x`, or `\\server\share\x`.
    rooted: bool,
    /// `["home", "x"]`, or `["C:", "Users", "x"]`.
    names: Vec<&'a str>,
}

impl<'a> Home<'a> {
    /// `None` for no folder at all, such as `/`.
    fn parse(home: &'a str) -> Option<Home<'a>> {
        let names: Vec<_> = home.split(is_separator).filter(|n| !n.is_empty()).collect();
        (!names.is_empty()).then(|| Home {
            rooted: home.starts_with(is_separator),
            names,
        })
    }

    /// How long the home folder is at the start of `text`, if it's there, ending where the
    /// folder's name ends.
    fn prefix_of(&self, text: &str) -> Option<usize> {
        let mut rest = text;
        for (i, name) in self.names.iter().enumerate() {
            if i > 0 || self.rooted {
                let after = rest.trim_start_matches(is_separator);
                if after.len() == rest.len() {
                    return None;
                }
                rest = after;
            }
            let head = rest.get(..name.len())?;
            if !head.eq_ignore_ascii_case(name) {
                return None;
            }
            rest = &rest[name.len()..];
        }
        if rest.chars().next().is_some_and(is_name) {
            return None;
        }
        Some(text.len() - rest.len())
    }
}

fn is_separator(c: char) -> bool {
    c == '/' || c == '\\'
}

/// A character that can go on a folder's name, so that a match ending before it would end in
/// the middle of one.
fn is_name(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '.')
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: Build = Build {
        send: true,
        debug: false,
    };
    const TICKED: Option<Choices> = Some(Choices {
        usage_statistics: false,
        crash_reports: true,
    });
    const UNTICKED: Option<Choices> = Some(Choices {
        usage_statistics: true,
        crash_reports: false,
    });

    fn homes(homes: &[&str]) -> Vec<String> {
        homes.iter().map(|h| h.to_string()).collect()
    }

    #[test]
    fn crash_reports_are_sent_only_when_ticked() {
        assert!(sends_crash_reports(RELEASE, TICKED, false));
        assert!(!sends_crash_reports(RELEASE, UNTICKED, false));
        // Before the first-run prompt is answered.
        assert!(!sends_crash_reports(RELEASE, None, false));
    }

    #[test]
    fn do_not_track_sends_no_crash_reports() {
        assert!(!sends_crash_reports(RELEASE, TICKED, true));
    }

    #[test]
    fn debug_builds_and_builds_without_send_send_no_crash_reports() {
        for build in [
            Build {
                send: true,
                debug: true,
            },
            Build {
                send: false,
                debug: false,
            },
        ] {
            assert!(!sends_crash_reports(build, TICKED, false), "{build:?}");
        }
        if cfg!(debug_assertions) || !cfg!(feature = "send") {
            assert!(!sends_crash_reports(Build::THIS, TICKED, false));
        }
    }

    #[test]
    fn linux_home_folders_become_a_tilde() {
        let home = homes(&["/home/x"]);
        assert_eq!(
            scrub("cannot open /home/x/repos/app/.git/index: denied", &home),
            "cannot open ~/repos/app/.git/index: denied"
        );
        assert_eq!(
            scrub("/home/x/.cargo/bin/parterre", &home),
            "~/.cargo/bin/parterre"
        );
        assert_eq!(scrub("in /home/x", &home), "in ~");
        assert_eq!(scrub("\"/home/x/\"", &home), "\"~/\"");
        assert_eq!(scrub("/home/x/a and /home/x/b", &home), "~/a and ~/b");
        // A trailing separator on the home folder changes nothing.
        assert_eq!(scrub("/home/x/a", &homes(&["/home/x/"])), "~/a");
    }

    #[test]
    fn macos_home_folders_become_a_tilde() {
        let home = homes(&["/Users/x"]);
        assert_eq!(
            scrub(
                "/Users/x/Applications/parterre.app/Contents/MacOS/parterre",
                &home
            ),
            "~/Applications/parterre.app/Contents/MacOS/parterre"
        );
        // APFS ignores case by default.
        assert_eq!(scrub("/users/X/Desktop/repo", &home), "~/Desktop/repo");
    }

    #[test]
    fn windows_home_folders_become_a_tilde_with_either_slash() {
        let home = homes(&[r"C:\Users\x"]);
        assert_eq!(
            scrub(r"C:\Users\x\AppData\Local\parterre\parterre.exe", &home),
            r"~\AppData\Local\parterre\parterre.exe"
        );
        assert_eq!(scrub("C:/Users/x/source/repo", &home), "~/source/repo");
        assert_eq!(scrub(r"c:\users\X\source", &home), r"~\source");
        // Debug-printed, as in `{:?}` of a path in a panic message.
        assert_eq!(
            scrub(r#"Err(Os { path: "C:\\Users\\x\\repo" })"#, &home),
            r#"Err(Os { path: "~\\repo" })"#
        );
        // Verbatim paths.
        assert_eq!(scrub(r"\\?\C:\Users\x\repo", &home), r"\\?\~\repo");
        // A home folder given with forward slashes.
        assert_eq!(
            scrub(r"C:\Users\x\repo", &homes(&["C:/Users/x"])),
            r"~\repo"
        );
    }

    #[test]
    fn other_folders_are_left_as_they_are() {
        let home = homes(&["/home/x", r"C:\Users\x"]);
        for text in [
            "/home/xavier/repo",
            "/home/x.old/repo",
            "/home/x-y",
            "/mnt/home/x/repo",
            "/home/y/repo",
            "/tmp/x",
            r"D:\Users\x\repo",
            r"C:\Users\xavier",
            "crates/parterre/src/app.rs",
            "home/x",
            "",
        ] {
            assert_eq!(scrub(text, &home), text, "{text:?}");
        }
    }

    #[test]
    fn the_whole_home_folder_is_scrubbed_where_one_is_in_another() {
        // A snap's home lies in the real one; a link may lead to the home folder.
        let home = homes(&["/home/x", "/var/home/x"]);
        assert_eq!(scrub("/var/home/x/repo", &home), "~/repo");
        assert_eq!(
            scrub("/home/x/snap/parterre/12", &home),
            "~/snap/parterre/12"
        );
    }

    #[test]
    fn no_home_folder_scrubs_nothing() {
        let text = "/home/x/repo";
        assert_eq!(scrub(text, &[]), text);
        assert_eq!(scrub(text, &homes(&["/", "", r"\"])), text);
    }

    #[test]
    fn letters_beyond_ascii_are_kept_whole() {
        let home = homes(&["/home/åsa"]);
        assert_eq!(scrub("/home/åsa/räksmörgås ✓", &home), "~/räksmörgås ✓");
        assert_eq!(scrub("/home/åsar", &home), "/home/åsar");
    }
}
