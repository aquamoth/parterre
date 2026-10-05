//! Conflicted files: what git's index holds for each, what kind of conflict that is, and what
//! can finish it. Content conflicts go to the user's merge tool; the kinds a merge tool can't
//! open (one side deleted the file, binary files, symlinks) are answered with one git command.
//! Submodules are left to the terminal.
//!
//! The index decides: a file is conflicted while git holds it unmerged (stages 1 to 3), and
//! resolved once staged, whatever its text.

use std::path::{Path, PathBuf};

use parterre_util::CancelTree;

use crate::branches::{Catalog, Error, Report};
use crate::git::{Git, GitError};
use crate::oid::Oid;

/// git's mode of a symlink.
const SYMLINK: u32 = 0o120000;
/// git's mode of a submodule.
const GITLINK: u32 = 0o160000;
/// How much of a file git reads to decide it's binary (`buffer_is_binary`).
const FIRST_FEW_BYTES: usize = 8000;

/// One side of a conflicted file, as an index stage holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub mode: u32,
    pub oid: Oid,
}

/// A conflicted file: its stages and what git's content says about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    /// Relative to the worktree's root, with `/` separators.
    pub path: String,
    /// The common ancestor's version (stage 1), ours (stage 2, `HEAD`) and theirs (stage 3).
    pub stages: [Option<Entry>; 3],
    /// A side is binary, by git's test: a NUL in its first 8000 bytes.
    pub binary: bool,
    /// The file is in the worktree.
    pub on_disk: bool,
}

/// What finishes a conflicted file in parterre, in one git command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Answer {
    /// Stage the file as it is in the worktree (`git add`).
    Keep,
    /// Remove it (`git rm`).
    Delete,
    /// Take our side (stage 2).
    Ours,
    /// Take their side (stage 3).
    Theirs,
}

impl Conflict {
    /// `git status --short`'s code: `UU`, `AA`, `UD`, `DU`, `AU`, `UA` or `DD`.
    pub fn code(&self) -> &'static str {
        match self.stages.map(|s| s.is_some()) {
            [true, true, true] => "UU",
            [false, true, true] => "AA",
            [true, true, false] => "UD",
            [true, false, true] => "DU",
            [false, true, false] => "AU",
            [false, false, true] => "UA",
            _ => "DD",
        }
    }

    /// `git status`'s words for [`Conflict::code`].
    pub fn words(&self) -> &'static str {
        match self.code() {
            "UU" => "both modified",
            "AA" => "both added",
            "UD" => "deleted by them",
            "DU" => "deleted by us",
            "AU" => "added by us",
            "UA" => "added by them",
            _ => "both deleted",
        }
    }

    fn has_mode(&self, mode: u32) -> bool {
        self.stages.iter().flatten().any(|e| e.mode == mode)
    }

    pub fn is_submodule(&self) -> bool {
        self.has_mode(GITLINK)
    }

    /// Why the merge tool can't open it, if it can't. `git mergetool` only merges content:
    /// for the rest it asks on the terminal (one side deleted, symlinks, submodules), and a
    /// merge tool can't merge binary files.
    pub fn merge_tool(&self) -> Result<(), &'static str> {
        if self.is_submodule() {
            Err("A submodule: resolve it in a terminal")
        } else if self.has_mode(SYMLINK) {
            Err("A symlink: no merge tool")
        } else if self.stages[1].is_none() || self.stages[2].is_none() {
            Err(match self.code() {
                "UD" => "Deleted by them: no merge tool",
                "DU" => "Deleted by us: no merge tool",
                "AU" => "Added by us only: no merge tool",
                "UA" => "Added by them only: no merge tool",
                _ => "Deleted by both: no merge tool",
            })
        } else if self.binary {
            Err("A binary file: no merge tool")
        } else {
            Ok(())
        }
    }

    /// What finishes it in parterre: keep or delete when a side has no file; a side when both
    /// have one the merge tool can't open. Nothing for content conflicts (the merge tool's)
    /// and submodules (the terminal's).
    pub fn answers(&self) -> Vec<Answer> {
        if self.is_submodule() {
            return Vec::new();
        }
        if self.stages[1].is_none() || self.stages[2].is_none() {
            let mut answers = Vec::new();
            if self.on_disk {
                answers.push(Answer::Keep);
            }
            answers.push(Answer::Delete);
            return answers;
        }
        if self.merge_tool().is_err() {
            return vec![Answer::Ours, Answer::Theirs];
        }
        Vec::new()
    }

    /// The git commands an answer runs, without the leading `git`.
    pub fn commands(&self, answer: Answer) -> Vec<Vec<String>> {
        let path = self.path.clone();
        let words = |w: &[&str]| -> Vec<String> {
            w.iter()
                .map(|s| s.to_string())
                .chain([path.clone()])
                .collect()
        };
        let side = match answer {
            Answer::Keep => return vec![words(&["add", "--"])],
            Answer::Delete => return vec![words(&["rm", "--quiet", "--"])],
            Answer::Ours => (1, "--ours"),
            Answer::Theirs => (2, "--theirs"),
        };
        match self.stages[side.0] {
            Some(_) => vec![words(&["checkout", side.1, "--"]), words(&["add", "--"])],
            // That side deleted it.
            None => vec![words(&["rm", "--quiet", "--"])],
        }
    }
}

