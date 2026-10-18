//! The index decides: a file is conflicted while git holds it unmerged (stages 1 to 3), and
//! resolved once staged, whatever its text. Submodules are left to the terminal.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parterre_util::CancelTree;

use crate::branches::{Catalog, Error, Report};
use crate::git::{Git, GitError};
use crate::oid::Oid;
use crate::operation::run_or_fail;

const SYMLINK_MODE: u32 = 0o120000;
const SUBMODULE_MODE: u32 = 0o160000;
/// As git's `buffer_is_binary`: a NUL in this many first bytes.
const BINARY_CHECK_LEN: usize = 8000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub mode: u32,
    pub oid: Oid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub path: String,
    /// Stages 1 (the common ancestor's), 2 (ours, `HEAD`) and 3 (theirs).
    pub stages: [Option<Entry>; 3],
    pub binary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Answer {
    Ours,
    Theirs,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    BothModified,
    BothAdded,
    DeletedByThem,
    DeletedByUs,
    AddedByUs,
    AddedByThem,
    BothDeleted,
}

impl Conflict {
    fn kind(&self) -> Kind {
        match self.stages.map(|s| s.is_some()) {
            [true, true, true] => Kind::BothModified,
            [false, true, true] => Kind::BothAdded,
            [true, true, false] => Kind::DeletedByThem,
            [true, false, true] => Kind::DeletedByUs,
            [false, true, false] => Kind::AddedByUs,
            [false, false, true] => Kind::AddedByThem,
            _ => Kind::BothDeleted,
        }
    }

    /// `git status --short`'s code.
    pub fn code(&self) -> &'static str {
        match self.kind() {
            Kind::BothModified => "UU",
            Kind::BothAdded => "AA",
            Kind::DeletedByThem => "UD",
            Kind::DeletedByUs => "DU",
            Kind::AddedByUs => "AU",
            Kind::AddedByThem => "UA",
            Kind::BothDeleted => "DD",
        }
    }

    /// What each side did, ours then theirs, in the changed-files table's letters, where git's
    /// `U` only says the file is unmerged. `·` where that side has no file at this path: a
    /// rename/rename, or a file moved aside for a directory.
    pub fn letters(&self) -> [char; 2] {
        match self.kind() {
            Kind::BothModified => ['M', 'M'],
            Kind::BothAdded => ['A', 'A'],
            Kind::DeletedByThem => ['M', 'D'],
            Kind::DeletedByUs => ['D', 'M'],
            Kind::AddedByUs => ['A', '·'],
            Kind::AddedByThem => ['·', 'A'],
            Kind::BothDeleted => ['D', 'D'],
        }
    }

    /// `git status`'s words.
    pub fn words(&self) -> &'static str {
        match self.kind() {
            Kind::BothModified => "both modified",
            Kind::BothAdded => "both added",
            Kind::DeletedByThem => "deleted by them",
            Kind::DeletedByUs => "deleted by us",
            Kind::AddedByUs => "added by us",
            Kind::AddedByThem => "added by them",
            Kind::BothDeleted => "both deleted",
        }
    }

    fn has_mode(&self, mode: u32) -> bool {
        self.stages.iter().flatten().any(|e| e.mode == mode)
    }

    pub fn is_submodule(&self) -> bool {
        self.has_mode(SUBMODULE_MODE)
    }

    /// `git mergetool` only merges content: for the rest it asks on the terminal, and a merge
    /// tool can't merge binary files.
    pub fn merge_tool(&self) -> Result<(), &'static str> {
        if self.is_submodule() {
            return Err("A submodule: resolve it in a terminal");
        }
        if self.has_mode(SYMLINK_MODE) {
            return Err("A symlink: no merge tool");
        }
        match self.kind() {
            Kind::DeletedByThem => Err("Deleted by them: no merge tool"),
            Kind::DeletedByUs => Err("Deleted by us: no merge tool"),
            Kind::AddedByUs => Err("Added by us only: no merge tool"),
            Kind::AddedByThem => Err("Added by them only: no merge tool"),
            Kind::BothDeleted => Err("Deleted by both: no merge tool"),
            Kind::BothModified | Kind::BothAdded if self.binary => {
                Err("A binary file: no merge tool")
            }
            Kind::BothModified | Kind::BothAdded => Ok(()),
        }
    }

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

    pub fn commands(&self, answer: Answer) -> Vec<Vec<String>> {
        let with_path = |words: &[&str]| -> Vec<String> {
            words
                .iter()
                .map(|w| (*w).to_owned())
                .chain([self.path.clone()])
                .collect()
        };
        if self.deletes(answer) {
            return vec![with_path(&["rm", "--quiet", "--"])];
        }
        let side = if answer == Answer::Ours {
            "--ours"
        } else {
            "--theirs"
        };
        vec![
            with_path(&["checkout", side, "--"]),
            with_path(&["add", "--"]),
        ]
    }
}

