//! Opening a folder in the platform's file manager, or a terminal in it, by running an opener
//! with the path as its argument or as its working directory (no shell). Research:
//! `docs/research/git-worktrees.md`, §4.

use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

/// Opens the folder `dir` in the file manager. A folder that isn't there is an error, as
/// Explorer would open Documents instead, and so is an opener that can't be started. What
/// the opener does after that isn't known: none of them reports it reliably.
pub fn open(dir: &Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("{} is gone", dir.display()));
    }
    crate::browser::spawn(opener(dir)).map_err(|e| format!("could not open {}: {e}", dir.display()))
}

/// Opens a terminal in the folder `dir`: the first of the platform's terminals that starts.
pub fn open_terminal(dir: &Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("{} is gone", dir.display()));
    }
    let mut last = None;
    for (mut cmd, quiet) in terminals(dir) {
        cmd.current_dir(dir);
        let started = if quiet {
            crate::browser::spawn(cmd)
        } else {
            // A shell in a console of its own needs that console's input and output.
            cmd.spawn().map(|mut child| {
                std::thread::spawn(move || child.wait());
            })
        };
        match started {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => last = Some(e),
            Err(e) => return Err(format!("could not open a terminal: {e}")),
        }
    }
    Err(match last {
        Some(e) => format!("could not open a terminal: none was found ({e})"),
        None => "could not open a terminal: none was found".to_owned(),
    })
}

#[cfg(target_os = "macos")]
fn opener(dir: &Path) -> Command {
    let mut cmd = Command::new("open");
    cmd.arg(dir);
    cmd
}

/// The terminals to try, in order, each started in the folder; `true` where its input and
/// output can go to nothing.
#[cfg(target_os = "macos")]
fn terminals(dir: &Path) -> Vec<(Command, bool)> {
    let mut cmd = Command::new("open");
    cmd.args(["-a", "Terminal"]).arg(dir);
    vec![(cmd, true)]
}

#[cfg(windows)]
fn opener(dir: &Path) -> Command {
    use std::os::windows::process::CommandExt;
    // Explorer opens Documents for a path with forward slashes (as git writes them), and for
    // one with a comma unless it is quoted, which Rust does only for spaces. A Windows path
    // can't hold a `"`, so quoting it by hand is safe.
    let mut cmd = Command::new("explorer.exe");
    cmd.raw_arg(explorer_arg(dir));
    cmd
}

/// `dir` for Explorer's command line: backslashes, in quotes.
#[cfg(any(windows, test))]
fn explorer_arg(dir: &Path) -> String {
    format!("\"{}\"", dir.to_string_lossy().replace('/', "\\"))
}

/// Windows Terminal, told to start where it is started (`-d .`, so that no path has to get
/// through its command line, which splits at `;`); else PowerShell in a console of its own.
#[cfg(windows)]
fn terminals(_dir: &Path) -> Vec<(Command, bool)> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let mut wt = Command::new("wt.exe");
    wt.args(["-d", "."]);
    let shell = |program: &str| {
        let mut cmd = Command::new(program);
        cmd.creation_flags(CREATE_NEW_CONSOLE);
        (cmd, false)
    };
    vec![(wt, true), shell("pwsh.exe"), shell("powershell.exe")]
}

#[cfg(not(any(windows, target_os = "macos")))]
fn opener(dir: &Path) -> Command {
    // xdg-open takes anything starting with `-` for an option; a folder given as an absolute
    // path never does.
    let mut cmd = Command::new("xdg-open");
    cmd.arg(std::path::absolute(dir).unwrap_or_else(|_| dir.to_owned()));
    cmd
}

/// `$TERMINAL`, then the common terminals; each opens where it is started.
#[cfg(not(any(windows, target_os = "macos")))]
fn terminals(_dir: &Path) -> Vec<(Command, bool)> {
    let named = std::env::var("TERMINAL").ok().filter(|t| !t.is_empty());
    named
        .into_iter()
        .chain(
            [
                "x-terminal-emulator",
                "gnome-terminal",
                "konsole",
                "xfce4-terminal",
                "kitty",
                "alacritty",
                "xterm",
            ]
            .map(str::to_owned),
        )
        .map(|program| (Command::new(program), true))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_gets_backslashes_in_quotes() {
        assert_eq!(
            explorer_arg(Path::new("C:/src/a,b/wt space")),
            r#""C:\src\a,b\wt space""#
        );
    }

    #[test]
    fn a_folder_that_is_gone_is_not_opened() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gone = dir.path().join("gone");
        assert!(open(&gone).unwrap_err().ends_with("is gone"));
        assert!(open_terminal(&gone).unwrap_err().ends_with("is gone"));
    }
}