/// Finishing one conflicted file in parterre, as it was conflicted when offered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolve {
    pub conflict: Conflict,
    pub answer: Answer,
    /// The side's label, as the menu named it; empty for keep and delete.
    pub side: String,
}

impl Resolve {
    /// `Keep a.txt`, `Delete a.txt`, `Use feature for a.txt`.
    pub fn label(&self) -> String {
        let path = &self.conflict.path;
        match self.answer {
            Answer::Keep => format!("Keep {path}"),
            Answer::Delete => format!("Delete {path}"),
            Answer::Ours | Answer::Theirs => format!("Use {} for {path}", self.side),
        }
    }
}

/// Runs `resolve`'s commands in the open worktree, if the file is still conflicted as it was.
pub(crate) fn execute(
    catalog: &Catalog,
    resolve: &Resolve,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<(), Error> {
    let git = Git::new(&catalog.root);
    let (_, now) = list(&git)?;
    let same = now
        .iter()
        .any(|c| c.path == resolve.conflict.path && c.stages == resolve.conflict.stages);
    if !same {
        return Err(Error::Invalid(format!(
            "{} is no longer conflicted as it was. Reload and try again.",
            resolve.conflict.path
        )));
    }
    if !resolve.conflict.answers().contains(&resolve.answer) {
        return Err(Error::Invalid(format!(
            "{} can't be finished that way.",
            resolve.conflict.path
        )));
    }
    for args in resolve.conflict.commands(resolve.answer) {
        if !crate::branches::run(&git, args, cancel, report)? {
            return Err(Error::Failed(
                report.steps.last().expect("ran").output.clone(),
            ));
        }
    }
    Ok(())
}

/// The two sides' names, as git writes them in the conflict markers: `HEAD` and `feature`
/// in a merge, `HEAD` and `abc1234 (subject)` in a rebase, `Updated upstream` and `Stashed
/// changes` in a stash pop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sides {
    pub ours: String,
    pub theirs: String,
}

impl Sides {
    /// The label of `answer`'s side, in full; `None` for keep and delete.
    pub fn label(&self, answer: Answer) -> Option<&str> {
        match answer {
            Answer::Ours => Some(&self.ours),
            Answer::Theirs => Some(&self.theirs),
            Answer::Keep | Answer::Delete => None,
        }
    }
}

/// A marker label's short form, for a menu item: `abc1234 (subject)` is `abc1234`.
pub fn short_label(label: &str) -> &str {
    label.split(" (").next().unwrap_or(label)
}

