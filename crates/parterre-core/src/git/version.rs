//! The oldest git parterre works with, checked once before the first repository is opened.
//!
//! An older git doesn't fail with an error: git 2.30 prints `--path-format=absolute` back as
//! output and exits 0, and parterre would take that for a path
//! (`docs/research/git-version-support.md`, #233).

use std::sync::OnceLock;

use super::GitError;

/// The oldest git parterre works with, as major and minor version. Three features without a
/// fallback set it, all from git 2.31: `diff-tree --diff-merges=first-parent`,
/// `rev-parse --path-format=absolute` and the `locked`/`prunable` lines of
/// `worktree list --porcelain`. Every installer declares it too, and
/// `crates/parterre/tests/packaging.rs` fails until they all agree.
pub const MINIMUM_VERSION: (u32, u32) = (2, 31);

/// Fails with [`GitError::TooOld`] if the git parterre runs is older than [`MINIMUM_VERSION`].
/// git runs once; a failure to start it isn't remembered, so installing git fixes it.
pub(super) fn check() -> Result<(), GitError> {
    match too_old(printed().map_err(GitError::Spawn)?) {
        Some(found) => Err(GitError::TooOld(found)),
        None => Ok(()),
    }
}

/// The version of the git parterre runs, as `git --version` prints it (`2.43.0`,
/// `2.47.1.windows.2`), for the usage statistics. `None` if git can't be started or its answer
/// can't be read.
pub fn version() -> Option<String> {
    printed().ok().and_then(read).map(str::to_owned)
}

/// What `git --version` printed, asked once.
fn printed() -> Result<&'static str, std::io::Error> {
    static PRINTED: OnceLock<String> = OnceLock::new();
    if let Some(printed) = PRINTED.get() {
        return Ok(printed);
    }
    let out = super::git_command().arg("--version").output()?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    Ok(PRINTED.get_or_init(|| stdout))
}

/// The version in what `git --version` printed.
fn read(out: &str) -> Option<&str> {
    let found = out.trim().strip_prefix("git version ")?;
    (!found.is_empty()).then_some(found)
}

/// The version `git --version` printed, if it is older than [`MINIMUM_VERSION`]. Output it
/// can't read passes: better to try than to refuse a git that may well work.
fn too_old(out: &str) -> Option<String> {
    let found = read(out)?;
    let mut parts = found.split('.').map(str::parse::<u32>);
    let (Some(Ok(major)), Some(Ok(minor))) = (parts.next(), parts.next()) else {
        return None;
    };
    ((major, minor) < MINIMUM_VERSION).then(|| found.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{read, too_old};

    #[test]
    fn reads_the_version_git_prints() {
        assert_eq!(read("git version 2.43.0\n"), Some("2.43.0"));
        assert_eq!(
            read("git version 2.47.1.windows.2\n"),
            Some("2.47.1.windows.2")
        );
        assert_eq!(read("hub version 2.14.2"), None);
        assert_eq!(read("git version "), None);
        assert_eq!(read(""), None);
    }

    #[test]
    fn older_than_the_minimum_is_too_old() {
        assert_eq!(too_old("git version 2.30.2\n"), Some("2.30.2".into()));
        assert_eq!(too_old("git version 1.9.5"), Some("1.9.5".into()));
    }

    #[test]
    fn the_minimum_and_newer_pass() {
        assert_eq!(too_old("git version 2.31.0\n"), None);
        assert_eq!(too_old("git version 2.43.0\n"), None);
        assert_eq!(too_old("git version 3.0.0\n"), None);
    }

    #[test]
    fn vendor_suffixes_are_read_past() {
        assert_eq!(too_old("git version 2.47.1.windows.2\n"), None);
        assert_eq!(too_old("git version 2.39.5 (Apple Git-154)\n"), None);
        // A build of git's `next` without its .git directory.
        assert_eq!(too_old("git version 2.56.GIT\n"), None);
        assert_eq!(
            too_old("git version 2.30.9.windows.1\n"),
            Some("2.30.9.windows.1".into())
        );
    }

    #[test]
    fn output_it_cannot_read_passes() {
        assert_eq!(too_old(""), None);
        assert_eq!(too_old("hub version 2.14.2"), None);
        assert_eq!(too_old("git version two"), None);
    }
}
