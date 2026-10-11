//! Git's `revert` has no `--autostash`, so parterre stashes uncommitted changes itself when
//! asked, and leaves putting them back to the user.

use std::collections::BTreeSet;
use std::path::Path;

use crate::branches::{Attention, Catalog, Error, Report, Stashed, Stuck};
use crate::git::{Git, GitError};
use crate::log::is_ancestor;
use crate::operation::{
    Edited, git_args, in_progress, refuse_if_stuck, run, run_or_fail, run_with_edited,
    stopped_or_not_committed,
};
use crate::repo::DEFAULT_ABBREV_LEN;
use crate::text::plural;
use crate::{Oid, Repo};
use parterre_util::CancelTree;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revert {
    pub branch: Option<String>,
    pub head: Oid,
    pub commit: Oid,
    pub is_merge: bool,
    pub message: Option<String>,
    pub stash: bool,
}

impl Revert {
    pub fn name(&self) -> &str {
        self.branch.as_deref().unwrap_or("HEAD")
    }
}

pub fn command(revert: &Revert) -> Vec<String> {
    let edit = if revert.message.is_some() {
        "--edit"
    } else {
        "--no-edit"
    };
    let mut args = vec!["revert".to_owned(), edit.to_owned()];
    if revert.is_merge {
        args.extend(["-m".to_owned(), "1".to_owned()]);
    }
    args.push(revert.commit.to_hex());
    args
}

pub fn commands(revert: &Revert) -> Vec<Vec<String>> {
    let mut steps = Vec::new();
    if revert.stash {
        steps.push(stash_command(revert.commit));
    }
    steps.push(command(revert));
    steps
}

fn stash_command(commit: Oid) -> Vec<String> {
    git_args(&["stash", "push", "-m", &stash_message(commit)])
}

fn stash_message(commit: Oid) -> String {
    format!(
        "autostash for reverting {}",
        commit.short(DEFAULT_ABBREV_LEN)
    )
}

pub(crate) fn stash_entry(git: &Git, commit: Oid) -> Option<String> {
    let suffix = format!(": {}", stash_message(commit));
    stash_named(git, "%gs", |subject| subject.ends_with(&suffix))
        .ok()
        .flatten()
}

/// The name, such as `stash@{0}`, of the first stash entry whose `field` matches.
fn stash_named(
    git: &Git,
    field: &str,
    matches: impl Fn(&str) -> bool,
) -> Result<Option<String>, GitError> {
    Ok(git
        .run(&["stash", "list", &format!("--format=%gd%x00{field}")])?
        .lines()
        .filter_map(|l| l.split_once('\0'))
        .find(|(_, value)| matches(value))
        .map(|(name, _)| name.to_owned()))
}

pub fn offered(repo: &Repo, catalog: &Catalog, commit: Oid) -> Option<String> {
    if !catalog.has_working_tree {
        return None;
    }
    let head = repo.lookup(&catalog.head?)?;
    let commit = repo.lookup(&commit)?;
    is_ancestor(repo, commit, head).then(|| catalog.current.clone().unwrap_or("HEAD".into()))
}

#[derive(Clone, Debug)]
pub struct Preview {
    pub branch: Option<String>,
    pub head: Oid,
    pub commit: Oid,
    pub is_merge: bool,
    pub message: String,
    pub revert_reference: bool,
    /// Tracked files only, staged or not.
    pub uncommitted: Vec<String>,
    pub staged: Vec<String>,
    /// Uncommitted files the revert changes too.
    pub overlap: Vec<String>,
}