/// The open worktree's conflicted files, by path, and its root they are relative to.
pub fn list(git: &Git) -> Result<(PathBuf, Vec<Conflict>), GitError> {
    let root = git.repo_root()?;
    let git = Git::new(&root);
    let out = git.run(&["ls-files", "--unmerged", "-z"])?;
    let mut conflicts: Vec<Conflict> = Vec::new();
    for record in out.split('\0').filter(|r| !r.is_empty()) {
        // `<mode> <oid> <stage>\t<path>`
        let parsed = record.split_once('\t').and_then(|(meta, path)| {
            let mut fields = meta.split(' ');
            let mode = u32::from_str_radix(fields.next()?, 8).ok()?;
            let oid = Oid::from_hex(fields.next()?)?;
            let stage: usize = fields.next()?.parse().ok()?;
            (1..=3)
                .contains(&stage)
                .then_some((path, stage, Entry { mode, oid }))
        });
        let Some((path, stage, entry)) = parsed else {
            return Err(GitError::Parse(format!(
                "unexpected ls-files record {record:?}"
            )));
        };
        if conflicts.last().is_none_or(|c| c.path != path) {
            conflicts.push(Conflict {
                path: path.to_owned(),
                stages: [None; 3],
                binary: false,
                on_disk: false,
            });
        }
        conflicts.last_mut().expect("pushed").stages[stage - 1] = Some(entry);
    }
    for c in &mut conflicts {
        let file = root.join(&c.path);
        c.on_disk = file.symlink_metadata().is_ok();
        if c.stages[1].is_some() && c.stages[2].is_some() && !c.is_submodule() {
            c.binary = c.stages[1..]
                .iter()
                .flatten()
                .filter(|e| e.mode != SYMLINK)
                .map(|e| is_binary_blob(&git, &e.oid))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .any(|b| b);
        }
    }
    Ok((root, conflicts))
}

fn is_binary_blob(git: &Git, oid: &Oid) -> Result<bool, GitError> {
    let bytes = git.run_bytes(&["cat-file", "blob", &oid.to_hex()])?;
    Ok(bytes.iter().take(FIRST_FEW_BYTES).any(|&b| b == 0))
}

/// The sides' names: as the markers in a conflicted text file have them, which is exactly
/// what git wrote; else from the operation in progress; else git's `ours` and `theirs`.
pub fn sides(git: &Git, root: &Path, conflicts: &[Conflict]) -> Sides {
    conflicts
        .iter()
        .filter(|c| c.merge_tool().is_ok())
        .find_map(|c| {
            let text = std::fs::read(root.join(&c.path)).ok()?;
            from_markers(&String::from_utf8_lossy(&text))
        })
        .or_else(|| from_operation(git))
        .unwrap_or_else(|| Sides {
            ours: "ours".into(),
            theirs: "theirs".into(),
        })
}

/// The labels of the first conflict's markers in `text`.
pub fn from_markers(text: &str) -> Option<Sides> {
    let mut lines = text.lines();
    let ours = lines.find_map(|l| l.strip_prefix("<<<<<<< "))?.trim_end();
    let theirs = lines.find_map(|l| l.strip_prefix(">>>>>>> "))?.trim_end();
    Some(Sides {
        ours: ours.to_owned(),
        theirs: theirs.to_owned(),
    })
}

