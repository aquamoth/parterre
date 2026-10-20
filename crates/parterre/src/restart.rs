//! An upgrade by the Windows installer closes a running parterre through the Restart Manager and
//! offers to start it again when done, but starts again only programs that asked it to. This
//! asks, with the repository shown, so that the graph comes back where it was. Only after an
//! update: not after a crash or a hang, nor at sign-in after Windows restarts.

use std::ffi::OsString;
use std::path::Path;

/// Asks to be started again on `repo` after an update, or as `parterre` alone without one.
/// Called again when the shown repository changes; the last call counts.
pub fn register(repo: Option<&Path>) {
    #[cfg(windows)]
    win32::register(&arguments(repo));
    #[cfg(not(windows))]
    let _ = repo;
}

/// The command line after the program's name, which Windows puts in front: the repository,
/// quoted. A backslash before the closing quote would escape it, so a trailing one (`C:\`) is
/// doubled. Windows paths hold no quotes.
#[cfg_attr(not(windows), allow(dead_code))]
fn arguments(repo: Option<&Path>) -> OsString {
    let mut line = OsString::new();
    if let Some(repo) = repo {
        line.push("\"");
        line.push(repo);
        if repo.as_os_str().as_encoded_bytes().ends_with(b"\\") {
            line.push("\\");
        }
        line.push("\"");
    }
    line
}

/// Declared here as `console` declares its kernel32 functions.
#[cfg(windows)]
#[allow(unsafe_code)]
mod win32 {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt as _;

    const RESTART_NO_CRASH: u32 = 1;
    const RESTART_NO_HANG: u32 = 2;
    const RESTART_NO_REBOOT: u32 = 8;
    /// `RESTART_MAX_CMD_LINE`, in characters with the terminating null.
    const MAX_LINE: usize = 1024;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn RegisterApplicationRestart(command_line: *const u16, flags: u32) -> i32;
    }

    pub fn register(arguments: &OsStr) {
        let mut line: Vec<u16> = arguments.encode_wide().collect();
        // Too long a path is left out: parterre then starts without one.
        if line.len() >= MAX_LINE {
            line.clear();
        }
        line.push(0);
        // SAFETY: `line` is a null-terminated UTF-16 string that outlives the call, which copies
        // it. A failure leaves parterre closed after an update, as before.
        unsafe {
            RegisterApplicationRestart(
                line.as_ptr(),
                RESTART_NO_CRASH | RESTART_NO_HANG | RESTART_NO_REBOOT,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::arguments;

    #[test]
    fn without_a_repository_parterre_starts_alone() {
        assert_eq!(arguments(None), "");
    }

    #[test]
    fn the_repository_is_quoted() {
        assert_eq!(
            arguments(Some(Path::new(r"C:\src\my repo"))),
            r#""C:\src\my repo""#
        );
    }

    #[test]
    fn a_trailing_backslash_does_not_escape_the_closing_quote() {
        assert_eq!(arguments(Some(Path::new(r"D:\"))), r#""D:\\""#);
    }
}
