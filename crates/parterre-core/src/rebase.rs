//! Rebasing is plain `git rebase <target>`, so the user's configuration applies: no
//! `--rebase-merges`, so merges are flattened, and git drops commits whose change is already in
//! the target. Commits the user squashes or drops make it `git rebase -i`, with the todo list
//! written for git.

use std::collections::HashSet;
use std::path::Path;

use parterre_util::CancelTree;

use crate::branches::{Catalog, Error, Report, Stuck};
use crate::git::Git;
use crate::log::is_ancestor;
use crate::operation::{
    Edited, has_tracked_changes, in_progress, kept_stash, refuse_if_moved_or_stuck,
    refuse_if_stuck, run, run_with_edited, stash_count, stopped,
};
use crate::{Oid, Repo};

/// A rebase the user agreed to. It runs only while the branch is still where it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rebase {
    pub branch: String,
    /// Where the branch was.
    pub head: Oid,
    pub onto: Oid,
    /// What the command names the target by: a branch's name, or the full hash.
    pub target: String,
    /// `--autostash` (`Some(true)`) or `--no-autostash` (`Some(false)`), where it differs from
    /// what `rebase.autoStash` does.
    pub autostash: Option<bool>,
    /// The todo list of an interactive rebase, in the order git replays the commits; empty
    /// when every commit is picked, for a plain rebase.
    pub todo: Vec<(Oid, Todo)>,
}

/// What an interactive rebase does with a commit: git's todo commands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Todo {
    #[default]
    Pick,
    /// Melds it into the commit replayed before it, keeping both messages.
    Squash,
    Drop,
}

impl Todo {
    pub const ALL: [Todo; 3] = [Todo::Pick, Todo::Squash, Todo::Drop];

    pub fn word(self) -> &'static str {
        match self {
            Todo::Pick => "pick",
            Todo::Squash => "squash",
            Todo::Drop => "drop",
        }
    }
}

/// What the rebase does with the commits, given what was `chosen` for them, in git's order: a
/// squash with no commit kept before it, which git refuses, is a pick until there is one again.
fn effective(chosen: &[(Oid, Todo)]) -> Vec<(Oid, Todo)> {
    let mut kept = false;
    chosen
        .iter()
        .map(|&(oid, todo)| {
            let todo = match todo {
                Todo::Squash if !kept => Todo::Pick,
                todo => todo,
            };
            kept |= todo != Todo::Drop;
            (oid, todo)
        })
        .collect()
}

pub fn todo_list(todo: &[(Oid, Todo)]) -> String {
    todo.iter()
        .map(|(oid, t)| format!("{} {oid}\n", t.word()))
        .collect()
}

pub fn command(rebase: &Rebase) -> Vec<String> {
    let mut args = vec!["rebase".to_owned()];
    if !rebase.todo.is_empty() {
        args.push("--interactive".into());
    }
    match rebase.autostash {
        Some(true) => args.push("--autostash".into()),
        Some(false) => args.push("--no-autostash".into()),
        None => {}
    }
    args.push(rebase.target.clone());
    args
}

/// The branch a rebase onto `onto` would replay, when parterre offers one: the open worktree's
/// branch, while no operation is in progress there, with commits of its own that `onto` lacks
/// while `onto` has commits it lacks. Otherwise the rebase would only fast-forward, or have
/// nothing to do.
pub fn offered<'a>(repo: &Repo, catalog: &'a Catalog, onto: Oid) -> Option<&'a str> {
    if !catalog.has_working_tree || catalog.stuck().is_some() {
        return None;
    }
    let branch = catalog.current.as_deref()?;
    let head = repo.lookup(&catalog.head?)?;
    let onto = repo.lookup(&onto)?;
    (!is_ancestor(repo, onto, head) && !is_ancestor(repo, head, onto)).then_some(branch)
}

/// Why git leaves one of the branch's commits out of a rebase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skipped {
    /// Its change is already in the target.
    AlreadyThere,
    /// A merge, flattened.
    Merge,
}

impl Skipped {
    pub fn reason(self) -> &'static str {
        match self {
            Skipped::AlreadyThere => "Already in the target: the rebase drops it",
            Skipped::Merge => "A merge: the rebase flattens it",
        }
    }
}

/// What a rebase onto a commit would do, for its confirmation.
#[derive(Clone, Debug)]
pub struct Preview {
    pub branch: String,
    pub head: Oid,
    pub onto: Oid,
    /// The branch's commits whose change the target already has.
    already_there: HashSet<Oid>,
    /// The branch's merges since the target.
    merges: HashSet<Oid>,
    /// The commits git replays, in its order (the oldest first), and what the user chose to do
    /// with each (see [`effective`]).
    replayed: Vec<(Oid, Todo)>,
    /// Uncommitted changes to tracked files, which a rebase refuses to start with.
    pub dirty: bool,
    /// `rebase.autoStash` is set.
    pub auto_stash: bool,
}

