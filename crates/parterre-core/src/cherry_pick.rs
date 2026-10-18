//! The commits go to a plain `git cherry-pick` as an explicit list, leaving out merges and
//! commits whose change is already there, which git would stop on. Git has no autostash for
//! it, so *Stash changes* stashes around it.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::branches::{Catalog, Error, Report, Stuck};
use crate::git::Git;
use crate::log::is_ancestor;
use crate::operation::{
    git_args, has_tracked_changes, in_progress, kept_stash, refuse_if_moved, refuse_if_stuck, run,
    run_or_fail, stash_count, stopped,
};
use crate::repo::DEFAULT_ABBREV_LEN;
use crate::{Oid, Repo};
use parterre_util::CancelTree;

const STASH_MESSAGE: &str = "parterre: before cherry-pick";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Picks {
    /// Every commit of this one the branch lacks.
    Lacking(Oid),
    /// Newest first, as the log lists them.
    Chosen(Vec<Oid>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CherryPick {
    pub branch: String,
    pub head: Oid,
    /// Oldest first.
    pub commits: Vec<Oid>,
    pub name: String,
    pub record_origin: bool,
    pub stash: bool,
}

pub fn command(pick: &CherryPick) -> Vec<String> {
    let mut args = vec!["cherry-pick".to_owned()];
    if pick.record_origin {
        args.push("-x".into());
    }
    args.extend(pick.commits.iter().map(Oid::to_hex));
    args
}

pub fn commands(pick: &CherryPick) -> Vec<Vec<String>> {
    if pick.stash {
        vec![stash_push(), command(pick), stash_pop()]
    } else {
        vec![command(pick)]
    }
}

fn stash_push() -> Vec<String> {
    git_args(&["stash", "push", "-m", STASH_MESSAGE])
}

fn stash_pop() -> Vec<String> {
    git_args(&["stash", "pop"])
}

/// Offered while the open worktree is stuck too, to be greyed out.
pub fn offered<'a>(repo: &Repo, catalog: &'a Catalog, commit: Oid) -> Option<&'a str> {
    offered_chosen(repo, catalog, &[commit])
}

pub fn offered_chosen<'a>(repo: &Repo, catalog: &'a Catalog, commits: &[Oid]) -> Option<&'a str> {
    if !catalog.has_working_tree {
        return None;
    }
    let branch = catalog.current.as_deref()?;
    let head = repo.lookup(&catalog.head?)?;
    commits
        .iter()
        .filter_map(|c| repo.lookup(c))
        .any(|c| !is_ancestor(repo, c, head))
        .then_some(branch)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skipped {
    OnBranch,
    /// The same change, by patch id.
    AlreadyThere,
    Merge,
}

