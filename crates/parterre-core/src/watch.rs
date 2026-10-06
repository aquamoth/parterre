//! Noticing that a repository's refs have changed, so the graph can reload by itself.
//!
//! Rather than asking git, this looks at the files git keeps refs in: `HEAD`, `packed-refs`,
//! the loose refs under `refs/` and a reftable's tables. git replaces such a file whenever it
//! moves a ref, which changes its modification time and that of its directory. Looking costs
//! a few hundred `stat` calls, cheap enough to repeat every second, and needs neither a git
//! process nor a file-watching library.
//!
//! The other worktrees count too: their HEADs, their being added, removed, moved or locked,
//! and their folders going missing (`docs/research/git-worktrees.md`, §3).
//!
//! A changed fingerprint means "maybe": `git pack-refs` or `git gc` rewrite the files without
//! moving any ref. Compare the reloaded snapshot with [`Repo::same_refs`](crate::Repo::same_refs).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::git::{Git, GitError, Location};

/// Where one repository keeps its refs.
#[derive(Clone, Debug)]
pub struct RefStorage {
    /// Files, and directories to walk, whose metadata make up the fingerprint.
    paths: Vec<PathBuf>,
    /// `<common>/worktrees`, whose entries are listed afresh each time.
    worktrees: PathBuf,
}

impl RefStorage {
    /// Finds the ref storage of the repository containing `dir`.
    pub fn locate(dir: &Path) -> Result<RefStorage, GitError> {
        let Location {
            git_dir,
            common_dir: common,
            ..
        } = Git::new(dir).location()?;
        let mut paths = vec![
            git_dir.join("HEAD"),
            // The main worktree's HEAD, when this is a linked one.
            common.join("HEAD"),
            common.join("packed-refs"),
            common.join("refs"),
            common.join("reftable"),
        ];
        // A linked worktree has refs of its own (`refs/bisect`, `refs/worktree`).
        if git_dir != common {
            paths.push(git_dir.join("refs"));
            paths.push(git_dir.join("reftable"));
        }
        Ok(RefStorage {
            paths,
            worktrees: common.join("worktrees"),
        })
    }

    /// A hash of the size and modification time of every file refs are kept in. It changes
    /// when a ref is created, moved or deleted.
    pub fn fingerprint(&self) -> u64 {
        let mut entries = Vec::new();
        for path in &self.paths {
            collect(path, &mut entries);
        }
        collect_worktrees(&self.worktrees, &mut entries);
        entries.sort_unstable();
        let mut hasher = DefaultHasher::new();
        entries.hash(&mut hasher);
        hasher.finish()
    }
}

/// One file or directory: its path, size and modification time in nanoseconds.
type Entry = (PathBuf, u64, u128);

/// Adds the linked worktrees: `worktrees` itself (added and removed ones), and for each its
/// `HEAD`, `gitdir` (moved), `locked` and reftable, and its folder's `.git` (gone). Not the
/// entries' own directories, nor the rest of them, which every `git status` there touches.
fn collect_worktrees(worktrees: &Path, out: &mut Vec<Entry>) {
    let Some(entry) = stat(worktrees) else {
        return;
    };
    out.push(entry);
    let Ok(dir) = std::fs::read_dir(worktrees) else {
        return;
    };
    for id in dir.flatten() {
        let id = id.path();
        for name in ["HEAD", "gitdir", "locked", "reftable"] {
            collect(&id.join(name), out);
        }
        // The `.git` file in the worktree's folder; relative since git 2.48 if asked for.
        if let Ok(gitdir) = std::fs::read_to_string(id.join("gitdir")) {
            out.extend(stat(&id.join(gitdir.trim_end())));
        }
    }
}

/// `path` with its size and modification time, if it is there.
fn stat(path: &Path) -> Option<Entry> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    Some((path.to_owned(), meta.len(), modified))
}

/// Adds `path` and, for a directory, everything below it. Missing paths add nothing.
fn collect(path: &Path, out: &mut Vec<Entry>) {
    let Some(entry) = stat(path) else {
        return;
    };
    out.push(entry);
    if path.is_dir()
        && let Ok(dir) = std::fs::read_dir(path)
    {
        for entry in dir.flatten() {
            collect(&entry.path(), out);
        }
    }
}
