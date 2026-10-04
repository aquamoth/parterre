//! Cherry-picking commits onto the open worktree's branch: either those chosen in the log,
//! which need not be adjacent, or, from the graph, every commit of a node the branch lacks.
//! They go on oldest first, as if applied one at a time, with a plain `git cherry-pick` of an
//! explicit list: merges, and commits whose change is already there, are left out, since git
//! would stop on them. Git has no autostash for it, so *Stash changes* stashes around it. A
//! cherry-pick that stops on conflicts leaves the worktree with an operation in progress,
//! finished with git for now.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::branches::{Attention, Catalog, Error, Report, Stuck, run};
use crate::git::Git;
use crate::log::is_ancestor;
use crate::{Oid, Repo};
use parterre_util::CancelTree;

/// The message of the stash entry *Stash changes* makes.
pub const STASH_MESSAGE: &str = "parterre: before cherry-pick";

/// What to cherry-pick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Picks {
    /// Every commit of this one the branch lacks, as a rebase would replay them; the graph's.
    Lacking(Oid),
    /// These commits, newest first as the log lists them; the log's selection.
    Chosen(Vec<Oid>),
}

/// A cherry-pick the user agreed to. It runs only while the branch is still where it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CherryPick {
    pub branch: String,
    /// Where the branch was.
    pub head: Oid,
    /// The commits, in the order they go on: the oldest first.
    pub commits: Vec<Oid>,
    /// What the menus named the commits by: a branch, a short hash, or how many.
    pub name: String,
    /// `-x`: add a "(cherry picked from commit …)" line to each message.
    pub record_origin: bool,
    /// Stash the uncommitted changes first, and put them back afterwards.
    pub stash: bool,
}

/// The cherry-pick's own command.
pub fn command(pick: &CherryPick) -> Vec<String> {
    let mut args = vec!["cherry-pick".to_owned()];
    if pick.record_origin {
        args.push("-x".into());
    }
    args.extend(pick.commits.iter().map(|c| c.to_hex()));
    args
}

/// The commands a cherry-pick runs, in order: with *Stash changes*, the stash around it.
pub fn commands(pick: &CherryPick) -> Vec<Vec<String>> {
    let words = |w: &[&str]| w.iter().map(|w| (*w).to_owned()).collect::<Vec<String>>();
    if pick.stash {
        vec![
            words(&["stash", "push", "-m", STASH_MESSAGE]),
            command(pick),
            words(&["stash", "pop"]),
        ]
    } else {
        vec![command(pick)]
    }
}

/// The branch the graph's *Cherry-pick* would put `commit`'s commits on, when there's one to
/// put them on: the open worktree's branch, when `commit` has commits it lacks. Offered while
/// the open worktree is stuck too, to be greyed out.
pub fn offered<'a>(repo: &Repo, catalog: &'a Catalog, commit: Oid) -> Option<&'a str> {
    offered_chosen(repo, catalog, &[commit])
}

/// The branch the log's *Cherry-pick* would put `commits` on: the open worktree's branch, when
/// one of them isn't on it yet.
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

/// Why a listed commit is left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skipped {
    /// The branch has it already.
    OnBranch,
    /// Its change is already on the branch.
    AlreadyThere,
    /// A merge, which `git cherry-pick` takes only with `-m`.
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

/// What a cherry-pick would do, for its confirmation.
#[derive(Clone, Debug)]
pub struct Preview {
    pub branch: String,
    pub head: Oid,
    /// The commits listed, newest first as the log lists them; they go on from the bottom up.
    pub listed: Vec<Oid>,
    skipped: HashMap<Oid, Skipped>,
    dropped: HashSet<Oid>,
    /// What the graph's menu named the commits by, if it was the graph's.
    pub name: Option<String>,
    /// Uncommitted changes to tracked files.
    pub dirty: bool,
}

impl Preview {
    /// The cherry-pick of `picks` onto the open worktree's branch; `name` is what the graph's
    /// menu named them by.
    pub fn load(path: &Path, picks: &Picks, name: Option<String>) -> Result<Preview, Error> {
        let catalog = Catalog::load(path)?;
        if let Some(stuck) = catalog.stuck() {
            return Err(Error::Invalid(format!("{}.", stuck.reason())));
        }
        let (Some(branch), Some(head)) = (catalog.current.clone(), catalog.head) else {
            return Err(Error::Invalid(
                "There's no branch checked out to cherry-pick onto.".into(),
            ));
        };
        let git = Git::new(&catalog.root);
        let h = head.to_hex();
        let oids = |out: &str| -> Vec<Oid> { out.lines().filter_map(Oid::from_hex).collect() };
        let hexes = |commits: &[Oid]| -> String {
            commits
                .iter()
                .map(|c| format!("{}\n", c.to_hex()))
                .collect()
        };
        let listed = match picks {
            Picks::Lacking(x) => {
                oids(&git.run(&["rev-list", "--topo-order", &x.to_hex(), &format!("^{h}")])?)
            }
            Picks::Chosen(commits) => commits.clone(),
        };
        let mut skipped = HashMap::new();
        // On the branch: not among what the listed commits reach and it doesn't.
        let not_on_branch: HashSet<Oid> = oids(&git.run_with_input(
            &["rev-list", "--stdin"],
            format!("{}^{h}\n", hexes(&listed)),
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
            hexes(&listed),
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
            dirty: !git
                .run(&["status", "--porcelain", "--untracked-files=no"])?
                .trim()
                .is_empty(),
        })
    }