impl Skipped {
    pub fn reason(self) -> &'static str {
        match self {
            Skipped::OnBranch => "Already on the branch: left out",
            Skipped::AlreadyThere => "Its change is already on the branch: left out",
            Skipped::Merge => "A merge: left out",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Preview {
    pub branch: String,
    pub head: Oid,
    /// Newest first, as the log lists them.
    pub listed: Vec<Oid>,
    skipped: HashMap<Oid, Skipped>,
    dropped: HashSet<Oid>,
    /// What the graph's menu named the commits by.
    pub name: Option<String>,
    pub dirty: bool,
}

impl Preview {
    pub fn load(path: &Path, picks: &Picks, name: Option<String>) -> Result<Preview, Error> {
        let catalog = Catalog::load(path)?;
        refuse_if_stuck(&catalog)?;
        let (Some(branch), Some(head)) = (catalog.current.clone(), catalog.head) else {
            return Err(Error::Invalid(
                "There's no branch checked out to cherry-pick onto.".into(),
            ));
        };
        let git = Git::new(&catalog.root);
        let oids = |out: &str| -> Vec<Oid> { out.lines().filter_map(Oid::from_hex).collect() };
        let listed = match picks {
            Picks::Lacking(tip) => oids(&git.run(&[
                "rev-list",
                "--topo-order",
                &tip.to_hex(),
                &format!("^{head}"),
            ])?),
            Picks::Chosen(commits) => commits.clone(),
        };
        let mut skipped = HashMap::new();
        // On the branch: not among what the listed commits reach and it doesn't.
        let not_on_branch: HashSet<Oid> = oids(&git.run_with_input(
            &["rev-list", "--stdin"],
            format!("{}^{head}\n", hex_lines(&listed)),
        )?)
        .into_iter()
        .collect();
        for &c in &listed {
            if !not_on_branch.contains(&c) {
                skipped.insert(c, Skipped::OnBranch);
            }
        }
        let parents = git.run_with_input(
            &["rev-list", "--no-walk=unsorted", "--parents", "--stdin"],
            hex_lines(&listed),
        )?;
        for line in parents.lines() {
            let mut words = line.split(' ');
            if let Some(c) = words.next().and_then(Oid::from_hex)
                && words.count() > 1
            {
                skipped.entry(c).or_insert(Skipped::Merge);
            }
        }
        let candidates: Vec<Oid> = listed
            .iter()
            .copied()
            .filter(|c| !skipped.contains_key(c))
            .collect();
        for c in already_there(&git, head, &candidates)? {
            skipped.insert(c, Skipped::AlreadyThere);
        }
        Ok(Preview {
            branch,
            head,
            listed,
            skipped,
            dropped: HashSet::new(),
            name,
            dirty: has_tracked_changes(&git)?,
        })
    }

    pub fn skipped(&self, commit: Oid) -> Option<Skipped> {
        self.skipped.get(&commit).copied()
    }

    /// `None` when it's left out anyway.
    pub fn picked(&self, commit: Oid) -> Option<bool> {
        (self.listed.contains(&commit) && self.skipped(commit).is_none())
            .then(|| !self.dropped.contains(&commit))
    }

    pub fn set_picked(&mut self, commit: Oid, picked: bool) {
        if self.picked(commit).is_none() {
            return;
        }
        if picked {
            self.dropped.remove(&commit);
        } else {
            self.dropped.insert(commit);
        }
    }

    /// Oldest first.
    pub fn commits(&self) -> Vec<Oid> {
        self.listed
            .iter()
            .rev()
            .copied()
            .filter(|&c| self.picked(c) == Some(true))
            .collect()
    }

    pub fn blocked(&self) -> Option<&'static str> {
        self.commits()
            .is_empty()
            .then_some("Pick at least one commit.")
    }

    pub fn cherry_pick(&self, record_origin: bool, stash: bool) -> CherryPick {
        let commits = self.commits();
        let name = match (&self.name, commits.as_slice()) {
            (Some(name), _) => name.clone(),
            (None, [one]) => one.short(DEFAULT_ABBREV_LEN),
            (None, many) => format!("{} commits", many.len()),
        };
        CherryPick {
            branch: self.branch.clone(),
            head: self.head,
            commits,
            name,
            record_origin,
            stash: stash && self.dirty,
        }
    }
}

fn hex_lines(commits: &[Oid]) -> String {
    commits.iter().map(|c| format!("{c}\n")).collect()
}

/// Which of `commits` make a change the branch at `head` already has, by git's patch ids:
/// compared with the branch's commits that none of them reaches, as `--cherry-mark` does.
fn already_there(git: &Git, head: Oid, commits: &[Oid]) -> Result<Vec<Oid>, Error> {
    if commits.is_empty() {
        return Ok(Vec::new());
    }
    let patches = |args: &[&str], input: String| {
        let mut all = vec![
            "log",
            "-p",
            "--no-merges",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--format=commit %H",
            "--stdin",
        ];
        all.extend_from_slice(args);
        git.run_with_input(&all, input)
    };
    let mut ours = format!("{head}\n");
    for c in commits {
        ours.push_str(&format!("^{c}\n"));
    }
    let mut text = patches(&["--no-walk=unsorted"], hex_lines(commits))?;
    let branch = patches(&[], ours)?;
    if branch.trim().is_empty() {
        return Ok(Vec::new());
    }
    text.push('\n');
    text.push_str(&branch);
    let ids = git.run_with_input(&["patch-id", "--stable"], text)?;
    let mut by_commit = HashMap::new();
    for line in ids.lines() {
        if let Some((id, commit)) = line.split_once(' ')
            && let Some(commit) = Oid::from_hex(commit)
        {
            by_commit.insert(commit, id.to_owned());
        }
    }
    let wanted: HashSet<Oid> = commits.iter().copied().collect();
    let on_branch: HashSet<&str> = by_commit
        .iter()
        .filter(|(c, _)| !wanted.contains(c))
        .map(|(_, id)| id.as_str())
        .collect();
    Ok(commits
        .iter()
        .copied()
        .filter(|c| {
            by_commit
                .get(c)
                .is_some_and(|id| on_branch.contains(id.as_str()))
        })
        .collect())
}

pub(crate) fn execute(
    catalog: &Catalog,
    pick: &CherryPick,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<(), Error> {
    refuse_if_moved(catalog, &pick.branch, pick.head)?;
    refuse_if_stuck(catalog)?;
    if pick.commits.is_empty() {
        return Err(Error::Invalid("There's nothing to cherry-pick.".into()));
    }
    let git = Git::new(&catalog.root);
    // Only a stash entry made here is put back: with nothing to stash, git makes none.
    let stashed = if pick.stash {
        let before = stash_count(&git);
        run_or_fail(&git, stash_push(), cancel, report)?;
        stash_count(&git) > before
    } else {
        false
    };
    let ok = run(&git, command(pick), cancel, report)?;
    let pick_output = report.last_output();
    let after = Catalog::load(&catalog.root)?;
    if let Some(Stuck::InProgress(_)) = after.stuck() {
        let mut attention = in_progress(stopped("Cherry-pick", after.conflicted.len()));
        if stashed {
            attention.message.push_str(
                " Your changes are kept in stash@{0}: put them back with git stash pop once \
                 it's done.",
            );
        }
        report.attention = Some(attention);
        return Ok(());
    }
    let popped = !stashed || run(&git, stash_pop(), cancel, report)?;
    if !ok {
        return Err(Error::Failed(if popped {
            pick_output
        } else {
            format!("{pick_output}\nYour changes are kept in a stash entry: git stash pop.")
        }));
    }
    if !popped {
        let after = Catalog::load(&catalog.root)?;
        report.attention = Some(kept_stash(
            format!("Cherry-picked {} onto {}", pick.name, pick.branch),
            after.conflicted.len(),
        ));
    }
    Ok(())
}