/// Finishing one conflicted file, as it was conflicted when offered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolve {
    pub conflict: Conflict,
    pub answer: Answer,
    /// The menu item it was picked as.
    pub item: String,
}

impl Resolve {
    pub fn label(&self) -> String {
        let path = &self.conflict.path;
        match self.answer {
            Answer::Delete => format!("Delete {path}"),
            Answer::Ours | Answer::Theirs => format!("{} for {path}", self.item),
        }
    }
}

pub(crate) fn execute(
    catalog: &Catalog,
    resolve: &Resolve,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<(), Error> {
    let git = Git::new(&catalog.root);
    let (_, now) = list(&git)?;
    let conflict = &resolve.conflict;
    if !now
        .iter()
        .any(|c| c.path == conflict.path && c.stages == conflict.stages)
    {
        return Err(Error::Invalid(format!(
            "{} is no longer conflicted as it was. Reload and try again.",
            conflict.path
        )));
    }
    if !conflict.answers().contains(&resolve.answer) {
        return Err(Error::Invalid(format!(
            "{} can't be finished that way.",
            conflict.path
        )));
    }
    for args in conflict.commands(resolve.answer) {
        run_or_fail(&git, args, cancel, report)?;
    }
    Ok(())
}

/// *Mine* is the side of the branch the user is on, *theirs* the one coming in: in a rebase,
/// stage 3, the user's commit being replayed onto theirs; otherwise stage 2, `HEAD`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sides {
    /// Stage 2's name.
    pub ours: Option<String>,
    /// Stage 3's name.
    pub theirs: Option<String>,
    pub mine_stage: u8,
}

impl Sides {
    pub fn name(&self, answer: Answer) -> Option<String> {
        let (stage, name) = match answer {
            Answer::Ours => (2, &self.ours),
            Answer::Theirs => (3, &self.theirs),
            Answer::Delete => return None,
        };
        let who = if stage == self.mine_stage {
            "mine"
        } else {
            "theirs"
        };
        Some(match name {
            Some(name) => format!("{who} ({name})"),
            None => who.to_owned(),
        })
    }

    pub fn mine_first(&self, mut answers: Vec<Answer>) -> Vec<Answer> {
        let mine = if self.mine_stage == 3 {
            Answer::Theirs
        } else {
            Answer::Ours
        };
        answers.sort_by_key(|a| *a != mine);
        answers
    }
}

/// The worktree's root, and its conflicted files by path.
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
    // Both sides' blobs in one git call, not one each (#309).
    let text_sides = |c: &Conflict| {
        let both = c.stages[1].is_some() && c.stages[2].is_some() && !c.is_submodule();
        c.stages[1..]
            .iter()
            .flatten()
            .filter(move |e| both && e.mode != SYMLINK_MODE)
            .map(|e| e.oid)
            .collect::<Vec<_>>()
    };
    let wanted: Vec<Oid> = conflicts.iter().flat_map(text_sides).collect();
    let binary: HashMap<Oid, bool> = wanted
        .iter()
        .copied()
        .zip(git.blobs(&wanted)?)
        .map(|(oid, bytes)| (oid, bytes.iter().take(BINARY_CHECK_LEN).any(|&b| b == 0)))
        .collect();
    for c in &mut conflicts {
        c.binary = text_sides(c)
            .iter()
            .any(|oid| binary.get(oid) == Some(&true));
    }
    Ok((root, conflicts))
}