impl Preview {
    pub fn load(path: &Path, commit: Oid) -> Result<Preview, Error> {
        let catalog = Catalog::load(path)?;
        refuse_if_stuck(&catalog)?;
        let Some(head) = catalog.head.filter(|_| catalog.has_working_tree) else {
            return Err(Error::Invalid(
                "There's nothing checked out to revert on.".into(),
            ));
        };
        let git = Git::new(&catalog.root);
        if !git.is_ancestor(commit, head)? {
            return Err(Error::Invalid(format!(
                "{} is not on {}. Reload and try again.",
                commit.short(DEFAULT_ABBREV_LEN),
                catalog.current.as_deref().unwrap_or("HEAD")
            )));
        }
        let parents = git.run(&["rev-list", "--parents", "-n", "1", &commit.to_hex()])?;
        let is_merge = parents.split_whitespace().count() > 2;
        let revert_reference = git
            .query(&["config", "--type=bool", "--get", "revert.reference"])?
            .is_some_and(|v| v.trim() == "true");
        let message = gits_message(&git, commit, is_merge, revert_reference)?;
        let (uncommitted, staged) = local_changes(&git)?;
        let touched = touched(&git, commit)?;
        let overlap = uncommitted
            .iter()
            .filter(|f| touched.contains(f))
            .cloned()
            .collect();
        Ok(Preview {
            branch: catalog.current.clone(),
            head,
            commit,
            is_merge,
            message,
            revert_reference,
            uncommitted,
            staged,
            overlap,
        })
    }

    pub fn name(&self) -> &str {
        self.branch.as_deref().unwrap_or("HEAD")
    }

    pub fn stashable(&self) -> bool {
        !self.uncommitted.is_empty()
    }

    /// Git refuses to revert over any staged change, and over local changes to the files the
    /// revert changes.
    pub fn refused(&self, stash: bool) -> Vec<String> {
        if stash && self.stashable() {
            return Vec::new();
        }
        let files: BTreeSet<&String> = self.staged.iter().chain(&self.overlap).collect();
        files.into_iter().cloned().collect()
    }

    pub fn refusal(&self, stash: bool) -> Option<String> {
        let n = self.refused(stash).len();
        if n == 0 {
            return None;
        }
        Some(if self.overlap.is_empty() {
            let have = if n == 1 { "has" } else { "have" };
            format!("{} {have} staged changes", plural(n, "file"))
        } else {
            format!(
                "{} changes {} with local changes",
                self.commit.short(DEFAULT_ABBREV_LEN),
                plural(n, "file")
            )
        })
    }

    pub fn caution(&self, stash: bool) -> Option<String> {
        (self.stashable() && !stash && self.refusal(false).is_none()).then(|| {
            let n = self.uncommitted.len();
            let stay = if n == 1 { "stays" } else { "stay" };
            format!(
                "{} with local changes {stay} in the worktree",
                plural(n, "file")
            )
        })
    }

    pub fn blocked(&self, stash: bool, message: &str) -> Option<String> {
        if let Some(why) = self.refusal(stash) {
            return Some(format!("{why}."));
        }
        let title = message.lines().next().unwrap_or("").trim();
        if title.is_empty() {
            return Some(if message.trim().is_empty() {
                "Enter a message for the revert.".into()
            } else {
                "Say why you're reverting on the title line.".into()
            });
        }
        None
    }

    pub fn revert(&self, stash: bool, message: &str) -> Revert {
        // Git words it as shown, except before 2.43 for a revert's revert, and with
        // `revert.reference`, where it leaves a comment for the title.
        let git_words_it = message.trim_end() == self.message.trim_end()
            && !self.revert_reference
            && !self.message.starts_with("Reapply \"");
        Revert {
            branch: self.branch.clone(),
            head: self.head,
            commit: self.commit,
            is_merge: self.is_merge,
            message: (!git_words_it).then(|| format!("{}\n", message.trim_end())),
            stash: stash && self.stashable(),
        }
    }
}