impl Preview {
    pub fn load(path: &Path, onto: Oid) -> Result<Preview, Error> {
        let catalog = Catalog::load(path)?;
        refuse_if_stuck(&catalog)?;
        let (Some(branch), Some(head)) = (catalog.current.clone(), catalog.head) else {
            return Err(Error::Invalid(
                "There's no branch checked out to rebase.".into(),
            ));
        };
        let git = Git::new(&catalog.root);
        let oids = |out: String, prefix: &str| -> HashSet<Oid> {
            out.lines()
                .filter_map(|l| l.strip_prefix(prefix))
                .filter_map(Oid::from_hex)
                .collect()
        };
        let range = format!("{onto}...{head}");
        let marks = git.run(&[
            "rev-list",
            "--right-only",
            "--cherry-mark",
            "--no-merges",
            &range,
        ])?;
        let merges = git.run(&["rev-list", "--merges", &format!("{onto}..{head}")])?;
        // The commits `git rebase -i` lists, in its order.
        let replayed = git.run(&[
            "rev-list",
            "--reverse",
            "--topo-order",
            "--right-only",
            "--cherry-pick",
            "--no-merges",
            &range,
        ])?;
        let dirty = has_tracked_changes(&git)?;
        let auto_stash = git
            .query(&["config", "--bool", "rebase.autoStash"])
            .ok()
            .flatten()
            .is_some_and(|v| v.trim() == "true");
        Ok(Preview {
            branch,
            head,
            onto,
            already_there: oids(marks, "="),
            merges: oids(merges, ""),
            replayed: replayed
                .lines()
                .filter_map(Oid::from_hex)
                .map(|oid| (oid, Todo::Pick))
                .collect(),
            dirty,
            auto_stash,
        })
    }

    pub fn skipped(&self, commit: Oid) -> Option<Skipped> {
        if self.merges.contains(&commit) {
            Some(Skipped::Merge)
        } else if self.already_there.contains(&commit) {
            Some(Skipped::AlreadyThere)
        } else {
            None
        }
    }

    /// What the rebase does with `commit`, if git replays it.
    pub fn todo(&self, commit: Oid) -> Option<Todo> {
        effective(&self.replayed)
            .into_iter()
            .find(|(oid, _)| *oid == commit)
            .map(|(_, t)| t)
    }

    /// Has the rebase do `todo` with `commit`, if git replays it.
    pub fn set_todo(&mut self, commit: Oid, todo: Todo) {
        if let Some(entry) = self.replayed.iter_mut().find(|(oid, _)| *oid == commit) {
            entry.1 = todo;
        }
    }

    /// Whether squashing `commits` squashes every one of them that git replays, and at least
    /// one: each has a commit kept before it.
    pub fn can_squash(&self, commits: &[Oid]) -> bool {
        let mut chosen = self.replayed.clone();
        let mut any = false;
        for entry in &mut chosen {
            if commits.contains(&entry.0) {
                entry.1 = Todo::Squash;
                any = true;
            }
        }
        any && effective(&chosen)
            .iter()
            .all(|(oid, t)| *t == Todo::Squash || !commits.contains(oid))
    }

    /// Why the rebase can't start as chosen.
    pub fn blocked(&self, stash: bool) -> Option<&'static str> {
        (self.dirty && !stash).then_some("Commit or stash your changes first.")
    }

    pub fn rebase(&self, target: String, stash: bool) -> Rebase {
        let autostash = match (self.dirty, stash, self.auto_stash) {
            (true, true, false) => Some(true),
            (true, false, true) => Some(false),
            _ => None,
        };
        let todo = effective(&self.replayed);
        let every_commit_picked = todo.iter().all(|(_, t)| *t == Todo::Pick);
        Rebase {
            branch: self.branch.clone(),
            head: self.head,
            onto: self.onto,
            target,
            autostash,
            todo: if every_commit_picked {
                Vec::new()
            } else {
                todo
            },
        }
    }
}

/// Runs the rebase if the branch is still where it was. A stop on conflicts, or changes the
/// autostash couldn't put back, come back as [`Report::attention`].
pub(crate) fn execute(
    catalog: &Catalog,
    rebase: &Rebase,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<(), Error> {
    refuse_if_moved_or_stuck(catalog, &rebase.branch, rebase.head)?;
    let git = Git::new(&catalog.root);
    let stashed_before = stash_count(&git);
    let rebased = if rebase.todo.is_empty() {
        run(&git, command(rebase), cancel, report)?
    } else {
        let todo = todo_list(&rebase.todo);
        run_with_edited(
            &git,
            command(rebase),
            Edited::TodoList,
            &todo,
            cancel,
            report,
        )?
    };
    let after = Catalog::load(&catalog.root)?;
    if let Some(Stuck::InProgress(_)) = after.stuck() {
        report.attention = Some(in_progress(stopped("Rebase", after.conflicted.len())));
        return Ok(());
    }
    if !rebased {
        return Err(Error::Failed(report.last_output()));
    }
    if stash_count(&git) > stashed_before {
        let title = format!("Rebased {} onto {}", rebase.branch, short_target(rebase));
        report.attention = Some(kept_stash(title, after.conflicted.len()));
    }
    Ok(())
}

pub fn short_target(rebase: &Rebase) -> String {
    crate::operation::short_target(&rebase.target, rebase.onto)
}
