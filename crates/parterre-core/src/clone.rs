//! Cloning a repository into a new folder (#358): `git clone` from any URL git takes, GitHub,
//! Azure DevOps or another host alike, with credentials left to git's credential helpers as
//! for fetching. The folder is named as git names it, and a clone that fails or is cancelled
//! leaves none behind.

use std::path::{Path, PathBuf};

use parterre_util::CancelTree;

use crate::branches::{Error, Report};
use crate::git::Git;
use crate::remote::Live;
use crate::worktree_folder;

/// A clone of `url` into `parent`/`name`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cloning {
    pub url: String,
    pub parent: PathBuf,
    pub name: String,
}

impl Cloning {
    /// The folder the clone goes in.
    pub fn path(&self) -> PathBuf {
        self.parent.join(&self.name)
    }

    /// Why it can't start, if it can't: said under the dialog's fields.
    pub fn error(&self) -> Option<String> {
        if !is_url(&self.url) {
            return Some("Enter the repository's URL.".into());
        }
        if !self.parent.is_absolute() || !self.parent.is_dir() {
            return Some("Choose an existing parent folder.".into());
        }
        let name = self.name.trim();
        if name.is_empty() || name == "." || name == ".." {
            return Some("Enter a folder name.".into());
        }
        if self.path().exists() {
            return Some(format!("{} already exists.", self.path().display()));
        }
        None
    }
}

/// `git clone`, into the full path, which may start with `-`.
pub fn command(cloning: &Cloning) -> Vec<String> {
    let path = cloning.path().to_string_lossy().into_owned();
    ["clone", "--progress", "--", &cloning.url, &path]
        .map(str::to_owned)
        .to_vec()
}

/// Whether `text` is something to clone rather than words to look for: a URL with a scheme,
/// git's `host:path`, or a local path. `owner/name` and plain words are not.
pub fn is_url(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    if text.contains("://") || text.starts_with(['/', '\\', '~']) || text.starts_with('.') {
        return true;
    }
    // `C:\…` or `C:/…`, a Windows path.
    let bytes = text.as_bytes();
    if bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return matches!(bytes[2], b'\\' | b'/');
    }
    // git's scp-like syntax: a colon before any slash, after a host.
    match text.split_once(':') {
        Some((host, path)) => {
            !host.is_empty()
                && !host.contains(|c: char| c == '/' || c.is_whitespace())
                && !path.is_empty()
        }
        None => false,
    }
}

/// The folder name git gives a clone of `url`: its last part, without `.git` (`parterre` from
/// `git@github.com:aquamoth/parterre.git`, `repo` from Azure DevOps' `…/_git/repo`), with
/// what Windows forbids in a name made `-`. Empty when the URL has no name in it.
pub fn folder_name(url: &str) -> String {
    let mut rest = url.trim().trim_end_matches(['/', '\\']);
    if let Some(stripped) = rest.strip_suffix(".git")
        && stripped.ends_with(['/', '\\'])
    {
        rest = stripped.trim_end_matches(['/', '\\']);
    }
    let start = rest.rfind(['/', '\\', ':']).map_or(0, |i| i + 1);
    let last = &rest[start..];
    let name = last
        .strip_suffix(".git")
        .or_else(|| last.strip_suffix(".bundle"))
        .unwrap_or(last);
    worktree_folder::folder_name(name)
}

/// Clones, streaming git's output to `live`. A clone that doesn't finish leaves no folder.
pub fn execute(
    cloning: &Cloning,
    cancel: &CancelTree,
    report: &mut Report,
    live: Option<&Live>,
) -> Result<(), Error> {
    if let Some(error) = cloning.error() {
        return Err(Error::Invalid(error));
    }
    let path = cloning.path();
    let result = crate::remote::run_live(
        &Git::new(&cloning.parent),
        command(cloning),
        cancel,
        report,
        live,
    );
    if !matches!(result, Ok(true)) {
        remove_unfinished(&path);
    }
    match result? {
        true => Ok(()),
        false => Err(crate::remote::network_failure(report)),
    }
}

/// git removes a folder it couldn't clone into, but not when it's killed by *Cancel*.
fn remove_unfinished(path: &Path) {
    if path.exists() {
        let _ = std::fs::remove_dir_all(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_are_gits() {
        let cases = [
            ("https://github.com/aquamoth/parterre.git", "parterre"),
            ("https://github.com/aquamoth/parterre", "parterre"),
            ("https://github.com/aquamoth/parterre/", "parterre"),
            ("git@github.com:aquamoth/parterre.git", "parterre"),
            ("git@host:repo.git", "repo"),
            (
                "ssh://git@ssh.github.com:443/aquamoth/parterre.git",
                "parterre",
            ),
            ("https://dev.azure.com/org/project/_git/repo", "repo"),
            (
                "https://org@dev.azure.com/org/My%20Project/_git/my.repo",
                "my.repo",
            ),
            ("/srv/git/project.git", "project"),
            ("/srv/git/project/.git", "project"),
            ("C:\\repos\\thing", "thing"),
            ("../bundle.bundle", "bundle"),
            ("", ""),
        ];
        for (url, name) in cases {
            assert_eq!(folder_name(url), name, "{url}");
        }
    }

    #[test]
    fn urls_and_paths_are_told_from_words() {
        for url in [
            "https://github.com/aquamoth/parterre.git",
            "git@github.com:aquamoth/parterre.git",
            "github.com:aquamoth/parterre",
            "ssh://host/repo",
            "file:///srv/repo",
            "/srv/repo",
            "~/repo",
            "./repo",
            "../repo",
            "C:\\repos\\thing",
            "C:/repos/thing",
            "C:\\My Repos\\thing",
            "\\\\server\\share\\repo",
        ] {
            assert!(is_url(url), "{url}");
        }
        for words in [
            "",
            "parterre",
            "aquamoth/parterre",
            "aqua moth",
            "a note: here",
            "C:",
            "host:",
            ":path",
        ] {
            assert!(!is_url(words), "{words}");
        }
    }

    #[test]
    fn the_command_ends_options_before_the_url_and_takes_the_full_path() {
        let cloning = Cloning {
            url: "-u".into(),
            parent: PathBuf::from("/tmp"),
            name: "x".into(),
        };
        let path = PathBuf::from("/tmp").join("x");
        assert_eq!(
            command(&cloning),
            ["clone", "--progress", "--", "-u", &path.to_string_lossy()]
        );
    }
}
