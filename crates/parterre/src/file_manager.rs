//! Opening a folder in the platform's file manager, by running its opener with the path as its
//! argument (no shell). Research: `docs/research/git-worktrees.md`, §4.

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

#[cfg(target_os = "macos")]
fn opener(dir: &Path) -> Command {
    let mut cmd = Command::new("open");
    cmd.arg(dir);
    cmd
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

#[cfg(not(any(windows, target_os = "macos")))]
fn opener(dir: &Path) -> Command {
    // xdg-open takes anything starting with `-` for an option; a folder given as an absolute
    // path never does.
    let mut cmd = Command::new("xdg-open");
    cmd.arg(std::path::absolute(dir).unwrap_or_else(|_| dir.to_owned()));
    cmd
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
    }
}
