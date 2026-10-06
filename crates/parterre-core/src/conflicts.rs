//! Conflicted files: what git's index holds for each, what kind of conflict that is, and what
//! can finish it. Content conflicts go to the user's merge tool; any conflict but a submodule's
//! can also be finished by taking one side whole, with one git command. Submodules are left to
//! the terminal.
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
}

/// What finishes a conflicted file in parterre, in one git command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Answer {
    /// Take our side (stage 2): its file, or no file if it deleted it.
    Ours,
    /// Take their side (stage 3).
    Theirs,
    /// Remove a file both sides deleted (`git rm`).
    Delete,
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

    /// What finishes it in parterre: either side, whole, or deleting what both sides deleted.
    /// Nothing for submodules (the terminal's).
    pub fn answers(&self) -> Vec<Answer> {
        if self.is_submodule() {
            Vec::new()
        } else if self.stages[1].is_none() && self.stages[2].is_none() {
            vec![Answer::Delete]
        } else {
            vec![Answer::Ours, Answer::Theirs]
        }
    }

    /// Whether `answer`'s side deleted the file, so taking it deletes it.
    pub fn deletes(&self, answer: Answer) -> bool {
        match answer {
            Answer::Ours => self.stages[1].is_none(),
            Answer::Theirs => self.stages[2].is_none(),
            Answer::Delete => true,
        }
    }

    /// The menu item for `answer`: `Use mine (merge-here)`, `Use theirs (feature, deleted)`,
    /// `Delete`.
    pub fn item(&self, sides: &Sides, answer: Answer) -> String {
        let Some(side) = sides.name(answer) else {
            return "Delete".to_owned();
        };
        if self.deletes(answer) {
            match side.strip_suffix(')') {
                Some(named) => format!("Use {named}, deleted)"),
                None => format!("Use {side} (deleted)"),
            }
        } else {
            format!("Use {side}")
        }
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
    /// The menu item it was picked as, such as `Use theirs (feature)`.
    pub item: String,
}

impl Resolve {
    /// `Use theirs (feature) for a.txt`, `Delete a.txt`.
    pub fn label(&self) -> String {
        let path = &self.conflict.path;
        match self.answer {
            Answer::Delete => format!("Delete {path}"),
            Answer::Ours | Answer::Theirs => format!("{} for {path}", self.item),
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

/// The two sides, as the user knows them: *mine* is the side of the branch the user is on,
/// *theirs* the one coming in. In a rebase that's stage 3, the user's commit being replayed
/// onto theirs; otherwise stage 2, `HEAD`. Each is named, where git says by what: a branch, a
/// commit, or the label git wrote in the markers (`Stashed changes`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sides {
    /// Stage 2's name.
    pub ours: Option<String>,
    /// Stage 3's name.
    pub theirs: Option<String>,
    /// The stage that is *mine*: 3 in a rebase, else 2.
    pub mine: u8,
}

impl Sides {
    /// `mine (merge-here)`, `theirs (feature)`, or `theirs` when it has no name; `None` for
    /// [`Answer::Delete`].
    pub fn name(&self, answer: Answer) -> Option<String> {
        let (stage, name) = match answer {
            Answer::Ours => (2, &self.ours),
            Answer::Theirs => (3, &self.theirs),
            Answer::Delete => return None,
        };
        let who = if stage == self.mine { "mine" } else { "theirs" };
        Some(match name {
            Some(name) => format!("{who} ({name})"),
            None => who.to_owned(),
        })
    }

    /// `answers` with mine first.
    pub fn mine_first(&self, mut answers: Vec<Answer>) -> Vec<Answer> {
        let mine = if self.mine == 3 {
            Answer::Theirs
        } else {
            Answer::Ours
        };
        answers.sort_by_key(|a| *a != mine);
        answers
    }
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
            });
        }
        conflicts.last_mut().expect("pushed").stages[stage - 1] = Some(entry);
    }
    // Both sides' blobs in one git call, not one each, to tell binary files: those with a NUL
    // in their first few bytes, as git tells them (#309).
    let sides = |c: &Conflict| {
        let both = c.stages[1].is_some() && c.stages[2].is_some() && !c.is_submodule();
        c.stages[1..]
            .iter()
            .flatten()
            .filter(move |e| both && e.mode != SYMLINK)
            .map(|e| e.oid)
            .collect::<Vec<_>>()
    };
    let wanted: Vec<Oid> = conflicts.iter().flat_map(sides).collect();
    let binary: std::collections::HashMap<Oid, bool> = wanted
        .iter()
        .copied()
        .zip(git.blobs(&wanted)?)
        .map(|(oid, bytes)| (oid, bytes.iter().take(FIRST_FEW_BYTES).any(|&b| b == 0)))
        .collect();
    for c in &mut conflicts {
        c.binary = sides(c).iter().any(|oid| binary.get(oid) == Some(&true));
    }
    Ok((root, conflicts))
}

