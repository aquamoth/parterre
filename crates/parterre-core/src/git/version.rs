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
    static TOO_OLD: OnceLock<Option<String>> = OnceLock::new();
    let too_old = match TOO_OLD.get() {
        Some(too_old) => too_old,
        None => {
            let out = super::git_command()
                .arg("--version")
                .output()
                .map_err(GitError::Spawn)?;
            let stdout = String::from_utf8_lossy(&out.stdout);
            TOO_OLD.get_or_init(|| too_old(&stdout))
        }
    };
    match too_old {
        Some(found) => Err(GitError::TooOld(found.clone())),
        None => Ok(()),
    }
}

/// The version `git --version` printed, if it is older than [`MINIMUM_VERSION`]. Output it
/// can't read passes: better to try than to refuse a git that may well work.
fn too_old(out: &str) -> Option<String> {
    let found = out.trim().strip_prefix("git version ")?;
    let mut parts = found.split('.').map(str::parse::<u32>);
    let (Some(Ok(major)), Some(Ok(minor))) = (parts.next(), parts.next()) else {
        return None;
    };
    ((major, minor) < MINIMUM_VERSION).then(|| found.to_owned())
}

#[cfg(test)]
mod tests {
    use super::too_old;

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