fn gits_message(
    git: &Git,
    commit: Oid,
    is_merge: bool,
    revert_reference: bool,
) -> Result<String, Error> {
    let hex = commit.to_hex();
    let name = |rev: &str| -> Result<String, Error> {
        let out = if revert_reference {
            git.run(&["show", "-s", "--pretty=reference", rev, "--"])?
        } else {
            git.run(&["rev-parse", "--verify", &format!("{rev}^{{commit}}")])?
        };
        Ok(out.trim().to_owned())
    };
    let body = git.run(&["show", "-s", "--format=%B", &hex, "--"])?;
    let subject = body.lines().next().unwrap_or("").trim_end();
    let title = if revert_reference {
        String::new()
    } else if let Some(original) = subject.strip_prefix("Revert \"")
        && !original.starts_with("Revert \"")
    {
        // As git 2.43 and later word it, leaving repeated reverts alone.
        format!("Reapply \"{original}")
    } else {
        format!("Revert \"{subject}\"")
    };
    let mut text = format!("{title}\n\nThis reverts commit {}", name(&hex)?);
    if is_merge {
        text.push_str(&format!(
            ", reversing\nchanges made to {}",
            name(&format!("{hex}^1"))?
        ));
    }
    text.push_str(".\n");
    Ok(text)
}

/// The files `commit` changed from its first parent.
fn touched(git: &Git, commit: Oid) -> Result<Vec<String>, Error> {
    let out = git.run(&[
        "diff-tree",
        "-r",
        "-z",
        "--no-commit-id",
        "--name-only",
        "--root",
        "-m",
        "--first-parent",
        &commit.to_hex(),
        "--",
    ])?;
    Ok(nul_separated(&out))
}