/// The sides of the open worktree's conflicts, named by the operation in progress.
pub fn sides(git: &Git, root: &Path, conflicts: &[Conflict]) -> Sides {
    let query = |args: &[&str]| -> Option<String> {
        git.query(args).ok().flatten().filter(|s| !s.is_empty())
    };
    let short = |rev: &str| query(&["rev-parse", "--short", "--verify", "-q", rev]);
    let read = |path: PathBuf| -> Option<String> {
        let text = std::fs::read_to_string(path).ok()?;
        Some(text.trim().to_owned()).filter(|t| !t.is_empty())
    };
    let branch = query(&["symbolic-ref", "--short", "-q", "HEAD"]).or_else(|| short("HEAD"));
    let Some(dir) = query(&["rev-parse", "--absolute-git-dir"]).map(PathBuf::from) else {
        return Sides {
            ours: branch,
            theirs: None,
            mine: 2,
        };
    };
    let rebase = ["rebase-merge", "rebase-apply"]
        .into_iter()
        .map(|d| dir.join(d))
        .find(|d| d.is_dir());
    if let Some(rebase) = rebase {
        // Replaying the user's branch onto theirs.
        let replayed = read(rebase.join("head-name"))
            .map(|h| h.trim_start_matches("refs/heads/").to_owned())
            .filter(|h| h != "detached HEAD")
            .or_else(|| short("REBASE_HEAD"));
        let onto = read(rebase.join("onto")).and_then(|oid| {
            query(&[
                "for-each-ref",
                "--points-at",
                &oid,
                "--format=%(refname:short)",
                "refs/heads",
            ])
            .and_then(|names| names.lines().next().map(str::to_owned))
            .or_else(|| short(&oid))
        });
        return Sides {
            ours: onto,
            theirs: replayed,
            mine: 3,
        };
    }
    let theirs = if dir.join("MERGE_HEAD").is_file() {
        // `Merge branch 'feature'`: what the user merged, by the name they gave.
        read(dir.join("MERGE_MSG"))
            .and_then(|m| Some(m.lines().next()?.split('\'').nth(1)?.to_owned()))
            .or_else(|| short("MERGE_HEAD"))
    } else if let Some(picked) = short("CHERRY_PICK_HEAD") {
        Some(picked)
    } else if let Some(reverted) = short("REVERT_HEAD") {
        Some(format!("revert of {reverted}"))
    } else {
        // No operation (a stash pop): the label git wrote in a text conflict's markers.
        conflicts
            .iter()
            .filter(|c| c.merge_tool().is_ok())
            .find_map(|c| {
                let text = std::fs::read(root.join(&c.path)).ok()?;
                from_markers(&String::from_utf8_lossy(&text)).map(|(_, theirs)| theirs)
            })
    };
    Sides {
        ours: branch,
        theirs,
        mine: 2,
    }
}

/// The labels of the first conflict's markers in `text`: `<<<<<<< ours` and `>>>>>>> theirs`.
pub fn from_markers(text: &str) -> Option<(String, String)> {
    let mut lines = text.lines();
    let ours = lines.find_map(|l| l.strip_prefix("<<<<<<< "))?.trim_end();
    let theirs = lines.find_map(|l| l.strip_prefix(">>>>>>> "))?.trim_end();
    Some((ours.to_owned(), theirs.to_owned()))
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

    fn conflict(stages: [Option<Entry>; 3], binary: bool) -> Conflict {
        Conflict {
            path: "a b.txt".into(),
            stages,
            binary,
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
            let c = conflict(stages, false);
            assert_eq!((c.code(), c.words()), (code, words));
        }
    }

    #[test]
    fn only_text_on_both_sides_goes_to_the_merge_tool() {
        let f = entry(FILE);
        assert_eq!(conflict([f, f, f], false).merge_tool(), Ok(()));
        assert_eq!(conflict([None, f, f], false).merge_tool(), Ok(()));
        assert!(conflict([f, f, f], true).merge_tool().is_err());
        assert!(conflict([f, f, None], false).merge_tool().is_err());
        let link = entry(SYMLINK);
        assert!(conflict([link, link, link], false).merge_tool().is_err());
        let sub = entry(GITLINK);
        assert!(conflict([sub, sub, sub], false).merge_tool().is_err());
    }

    #[test]
    fn answers_by_kind() {
        let f = entry(FILE);
        let sides = vec![Answer::Ours, Answer::Theirs];
        assert_eq!(conflict([f, f, f], false).answers(), sides);
        assert_eq!(conflict([f, f, f], true).answers(), sides);
        let link = entry(SYMLINK);
        assert_eq!(conflict([link, link, link], false).answers(), sides);
        assert_eq!(conflict([f, f, None], false).answers(), sides);
        assert_eq!(
            conflict([f, None, None], false).answers(),
            vec![Answer::Delete]
        );
        let sub = entry(GITLINK);
        assert_eq!(conflict([sub, sub, sub], false).answers(), vec![]);
    }

    #[test]
    fn items_name_mine_and_theirs_and_say_what_deletes() {
        let f = entry(FILE);
        let merge = Sides {
            ours: Some("merge-here".into()),
            theirs: Some("feature".into()),
            mine: 2,
        };
        let c = conflict([f, f, None], false);
        assert_eq!(c.item(&merge, Answer::Ours), "Use mine (merge-here)");
        assert_eq!(
            c.item(&merge, Answer::Theirs),
            "Use theirs (feature, deleted)"
        );
        // A rebase replays the user's commit (stage 3) onto theirs (stage 2).
        let rebase = Sides {
            ours: Some("main".into()),
            theirs: Some("topic".into()),
            mine: 3,
        };
        assert_eq!(c.item(&rebase, Answer::Ours), "Use theirs (main)");
        assert_eq!(c.item(&rebase, Answer::Theirs), "Use mine (topic, deleted)");
        assert_eq!(
            rebase.mine_first(c.answers()),
            vec![Answer::Theirs, Answer::Ours]
        );
        let unnamed = Sides {
            ours: None,
            theirs: None,
            mine: 2,
        };
        assert_eq!(c.item(&unnamed, Answer::Theirs), "Use theirs (deleted)");
    }

    #[test]
    fn a_side_that_deleted_the_file_is_taken_by_removing_it() {
        let f = entry(FILE);
        let c = conflict([f, f, None], true);
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
            Some(("HEAD".into(), "954001b (Edit list)".into()))
        );
        assert_eq!(from_markers("no markers\n"), None);
    }
}