/// Named by the operation in progress.
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
            mine_stage: 2,
        };
    };
    let rebase = ["rebase-merge", "rebase-apply"]
        .into_iter()
        .map(|d| dir.join(d))
        .find(|d| d.is_dir());
    if let Some(rebase) = rebase {
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
            mine_stage: 3,
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
                marker_labels(&String::from_utf8_lossy(&text)).map(|(_, theirs)| theirs)
            })
    };
    Sides {
        ours: branch,
        theirs,
        mine_stage: 2,
    }
}

/// Of the first conflict in `text`: `<<<<<<< ours` and `>>>>>>> theirs`.
fn marker_labels(text: &str) -> Option<(String, String)> {
    let mut lines = text.lines();
    let ours = lines.find_map(|l| l.strip_prefix("<<<<<<< "))?.trim_end();
    let theirs = lines.find_map(|l| l.strip_prefix(">>>>>>> "))?.trim_end();
    Some((ours.to_owned(), theirs.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE_MODE: u32 = 0o100644;

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

    #[test]
    fn codes_follow_the_stages_as_git_status_names_them() {
        let f = entry(FILE_MODE);
        let cases = [
            ([f, f, f], "UU", "MM", "both modified"),
            ([None, f, f], "AA", "AA", "both added"),
            ([f, f, None], "UD", "MD", "deleted by them"),
            ([f, None, f], "DU", "DM", "deleted by us"),
            ([None, f, None], "AU", "A·", "added by us"),
            ([None, None, f], "UA", "·A", "added by them"),
            ([f, None, None], "DD", "DD", "both deleted"),
        ];
        for (stages, code, letters, words) in cases {
            let c = conflict(stages, false);
            let shown: String = c.letters().iter().collect();
            assert_eq!(
                (c.code(), shown.as_str(), c.words()),
                (code, letters, words)
            );
        }
    }

    #[test]
    fn only_text_on_both_sides_goes_to_the_merge_tool() {
        let f = entry(FILE_MODE);
        assert_eq!(conflict([f, f, f], false).merge_tool(), Ok(()));
        assert_eq!(conflict([None, f, f], false).merge_tool(), Ok(()));
        assert!(conflict([f, f, f], true).merge_tool().is_err());
        assert!(conflict([f, f, None], false).merge_tool().is_err());
        let link = entry(SYMLINK_MODE);
        assert!(conflict([link, link, link], false).merge_tool().is_err());
        let sub = entry(SUBMODULE_MODE);
        assert!(conflict([sub, sub, sub], false).merge_tool().is_err());
    }

    #[test]
    fn answers_by_kind() {
        let f = entry(FILE_MODE);
        let sides = vec![Answer::Ours, Answer::Theirs];
        assert_eq!(conflict([f, f, f], false).answers(), sides);
        assert_eq!(conflict([f, f, f], true).answers(), sides);
        let link = entry(SYMLINK_MODE);
        assert_eq!(conflict([link, link, link], false).answers(), sides);
        assert_eq!(conflict([f, f, None], false).answers(), sides);
        assert_eq!(
            conflict([f, None, None], false).answers(),
            vec![Answer::Delete]
        );
        let sub = entry(SUBMODULE_MODE);
        assert_eq!(conflict([sub, sub, sub], false).answers(), vec![]);
    }

    #[test]
    fn items_name_mine_and_theirs_and_say_what_deletes() {
        let f = entry(FILE_MODE);
        let merge = Sides {
            ours: Some("merge-here".into()),
            theirs: Some("feature".into()),
            mine_stage: 2,
        };
        let c = conflict([f, f, None], false);
        assert_eq!(c.item(&merge, Answer::Ours), "Use mine (merge-here)");
        assert_eq!(
            c.item(&merge, Answer::Theirs),
            "Use theirs (feature, deleted)"
        );
        let rebase = Sides {
            ours: Some("main".into()),
            theirs: Some("topic".into()),
            mine_stage: 3,
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
            mine_stage: 2,
        };
        assert_eq!(c.item(&unnamed, Answer::Theirs), "Use theirs (deleted)");
    }

    #[test]
    fn a_side_that_deleted_the_file_is_taken_by_removing_it() {
        let f = entry(FILE_MODE);
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
            marker_labels(text),
            Some(("HEAD".into(), "954001b (Edit list)".into()))
        );
        assert_eq!(marker_labels("no markers\n"), None);
    }
}