fn nul_separated(out: &str) -> Vec<String> {
    out.split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The tracked files with uncommitted changes, and those of them staged. A rename counts by
/// both its names.
fn local_changes(git: &Git) -> Result<(Vec<String>, Vec<String>), Error> {
    let out = git.run(&["status", "--porcelain=v1", "-z", "--untracked-files=no"])?;
    let (mut uncommitted, mut staged) = (BTreeSet::new(), BTreeSet::new());
    let mut entries = out.split('\0').filter(|e| !e.is_empty());
    while let Some(entry) = entries.next() {
        let (Some(x), Some(path)) = (entry.chars().next(), entry.get(3..)) else {
            continue;
        };
        let renamed_from = if matches!(x, 'R' | 'C') {
            entries.next()
        } else {
            None
        };
        for path in std::iter::once(path).chain(renamed_from) {
            if x != ' ' {
                staged.insert(path.to_owned());
            }
            uncommitted.insert(path.to_owned());
        }
    }
    Ok((
        uncommitted.into_iter().collect(),
        staged.into_iter().collect(),
    ))
}

fn stash_top(git: &Git) -> Option<String> {
    git.query(&["rev-parse", "--verify", "--quiet", "refs/stash"])
        .ok()
        .flatten()
        .map(|s| s.trim().to_owned())
}

pub(crate) fn execute(
    catalog: &Catalog,
    revert: &Revert,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<(), Error> {
    let name = revert.name();
    if catalog.current != revert.branch || catalog.head != Some(revert.head) {
        return Err(Error::Invalid(format!(
            "{name} moved, or is no longer checked out here. Reload and try again."
        )));
    }
    refuse_if_stuck(catalog)?;
    let git = Git::new(&catalog.root);
    let short = revert.commit.short(DEFAULT_ABBREV_LEN);
    if !git.is_ancestor(revert.commit, revert.head)? {
        return Err(Error::Invalid(format!("{short} is not on {name}.")));
    }
    let (uncommitted, staged) = local_changes(&git)?;
    if revert.stash && !uncommitted.is_empty() {
        if let Some(stashed) = stash(&git, revert.commit, cancel, report)? {
            report.stash = Some(stashed);
        }
    } else if !revert.stash {
        // Changes made since the dialog may be ones git refuses to revert over.
        let touched = touched(&git, revert.commit)?;
        if !staged.is_empty() || uncommitted.iter().any(|f| touched.contains(f)) {
            return Err(Error::Invalid(
                "Your local changes changed. Reload and try again.".into(),
            ));
        }
    }
    let ok = match &revert.message {
        None => run(&git, command(revert), cancel, report)?,
        Some(message) => run_with_edited(
            &git,
            command(revert),
            Edited::CommitMessage,
            message,
            cancel,
            report,
        )?,
    };
    let after = Catalog::load(&catalog.root)?;
    if !ok
        && after.head == Some(revert.head)
        && after.conflicted.is_empty()
        && index_matches_head(&git)?
        && !untracked_in_the_way(&git, revert.commit)?
    {
        // The commit's changes are undone already. Git 2.34 leaves the revert in progress
        // then; later versions don't.
        if matches!(after.stuck(), Some(Stuck::InProgress(_))) {
            run(&git, git_args(&["revert", "--abort"]), cancel, report)?;
        }
        report.attention = Some(Attention {
            title: format!("Nothing to revert: {short}'s changes are already undone"),
            message: String::new(),
        });
        return Ok(());
    }
    if let Some(Stuck::InProgress(_)) = after.stuck() {
        let mut attention = in_progress(stopped_or_not_committed("Revert", after.conflicted.len()));
        if let Some(stashed) = report.stash.take() {
            let entry = stash_entry(&git, revert.commit).unwrap_or_else(|| stashed.oid.to_hex());
            attention
                .message
                .push_str(&format!(" Your changes are stashed in {entry}."));
        }
        report.attention = Some(attention);
        return Ok(());
    }
    if !ok {
        return Err(Error::Failed(report.last_output()));
    }
    report.created = after.head;
    Ok(())
}

fn stash(
    git: &Git,
    commit: Oid,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<Option<Stashed>, Error> {
    let before = stash_top(git);
    run_or_fail(git, stash_command(commit), cancel, report)?;
    let Some(top) = stash_top(git).filter(|top| Some(top) != before.as_ref()) else {
        return Ok(None);
    };
    let Some(oid) = Oid::from_hex(&top) else {
        return Ok(None);
    };
    let files = git.run(&["stash", "show", "--name-only", "-z", &top])?;
    Ok(Some(Stashed {
        oid,
        name: stash_entry(git, commit).unwrap_or_else(|| "stash@{0}".into()),
        files: nul_separated(&files),
    }))
}

/// Git fails without a conflict where the revert would write over untracked files.
fn untracked_in_the_way(git: &Git, commit: Oid) -> Result<bool, Error> {
    let touched = touched(git, commit)?;
    if touched.is_empty() {
        return Ok(false);
    }
    let mut args = vec!["ls-files", "--others", "--exclude-standard", "--"];
    args.extend(touched.iter().map(String::as_str));
    Ok(!git.run(&args)?.trim().is_empty())
}

fn index_matches_head(git: &Git) -> Result<bool, Error> {
    Ok(git
        .query(&["diff", "--cached", "--quiet", "HEAD", "--"])?
        .is_some())
}

pub(crate) fn restore(
    catalog: &Catalog,
    stash: Oid,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<(), Error> {
    refuse_if_stuck(catalog)?;
    let git = Git::new(&catalog.root);
    let hex = stash.to_hex();
    let entry = stash_named(&git, "%H", |oid| oid == hex)?.ok_or_else(|| {
        Error::Invalid(format!(
            "The stash entry {} is gone from the stash list.",
            stash.short(DEFAULT_ABBREV_LEN)
        ))
    })?;
    let ok = run(&git, git_args(&["stash", "pop", &entry]), cancel, report)?;
    let after = Catalog::load(&catalog.root)?;
    if !after.conflicted.is_empty() {
        let n = after.conflicted.len();
        report.attention = Some(Attention {
            title: format!(
                "Restoring stashed changes conflicted in {}",
                plural(n, "file")
            ),
            message: format!(
                "Resolve {} with git. Your changes are also kept in {entry} until you drop it.",
                if n == 1 { "it" } else { "them" }
            ),
        });
        return Ok(());
    }
    if !ok {
        return Err(Error::Failed(report.last_output()));
    }
    Ok(())
}
