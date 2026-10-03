//! Rebasing the open worktree's branch onto another commit: which of its commits git would
//! leave out, and running it. It is plain `git rebase <target>`, so the user's configuration
//! applies: no `--rebase-merges`, so merges are flattened, and git drops commits whose change
//! is already in the target. Commits the user squashes or drops make it `git rebase -i`, with
//! the todo list written for git. A rebase that stops on conflicts leaves the worktree with an
//! operation in progress, finished with git for now.

use std::collections::HashSet;
use std::path::Path;

use crate::branches::{Attention, Cancel, Catalog, Error, Report, Stuck, run, run_with};
use crate::git::Git;
use crate::log::is_ancestor;
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

    /// The command as the todo list names it.
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
pub fn effective(chosen: &[(Oid, Todo)]) -> Vec<(Oid, Todo)> {
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

/// The todo list git reads for `todo`, one line per commit.
pub fn todo_list(todo: &[(Oid, Todo)]) -> String {
    todo.iter()
        .map(|(oid, t)| format!("{} {}\n", t.word(), oid.to_hex()))
        .collect()
}

/// The command a rebase runs.
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
        if let Some(stuck) = catalog.stuck() {
            return Err(Error::Invalid(format!("{}.", stuck.reason())));
        }
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
        let range = format!("{}...{}", onto.to_hex(), head.to_hex());
        let marks = git.run(&[
            "rev-list",
            "--right-only",
            "--cherry-mark",
            "--no-merges",
            &range,
        ])?;
        let merges = git.run(&[
            "rev-list",
            "--merges",
            &format!("{}..{}", onto.to_hex(), head.to_hex()),
        ])?;
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
        let dirty = !git
            .run(&["status", "--porcelain", "--untracked-files=no"])?
            .trim()
            .is_empty();
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

    /// Why git leaves `commit` out, if it does.
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

    /// The rebase, with the target named by `target` and the changes stashed or not.
    pub fn rebase(&self, target: String, stash: bool) -> Rebase {
        let autostash = match (self.dirty, stash, self.auto_stash) {
            (true, true, false) => Some(true),
            (true, false, true) => Some(false),
            _ => None,
        };
        Rebase {
            branch: self.branch.clone(),
            head: self.head,
            onto: self.onto,
            target,
            autostash,
            todo: {
                let todo = effective(&self.replayed);
                if todo.iter().all(|(_, t)| *t == Todo::Pick) {
                    Vec::new()
                } else {
                    todo
                }
            },
        }
    }
}

/// Runs the rebase if the branch is still where it was. A stop on conflicts, or changes the
/// autostash couldn't put back, come back as [`Report::attention`].
pub(crate) fn execute(
    catalog: &Catalog,
    rebase: &Rebase,
    cancel: &Cancel,
    report: &mut Report,
) -> Result<(), Error> {
    if catalog.current.as_deref() != Some(rebase.branch.as_str())
        || catalog.head != Some(rebase.head)
    {
        return Err(Error::Invalid(format!(
            "Branch {} moved, or is no longer checked out here. Reload and try again.",
            rebase.branch
        )));
    }
    if let Some(stuck) = catalog.stuck() {
        return Err(Error::Invalid(format!("{}.", stuck.reason())));
    }
    let git = Git::new(&catalog.root);
    let stashes = || {
        git.run(&["stash", "list"])
            .map(|out| out.lines().count())
            .unwrap_or(0)
    };
    let before = stashes();
    let ok = if rebase.todo.is_empty() {
        run(&git, command(rebase), cancel, report)?
    } else {
        // Git asks its sequence editor for the todo list: copy ours over it.
        let dir = git.run(&["rev-parse", "--absolute-git-dir"])?;
        let path = Path::new(dir.trim()).join("parterre-rebase-todo");
        std::fs::write(&path, todo_list(&rebase.todo))
            .map_err(|e| Error::Invalid(format!("Couldn't write the todo list: {e}")))?;
        // Git runs it with sh, Git for Windows' too, which takes forward slashes.
        let quoted = path
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', r"'\''");
        let editor = format!("cp '{quoted}'");
        let ok = run_with(
            &git,
            command(rebase),
            &[("GIT_SEQUENCE_EDITOR", &editor)],
            cancel,
            report,
        );
        let _ = std::fs::remove_file(&path);
        ok?
    };
    let after = Catalog::load(&catalog.root)?;
    if let Some(Stuck::InProgress(_)) = after.stuck() {
        let n = after.conflicted.len();
        report.attention = Some(Attention {
            title: if n == 0 {
                "Rebase stopped".into()
            } else {
                format!("Rebase stopped on conflicts in {}", plural(n, "file"))
            },
            message: "Finish or abort it with git, or go to another worktree.".into(),
        });
        return Ok(());
    }
    if !ok {
        return Err(Error::Failed(report.steps.last().unwrap().output.clone()));
    }
    if stashes() > before {
        // Git applied what it could, with conflict markers, and kept the stash entry too.
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
            title: format!("Rebased {} onto {}", rebase.branch, short_target(rebase)),
            message,
        });
    }
    Ok(())
}

/// The target as the menus name it: a branch, or a short hash.
pub fn short_target(rebase: &Rebase) -> String {
    if rebase.target == rebase.onto.to_hex() {
        rebase.onto.to_hex()[..7].to_owned()
    } else {
        rebase.target.clone()
    }
}

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}