/// The labels git gives an operation's sides, for conflicts with no markers to read.
fn from_operation(git: &Git) -> Option<Sides> {
    let dir = PathBuf::from(git.query(&["rev-parse", "--absolute-git-dir"]).ok()??);
    let commit = |name: &str| -> Option<String> {
        let line = git
            .query(&["log", "-1", "--format=%h (%s)", name, "--"])
            .ok()??;
        (!line.is_empty()).then_some(line)
    };
    let theirs = if dir.join("MERGE_HEAD").is_file() {
        // `Merge branch 'feature'`: git labels their side by the name merged.
        std::fs::read_to_string(dir.join("MERGE_MSG"))
            .ok()
            .and_then(|m| Some(m.lines().next()?.split('\'').nth(1)?.to_owned()))
            .or_else(|| {
                git.query(&["rev-parse", "--short", "MERGE_HEAD"])
                    .ok()
                    .flatten()
            })?
    } else if let Some(c) = commit("REBASE_HEAD")
        .filter(|_| dir.join("rebase-merge").is_dir() || dir.join("rebase-apply").is_dir())
    {
        c
    } else if let Some(c) = commit("CHERRY_PICK_HEAD") {
        c
    } else {
        format!("parent of {}", commit("REVERT_HEAD")?)
    };
    Some(Sides {
        ours: "HEAD".into(),
        theirs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(mode: u32) -> Option<Entry> {
        Some(Entry {
            mode,
            oid: Oid::from_hex(&"1".repeat(40)).unwrap(),
        })
    }

    fn conflict(stages: [Option<Entry>; 3], binary: bool, on_disk: bool) -> Conflict {
        Conflict {
            path: "a b.txt".into(),
            stages,
            binary,
            on_disk,
        }
    }

    const FILE: u32 = 0o100644;

    #[test]
    fn codes_follow_the_stages_as_git_status_names_them() {
        let f = entry(FILE);
        let cases = [
            ([f, f, f], "UU", "both modified"),
            ([None, f, f], "AA", "both added"),
            ([f, f, None], "UD", "deleted by them"),
            ([f, None, f], "DU", "deleted by us"),
            ([None, f, None], "AU", "added by us"),
            ([None, None, f], "UA", "added by them"),
            ([f, None, None], "DD", "both deleted"),
        ];
        for (stages, code, words) in cases {
            let c = conflict(stages, false, true);
            assert_eq!((c.code(), c.words()), (code, words));
        }
    }

    #[test]
    fn only_text_on_both_sides_goes_to_the_merge_tool() {
        let f = entry(FILE);
        assert_eq!(conflict([f, f, f], false, true).merge_tool(), Ok(()));
        assert_eq!(conflict([None, f, f], false, true).merge_tool(), Ok(()));
        assert!(conflict([f, f, f], true, true).merge_tool().is_err());
        assert!(conflict([f, f, None], false, true).merge_tool().is_err());
        let link = entry(SYMLINK);
        assert!(
            conflict([link, link, link], false, true)
                .merge_tool()
                .is_err()
        );
        let sub = entry(GITLINK);
        assert!(conflict([sub, sub, sub], false, true).merge_tool().is_err());
    }

    #[test]
    fn answers_by_kind() {
        let f = entry(FILE);
        assert_eq!(conflict([f, f, f], false, true).answers(), vec![]);
        assert_eq!(
            conflict([f, f, f], true, true).answers(),
            vec![Answer::Ours, Answer::Theirs]
        );
        let link = entry(SYMLINK);
        assert_eq!(
            conflict([link, link, link], false, true).answers(),
            vec![Answer::Ours, Answer::Theirs]
        );
        assert_eq!(
            conflict([f, f, None], false, true).answers(),
            vec![Answer::Keep, Answer::Delete]
        );
        // Nothing on disk to keep.
        assert_eq!(
            conflict([f, None, None], false, false).answers(),
            vec![Answer::Delete]
        );
        let sub = entry(GITLINK);
        assert_eq!(conflict([sub, sub, sub], false, true).answers(), vec![]);
    }

    #[test]
    fn a_side_that_deleted_the_file_is_taken_by_removing_it() {
        let f = entry(FILE);
        let c = conflict([f, f, None], true, true);
        assert_eq!(
            c.commands(Answer::Theirs),
            vec![vec!["rm", "--quiet", "--", "a b.txt"]]
        );
        assert_eq!(
            c.commands(Answer::Ours),
            vec![
                vec!["checkout", "--ours", "--", "a b.txt"],
                vec!["add", "--", "a b.txt"]
            ]
        );
    }

    #[test]
    fn marker_labels_come_from_the_first_conflict() {
        let text = "a\n<<<<<<< HEAD\nx\n||||||| base\n=======\ny\n>>>>>>> 954001b (Edit list)\n\
                    <<<<<<< other\n";
        assert_eq!(
            from_markers(text),
            Some(Sides {
                ours: "HEAD".into(),
                theirs: "954001b (Edit list)".into()
            })
        );
        assert_eq!(from_markers("no markers\n"), None);
        assert_eq!(short_label("954001b (Edit list)"), "954001b");
        assert_eq!(short_label("Stashed changes"), "Stashed changes");
    }
}
