//! The `PATH` the programs parterre starts get (#326).
//!
//! Started from the Dock, Finder or Spotlight, a macOS app has launchd's `PATH`,
//! `/usr/bin:/bin:/usr/sbin:/sbin`, not the one the user's shell sets up, so `gh`, git hooks,
//! merge tools and a newer `git` from Homebrew and the like would not be found. Then, as VS Code
//! and Zed do, the user's shell is asked for its `PATH` once, as a login and interactive shell
//! so that it reads what a terminal's does, and git and `gh` get that. Started from a terminal,
//! or on another system, parterre's own `PATH` is passed on as it is.

// Only macOS asks a shell; asking is tested on every Unix.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::ffi::OsString;
use std::process::Command;
#[cfg(unix)]
use std::process::{Child, Stdio};
#[cfg(target_os = "macos")]
use std::sync::OnceLock;
#[cfg(unix)]
use std::time::{Duration, Instant};

/// How long the shell may take: a profile that hangs leaves launchd's `PATH`.
#[cfg(unix)]
const TIMEOUT: Duration = Duration::from_secs(5);
/// Printed around the `PATH`, so that whatever the profile prints is left out.
#[cfg(unix)]
const MARKER: &str = "_PARTERRE_PATH_";

/// Asks the shell on a thread of its own, if parterre was started outside a terminal, so that
/// the answer is in by the time the first command needs it.
pub fn start() {
    #[cfg(target_os = "macos")]
    if started_by_launchd() {
        std::thread::spawn(shell_path);
    }
}

/// Gives `cmd` the shell's `PATH`, if parterre was started outside a terminal: waits for the
/// shell's answer, unless it is in. A program named without a folder is looked for in it too.
pub fn apply(cmd: &mut Command) -> &mut Command {
    if let Some(path) = shell_path() {
        cmd.env("PATH", path);
    }
    cmd
}

/// The `PATH` that programs parterre starts get: the shell's, or else parterre's own.
pub fn path() -> Option<OsString> {
    shell_path().cloned().or_else(|| std::env::var_os("PATH"))
}

#[cfg(target_os = "macos")]
fn shell_path() -> Option<&'static OsString> {
    static PATH: OnceLock<Option<OsString>> = OnceLock::new();
    PATH.get_or_init(|| {
        if !started_by_launchd() {
            return None;
        }
        let shell = std::env::var_os("SHELL")
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(|| "/bin/zsh".into());
        ask(Command::new(shell), TIMEOUT)
    })
    .as_ref()
}

#[cfg(not(target_os = "macos"))]
fn shell_path() -> Option<&'static OsString> {
    None
}

/// From the Dock, Finder, Spotlight or `open`.
#[cfg(target_os = "macos")]
fn started_by_launchd() -> bool {
    std::os::unix::process::parent_id() == 1
}

/// The `PATH` that `shell`, run as a login and interactive shell, has; `None` if it fails or
/// takes longer than `timeout`.
#[cfg(unix)]
fn ask(mut shell: Command, timeout: Duration) -> Option<OsString> {
    use std::io::Read;
    use std::os::unix::ffi::OsStringExt;

    // `printenv`, as `$PATH` would be a list in fish.
    let script = format!("echo {MARKER}; /usr/bin/printenv PATH; echo {MARKER}");
    let mut child = shell
        .args(["-l", "-i", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    // Read on a thread of its own, up to the second marker: something the profile starts in
    // the background can hold the output open long after the shell is done.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut seen = Vec::new();
        let mut buf = [0; 4096];
        while between_markers(&seen).is_none() {
            match out.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => seen.extend_from_slice(&buf[..n]),
            }
        }
        let _ = tx.send(between_markers(&seen).map(<[u8]>::to_vec));
    });
    let path = rx.recv_timeout(timeout).ok().flatten();
    reap(child, Instant::now() + timeout);
    path.filter(|path| !path.is_empty()).map(OsString::from_vec)
}

/// What `out` holds between the two markers, without the line breaks around it.
#[cfg(unix)]
fn between_markers(out: &[u8]) -> Option<&[u8]> {
    let marker = MARKER.as_bytes();
    let find = |hay: &[u8]| hay.windows(marker.len()).position(|w| w == marker);
    let start = find(out)? + marker.len();
    let rest = &out[start..];
    Some(rest[..find(rest)?].trim_ascii())
}

/// Waits for the shell, which has answered or run out of time, on a thread of its own, and
/// kills it if it is still there at `deadline`.
#[cfg(unix)]
fn reap(mut child: Child, deadline: Instant) {
    std::thread::spawn(move || {
        loop {
            match child.try_wait() {
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                Ok(Some(_)) | Err(_) => return,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    /// `sh` running `script` in place of the user's shell, which it is given the arguments of.
    fn shell(script: &str) -> (tempfile::TempDir, Command) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("shell");
        std::fs::write(&file, script).unwrap();
        let mut cmd = Command::new("sh");
        cmd.arg(file);
        (dir, cmd)
    }

    #[cfg(unix)]
    #[test]
    fn the_path_comes_from_between_the_markers() {
        let out = b"Last login: today\n_PARTERRE_PATH_\n/opt/homebrew/bin:/usr/bin\n_PARTERRE_PATH_\nbye\n";
        assert_eq!(
            between_markers(out),
            Some(&b"/opt/homebrew/bin:/usr/bin"[..])
        );
        assert_eq!(between_markers(b"_PARTERRE_PATH_\n/usr/bin\n"), None);
    }

    #[cfg(unix)]
    #[test]
    fn the_profile_sets_the_path_and_what_it_prints_or_leaves_running_does_not_matter() {
        let (_dir, cmd) = shell(
            r#"[ "$1 $2 $3" = "-l -i -c" ] || exit 1
echo "welcome from the profile"
PATH=/from/profile:$PATH; export PATH
sleep 5 &
sh -c "$4"
echo "goodbye"
"#,
        );
        let started = Instant::now();
        let path = ask(cmd, Duration::from_secs(4)).expect("a PATH");
        assert!(started.elapsed() < Duration::from_secs(4), "not held up");
        let path = path.into_string().unwrap();
        assert!(path.starts_with("/from/profile:"), "{path}");
        assert!(!path.contains('\n'), "{path:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_shell_that_hangs_or_fails_gives_none() {
        let (_dir, hangs) = shell("sleep 10\n");
        let started = Instant::now();
        assert_eq!(ask(hangs, Duration::from_millis(300)), None);
        assert!(started.elapsed() < Duration::from_secs(5));

        let (_dir, fails) = shell("exit 1\n");
        assert_eq!(ask(fails, TIMEOUT), None);
        assert_eq!(ask(Command::new("/no/such/shell"), TIMEOUT), None);
    }

    #[test]
    fn started_from_a_terminal_nothing_changes() {
        let mut cmd = Command::new("git");
        apply(&mut cmd);
        assert_eq!(cmd.get_envs().count(), 0);
    }
}