    /// Why `commit` is left out, if it is.
    pub fn skipped(&self, commit: Oid) -> Option<Skipped> {
        self.skipped.get(&commit).copied()
    }

    /// Whether `commit` is picked: `None` when it's left out anyway.
    pub fn picked(&self, commit: Oid) -> Option<bool> {
        (self.listed.contains(&commit) && self.skipped(commit).is_none())
            .then(|| !self.dropped.contains(&commit))
    }

    /// Picks `commit`, or drops it, if it isn't left out anyway.
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

    /// The commits picked, in the order they go on: the oldest first.
    pub fn commits(&self) -> Vec<Oid> {
        self.listed
            .iter()
            .rev()
            .copied()
            .filter(|&c| self.picked(c) == Some(true))
            .collect()
    }

    /// Why the cherry-pick can't start.
    pub fn blocked(&self) -> Option<&'static str> {
        self.commits()
            .is_empty()
            .then_some("Pick at least one commit.")
    }

    /// The cherry-pick, with `-x` or not, and the changes stashed or not.
    pub fn cherry_pick(&self, record_origin: bool, stash: bool) -> CherryPick {
        let commits = self.commits();
        let name = match (&self.name, commits.as_slice()) {
            (Some(name), _) => name.clone(),
            (None, [one]) => one.to_hex()[..7].to_owned(),
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
    let theirs: String = commits
        .iter()
        .map(|c| format!("{}\n", c.to_hex()))
        .collect();
    let mut ours = format!("{}\n", head.to_hex());
    for c in commits {
        ours.push_str(&format!("^{}\n", c.to_hex()));
    }
    let mut text = patches(&["--no-walk=unsorted"], theirs)?;
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

fn stash_count(git: &Git) -> usize {
    git.run(&["stash", "list"])
        .map(|out| out.lines().count())
        .unwrap_or(0)
}

/// Runs the cherry-pick if the branch is still where it was. A stop on conflicts, or changes
/// the stash couldn't put back, come back as [`Report::attention`].
pub(crate) fn execute(
    catalog: &Catalog,
    pick: &CherryPick,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<(), Error> {
    if catalog.current.as_deref() != Some(pick.branch.as_str()) || catalog.head != Some(pick.head) {
        return Err(Error::Invalid(format!(
            "Branch {} moved, or is no longer checked out here. Reload and try again.",
            pick.branch
        )));
    }
    if let Some(stuck) = catalog.stuck() {
        return Err(Error::Invalid(format!("{}.", stuck.reason())));
    }
    if pick.commits.is_empty() {
        return Err(Error::Invalid("There's nothing to cherry-pick.".into()));
    }
    let git = Git::new(&catalog.root);
    let mut steps = commands(pick).into_iter();
    // Only a stash entry made here is put back: with nothing to stash, git makes none.
    let stashed = if pick.stash {
        let before = stash_count(&git);
        if !run(&git, steps.next().unwrap(), cancel, report)? {
            return Err(Error::Failed(report.steps.last().unwrap().output.clone()));
        }
        stash_count(&git) > before
    } else {
        false
    };
    let ok = run(&git, steps.next().unwrap(), cancel, report)?;
    let picked = report.steps.last().unwrap().output.clone();
    let after = Catalog::load(&catalog.root)?;
    if let Some(Stuck::InProgress(_)) = after.stuck() {
        let n = after.conflicted.len();
        let mut message = "Finish or abort it with git, or go to another worktree.".to_owned();
        if stashed {
            message.push_str(
                " Your changes are kept in stash@{0}: put them back with git stash pop once \
                 it's done.",
            );
        }
        report.attention = Some(Attention {
            title: if n == 0 {
                "Cherry-pick stopped".into()
            } else {
                format!("Cherry-pick stopped on conflicts in {}", plural(n, "file"))
            },
            message,
        });
        return Ok(());
    }
    let popped = if stashed {
        run(&git, steps.next().unwrap(), cancel, report)?
    } else {
        true
    };
    if !ok {
        return Err(Error::Failed(if popped {
            picked
        } else {
            format!("{picked}\nYour changes are kept in a stash entry: git stash pop.")
        }));
    }
    if !popped {
        // Git applied what it could, with conflict markers, and kept the stash entry too.
        let after = Catalog::load(&catalog.root)?;
        let message = match after.conflicted.len() {
            0 => "Git couldn't put your changes back, and kept them in a stash entry. \
                  Recover them with git stash pop."
                .to_owned(),
            n => format!(
                "Putting your changes back conflicted in {}. Resolve {} with git; your \
                 changes are also kept in a stash entry until you drop it.",
                plural(n, "file"),
                if n == 1 { "it" } else { "them" }
            ),
        };
        report.attention = Some(Attention {
            title: format!("Cherry-picked {} onto {}", pick.name, pick.branch),
            message,
        });
    }
    Ok(())
}

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}
