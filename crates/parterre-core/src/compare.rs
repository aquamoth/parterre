use crate::changed_files::{ChangedFile, FileStatus, compare_paths};
use crate::conflicts::{Conflict, Sides};
use crate::file_diff::Rev;
use crate::git::{Git, GitError};
use crate::log::{ancestor_first, is_ancestor};
use crate::oid::Oid;
use crate::repo::{CommitIx, Repo};

/// `old` goes on the left.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Comparison {
    pub old: Rev,
    pub new: Rev,
    /// Compare `new` with the merge base, as `git diff --merge-base` does, what a pull request
    /// shows. The working tree counts as `HEAD`.
    pub since_ancestor: bool,
}

impl Comparison {
    /// In the order picked, except that an ancestor goes on the left.
    pub fn of(repo: &Repo, first: CommitIx, second: CommitIx, since_ancestor: bool) -> Comparison {
        let (old, new) = ancestor_first(repo, first, second);
        Comparison {
            old: Rev::Commit(repo.commit(old).oid),
            new: Rev::Commit(repo.commit(new).oid),
            since_ancestor,
        }
    }

    pub fn with_working_tree(commit: Oid, since_ancestor: bool) -> Comparison {
        Comparison {
            old: Rev::Commit(commit),
            new: Rev::WorkingTree,
            since_ancestor,
        }
    }

    pub fn swapped(self) -> Comparison {
        Comparison {
            old: self.new,
            new: self.old,
            ..self
        }
    }

    /// Whether [`Comparison::since_ancestor`] changes anything: not when `old` is an ancestor
    /// of `new`, and so their merge base. The working tree counts as `HEAD`; a commit missing
    /// from `repo` as forked.
    pub fn ancestor_matters(&self, repo: &Repo) -> bool {
        let ix = |rev: Rev| match rev {
            Rev::Commit(oid) => repo.lookup(&oid),
            Rev::WorkingTree => repo.head_commit(),
        };
        match (ix(self.old), ix(self.new)) {
            (Some(old), Some(new)) => !is_ancestor(repo, old, new),
            _ => true,
        }
    }

    pub fn reads_working_tree(&self) -> bool {
        self.old == Rev::WorkingTree || self.new == Rev::WorkingTree
    }

    pub fn run(&self, git: &Git) -> Result<Compared, GitError> {
        let base = if self.since_ancestor {
            let name = |rev: Rev| rev.commit().map_or("HEAD".to_owned(), |o| o.to_hex());
            git.merge_base(&name(self.old), &name(self.new))?
                .map(Rev::Commit)
        } else {
            Some(self.old)
        };
        let mut files = match (base, self.new) {
            (None, _) | (Some(Rev::WorkingTree), Rev::WorkingTree) => Vec::new(),
            (Some(Rev::Commit(a)), Rev::Commit(b)) => git.changed_between(&a, &b)?,
            (Some(Rev::Commit(a)), Rev::WorkingTree) => git.changed_in_working_tree(&a, false)?,
            (Some(Rev::WorkingTree), Rev::Commit(b)) => git.changed_in_working_tree(&b, true)?,
        };
        let working_tree = if self.reads_working_tree() {
            let (root, conflicts) = crate::conflicts::list(git)?;
            add_conflicted(&mut files, &conflicts);
            let sides = crate::conflicts::sides(git, &root, &conflicts);
            Some(WorkingTree {
                root,
                conflicts,
                sides,
            })
        } else {
            None
        };
        Ok(Compared {
            base,
            files,
            working_tree,
        })
    }
}

/// Adds the conflicted files `git diff` left out for equalling the commit's: the same on both
/// sides, as `HEAD` has them.
fn add_conflicted(files: &mut Vec<ChangedFile>, conflicts: &[Conflict]) {
    let listed = files.len();
    for c in conflicts {
        if files.iter().any(|f| f.path == c.path) {
            continue;
        }
        let ours = c.stages[1].map_or(0, |e| e.mode);
        let lines = (!c.binary && !c.is_submodule()).then_some(0);
        files.push(ChangedFile {
            path: c.path.clone(),
            old_path: None,
            status: FileStatus::Unmerged,
            modes: [ours; 2],
            added: lines,
            removed: lines,
        });
    }
    if files.len() > listed {
        files.sort_by(|a, b| compare_paths(&a.path, &b.path));
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkingTree {
    /// The files' paths are relative to it.
    pub root: std::path::PathBuf,
    pub conflicts: Vec<Conflict>,
    pub sides: Sides,
}

impl WorkingTree {
    pub fn conflict(&self, path: &str) -> Option<&Conflict> {
        self.conflicts.iter().find(|c| c.path == path)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compared {
    /// Where the file diffs' left side is read from: `old`, or the merge base. `None`, with no
    /// files, for the merge base of unrelated histories.
    pub base: Option<Rev>,
    pub files: Vec<ChangedFile>,
    pub working_tree: Option<WorkingTree>,
}
