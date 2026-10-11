use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use crate::browser::spawn_detached;

/// Opens the folder `dir` in the file manager. A missing folder is an error, as Explorer would
/// open Documents instead. Whether the opener then succeeds isn't known: none reports it
/// reliably.
pub fn open(dir: &Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("{} is gone", dir.display()));
    }
    spawn_detached(opener(dir)).map_err(|e| format!("could not open {}: {e}", dir.display()))
}

/// Opens the first of the platform's terminals that starts, in the folder `dir`.
pub fn open_terminal(dir: &Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("{} is gone", dir.display()));
    }
    let mut not_found = None;
    for Terminal {
        mut command,
        own_console,
    } in terminals(dir)
    {
        command.current_dir(dir);
        let started = if own_console {
            command.spawn().map(|mut child| {
                std::thread::spawn(move || child.wait());
            })
        } else {
            spawn_detached(command)
        };
        match started {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => not_found = Some(e),
            Err(e) => return Err(format!("could not open a terminal: {e}")),
        }
    }
    Err(match not_found {
        Some(e) => format!("could not open a terminal: none was found ({e})"),
        None => "could not open a terminal: none was found".to_owned(),
    })
}

struct Terminal {
    command: Command,
    /// A shell in a console of its own, which needs that console's input and output.
    own_console: bool,
}

impl Terminal {
    fn detached(command: Command) -> Terminal {
        Terminal {
            command,
            own_console: false,
        }
    }
}

#[cfg(target_os = "macos")]
fn opener(dir: &Path) -> Command {
    let mut cmd = Command::new("open");
    cmd.arg(dir);
    cmd
}

#[cfg(target_os = "macos")]
fn terminals(dir: &Path) -> Vec<Terminal> {
    let mut cmd = Command::new("open");
    cmd.args(["-a", "Terminal"]).arg(dir);
    vec![Terminal::detached(cmd)]
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

#[cfg(any(windows, test))]
fn explorer_arg(dir: &Path) -> String {
    format!("\"{}\"", dir.to_string_lossy().replace('/', "\\"))
}

/// Windows Terminal is told to start where it is started (`-d .`), so that no path has to get
/// through its command line, which splits at `;`.
#[cfg(windows)]
fn terminals(_dir: &Path) -> Vec<Terminal> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let mut wt = Command::new("wt.exe");
    wt.args(["-d", "."]);
    let shell = |program: &str| {
        let mut command = Command::new(program);
        command.creation_flags(CREATE_NEW_CONSOLE);
        Terminal {
            command,
            own_console: true,
        }
    };
    vec![
        Terminal::detached(wt),
        shell("pwsh.exe"),
        shell("powershell.exe"),
    ]
}

#[cfg(not(any(windows, target_os = "macos")))]
fn opener(dir: &Path) -> Command {
    // xdg-open takes anything starting with `-` for an option; an absolute path never does.
    let mut cmd = Command::new("xdg-open");
    cmd.arg(std::path::absolute(dir).unwrap_or_else(|_| dir.to_owned()));
    cmd
}

#[cfg(not(any(windows, target_os = "macos")))]
fn terminals(_dir: &Path) -> Vec<Terminal> {
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
        .map(|program| Terminal::detached(Command::new(program)))
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
