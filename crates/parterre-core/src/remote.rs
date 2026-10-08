//! Keeping local branches and their remotes in step (#181): fetching every remote, pulling the
//! open worktree's branch, pushing a local branch to its own name on a remote, with a
//! lease-guarded force push, and setting a branch's upstream.
//!
//! The network commands run with `--progress`, their output streamed to a [`Live`] for the
//! dialog that shows it. Parterre asks git before each step and never parses what it prints.

use std::io::Read;
use std::sync::{Arc, Mutex, PoisonError};

use parterre_util::CancelTree;

use crate::branches::{
    Attention, Catalog, Deletion, Error, LocalBranch, Report, Step, Stuck, Warning, all_commits,
    lost_commits, same_losses,
};
use crate::git::{Git, GitError};
use crate::{Oid, Repo};

/// Said after a network command fails: parterre has no prompt of its own yet.
pub const NO_PROMPT: &str = "Parterre can't ask for passwords or passphrases: git needs a \
                             credential helper or ssh-agent.";

/// How a pull reconciles a branch that has diverged from its upstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reconcile {
    Merge,
    Rebase,
}

impl Reconcile {
    pub const ALL: [Reconcile; 2] = [Reconcile::Merge, Reconcile::Rebase];

    pub fn name(self) -> &'static str {
        match self {
            Reconcile::Merge => "Merge",
            Reconcile::Rebase => "Rebase",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Reconcile::Merge => "--no-rebase",
            Reconcile::Rebase => "--rebase",
        }
    }
}

/// `git pull` on the open worktree's branch, at the commit it was offered at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pull {
    pub branch: String,
    pub head: Oid,
    /// Chosen by the user, when the branch had diverged and git's config didn't say.
    pub how: Option<Reconcile>,
}

/// A pull that needs the user to say how: the branch and its upstream have diverged, and
/// neither `pull.rebase`, `pull.ff` nor the branch's `rebase` is set. Nothing has changed but
/// the fetch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diverged {
    pub pull: Pull,
    /// The upstream's short name, `origin/main`.
    pub upstream: String,
    pub ahead: usize,
    pub behind: usize,
}

impl Diverged {
    /// The pull `how` runs.
    pub fn pull(&self, how: Reconcile) -> Pull {
        Pull {
            how: Some(how),
            ..self.pull.clone()
        }
    }

    pub fn command(&self, how: Reconcile) -> Vec<String> {
        pull_command(Some(how))
    }
}

/// Pushes local branch `branch`, at `tip`, to branch `to` on `remote`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Push {
    pub branch: String,
    pub tip: Oid,
    pub remote: String,
    pub to: String,
}

impl Push {
    /// Pushing `branch` to `remote` as `git push` does: to its upstream on the upstream's
    /// remote, to its own name on any other.
    pub fn new(branch: &LocalBranch, remote: &str) -> Push {
        let to = branch
            .upstream_remote
            .as_ref()
            .filter(|(r, _)| r == remote)
            .map_or(&branch.name, |(_, b)| b);
        Push {
            branch: branch.name.clone(),
            tip: branch.tip,
            remote: remote.to_owned(),
            to: to.to_owned(),
        }
    }

    /// Where it goes: `origin`, or `origin/main` for a branch of another name.
    pub fn target(&self) -> String {
        if self.to == self.branch {
            self.remote.clone()
        } else {
            self.remote_branch()
        }
    }

    /// The branch it goes to, as its remote-tracking branch is called: `origin/main`.
    pub fn remote_branch(&self) -> String {
        format!("{}/{}", self.remote, self.to)
    }

    /// That remote-tracking branch in full: `refs/remotes/origin/main`.
    fn tracking(&self) -> String {
        format!("refs/remotes/{}", self.remote_branch())
    }
}

/// `git branch --set-upstream-to`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetUpstream {
    pub branch: String,
    /// A remote-tracking branch, `origin/main`.
    pub upstream: String,
}

/// A branch on a remote, at the tip its remote-tracking branch had when offered: `origin/topic`
/// is branch `topic` of remote `origin`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteBranchTip {
    pub remote: String,
    pub branch: String,
    pub tip: Oid,
}

impl RemoteBranchTip {
    /// `origin/topic`.
    pub fn name(&self) -> String {
        format!("{}/{}", self.remote, self.branch)
    }
}

/// What pushing a branch to a remote would do, by the last fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushState {
    /// The remote has no branch of that name: the push creates it.
    New,
    /// The remote's branch is behind: a fast-forward.
    Ahead,
    /// Nothing to send: the same commit, or the remote's branch has it already.
    UpToDate,
    /// The remote's branch has commits the branch lacks: only a force push sends it.
    Force,
}

fn words(s: &[&str]) -> Vec<String> {
    s.iter().map(|s| (*s).to_owned()).collect()
}

pub(crate) fn fetch_command() -> Vec<String> {
    words(&["fetch", "--progress", "--all", "--prune"])
}

fn fetch_remote_command(remote: &str) -> Vec<String> {
    words(&["fetch", "--progress", "--prune", remote])
}

pub(crate) fn pull_command(how: Option<Reconcile>) -> Vec<String> {
    let mut args = words(&["pull", "--progress"]);
    args.extend(how.map(|h| h.flag().to_owned()));
    args
}

/// `git push`, with `-u` only when the branch has no upstream at all, so an upstream not pushed
/// yet is left as it is. `force` is the remote's tip it replaces: a lease on the branch's own
/// name takes `--force-if-includes`, but git checks that against the reflog of the local branch
/// named like the remote's, so a push to another name leases that tip instead.
pub(crate) fn push_command(catalog: &Catalog, push: &Push, force: Option<Oid>) -> Vec<String> {
    let mut args = words(&["push", "--progress"]);
    let has_upstream = catalog
        .locals
        .iter()
        .any(|b| b.name == push.branch && b.upstream.is_some());
    if !has_upstream {
        args.push("-u".into());
    }
    match force {
        Some(_) if push.to == push.branch => {
            args.push(format!("--force-with-lease=refs/heads/{}", push.to));
            args.push("--force-if-includes".into());
        }
        Some(theirs) => {
            args.push(format!(
                "--force-with-lease=refs/heads/{}:{}",
                push.to,
                theirs.to_hex()
            ));
        }
        None => {}
    }
    args.push(push.remote.clone());
    if push.to == push.branch {
        args.push(push.branch.clone());
    } else {
        args.push(format!("{}:{}", push.branch, push.to));
    }
    args
}

/// `git branch --set-upstream-to=…`.
pub fn set_upstream_command(set: &SetUpstream) -> Vec<String> {
    vec![
        "branch".into(),
        format!("--set-upstream-to=refs/remotes/{}", set.upstream),
        "--".into(),
        set.branch.clone(),
    ]
}

/// Deletes the branch on its remote, only if it is still where the last fetch saw it.
pub(crate) fn delete_remote_command(b: &RemoteBranchTip) -> Vec<String> {
    vec![
        "push".into(),
        "--progress".into(),
        format!(
            "--force-with-lease=refs/heads/{}:{}",
            b.branch,
            b.tip.to_hex()
        ),
        b.remote.clone(),
        "--delete".into(),
        b.branch.clone(),
    ]
}

/// Pushing local branch `branch` to each remote, by name, with what each would do by the
/// commits in `repo`. Empty if there is no such branch.
pub fn push_targets(repo: &Repo, catalog: &Catalog, branch: &str) -> Vec<(Push, PushState)> {
    let tip = |full: &str| {
        repo.refs
            .iter()
            .find(|r| r.full_name == full)
            .map(|r| r.target)
    };
    let (Some(local), Some(branch)) = (
        tip(&format!("refs/heads/{branch}")),
        catalog.locals.iter().find(|b| b.name == branch),
    ) else {
        return Vec::new();
    };
    let mut remotes = catalog.remote_names.clone();
    remotes.sort();
    remotes
        .into_iter()
        .map(|remote| {
            let push = Push::new(branch, &remote);
            let state = match tip(&push.tracking()) {
                None => PushState::New,
                Some(theirs) if repo.reaches(theirs, local) => PushState::UpToDate,
                Some(theirs) if repo.reaches(local, theirs) => PushState::Ahead,
                Some(_) => PushState::Force,
            };
            (push, state)
        })
        .collect()
}

/// The open worktree's branch, when it has an upstream and `commit` is where it or that
/// upstream is: what *Pull* pulls (#356).
pub fn pull_offered(catalog: &Catalog, commit: Oid) -> Option<&LocalBranch> {
    if !catalog.has_working_tree {
        return None;
    }
    let current = catalog.current.as_deref()?;
    let branch = catalog.locals.iter().find(|b| b.name == current)?;
    let upstream = branch.upstream.as_deref()?;
    let at_upstream = || {
        catalog
            .remotes
            .iter()
            .any(|r| r.name == upstream && r.tip == commit)
    };
    (catalog.head == Some(commit) || at_upstream()).then_some(branch)
}

/// `git fetch --all --prune`.
pub(crate) fn fetch(
    catalog: &Catalog,
    cancel: &CancelTree,
    report: &mut Report,
    live: Option<&Live>,
) -> Result<(), Error> {
    if catalog.remote_names.is_empty() {
        return Err(Error::Invalid("This repository has no remote.".into()));
    }
    let git = Git::new(&catalog.root);
    if !run_live(&git, fetch_command(), cancel, report, live)? {
        return Err(network_failure(report));
    }
    Ok(())
}

/// Fetches the upstream's remote, then pulls, unless the branch has diverged and neither git's
/// config nor the user says how to reconcile it: then the user is asked, with nothing changed
/// but the fetch.
/// A stop on conflicts comes back as [`Report::attention`].
pub(crate) fn pull(
    catalog: &Catalog,
    pull: &Pull,
    cancel: &CancelTree,
    report: &mut Report,
    live: Option<&Live>,
) -> Result<Option<Diverged>, Error> {
    let branch = pull.branch.as_str();
    if !catalog.has_working_tree
        || catalog.current.as_deref() != Some(branch)
        || catalog.head != Some(pull.head)
    {
        return Err(Error::Invalid(format!(
            "Branch {branch} moved, or is no longer checked out here. Reload and try again."
        )));
    }
    if let Some(stuck) = catalog.stuck() {
        return Err(Error::Invalid(format!("{}.", stuck.reason())));
    }
    let Some(upstream) = catalog
        .locals
        .iter()
        .find(|b| b.name == branch)
        .and_then(|b| b.upstream.clone())
    else {
        return Err(Error::Invalid(format!("Branch {branch} has no upstream.")));
    };
    let git = Git::new(&catalog.root);
    // Fetched here, so that a failure to reach the remote says what parterre can't do.
    let remote = git.query(&["config", "--get", &format!("branch.{branch}.remote")])?;
    if let Some(remote) = remote.filter(|r| catalog.remote_names.contains(r))
        && !run_live(&git, fetch_remote_command(&remote), cancel, report, live)?
    {
        return Err(network_failure(report));
    }
    if pull.how.is_none() {
        let range = format!("refs/heads/{branch}...{branch}@{{upstream}}");
        let counts = git.query(&["rev-list", "--left-right", "--count", &range, "--"])?;
        if let Some((ahead, behind)) = counts.as_deref().and_then(parse_counts) {
            if behind == 0 {
                return Ok(None);
            }
            if ahead > 0 && !reconcile_configured(&git, branch)? {
                return Ok(Some(Diverged {
                    pull: pull.clone(),
                    upstream,
                    ahead,
                    behind,
                }));
            }
        }
    }
    let ok = run_live(&git, pull_command(pull.how), cancel, report, live)?;
    let after = Catalog::load(&catalog.root)?;
    if let Some(stuck) = after.stuck() {
        let n = after.conflicted.len();
        let title = match (stuck, n) {
            (_, 1..) => format!("Pull stopped on conflicts in {}", plural(n, "file")),
            (Stuck::InProgress(_), 0) => "Pull stopped".to_owned(),
            (Stuck::Conflicts, 0) => unreachable!("conflicts without conflicted files"),
        };
        report.attention = Some(Attention {
            title,
            message: "Finish or abort it with git, or go to another worktree.".into(),
        });
        return Ok(None);
    }
    if !ok {
        return Err(Error::Failed(last_output(report)));
    }
    Ok(None)
}

/// Pushes the branch to the remote. One that would replace the remote's commits asks first
/// ([`Warning`]: a confirmation when the branch has a copy of each, else a warning with the
/// commits lost), then forces with a lease. A push the remote rejects is followed by a fetch,
/// to say whether the remote has commits the branch lacks.
pub(crate) fn push(
    catalog: &Catalog,
    action: &crate::branches::Action,
    push: &Push,
    approval: Option<&Warning>,
    cancel: &CancelTree,
    report: &mut Report,
    live: Option<&Live>,
) -> Result<Option<Warning>, Error> {
    let branch = push.branch.as_str();
    if !catalog
        .locals
        .iter()
        .any(|b| b.name == branch && b.tip == push.tip)
    {
        return Err(Error::Invalid(format!(
            "Branch {branch} moved or no longer exists. Reload and try again."
        )));
    }
    if !catalog.remote_names.contains(&push.remote) {
        return Err(Error::Invalid(format!(
            "There is no remote {}. Reload and try again.",
            push.remote
        )));
    }
    let git = Git::new(&catalog.root);
    let tracking = push.tracking();
    let theirs_name = push.remote_branch();
    let theirs = remote_tip(&git, &tracking)?;
    if let Some(theirs) = theirs
        && is_ancestor(&git, push.tip, theirs)?
    {
        return Err(Error::Invalid(format!(
            "{theirs_name} has every commit of {branch}: there's nothing to push."
        )));
    }
    let force = match theirs {
        Some(theirs) if !is_ancestor(&git, theirs, push.tip)? => Some(theirs),
        _ => None,
    };
    if force.is_some() {
        let (lost, replaced) = lost_and_replaced(&git, branch, &tracking)?;
        let approved =
            approval.is_some_and(|w| w.action == *action && w.head == theirs && w.commits == lost);
        if !approved {
            let repo = Arc::new(git.load()?);
            return Ok(Some(Warning {
                action: action.clone(),
                commits: lost,
                replaced,
                deletions: Vec::new(),
                repo: Some(repo),
                changed: false,
                commands: vec![push_command(catalog, push, force)],
                head: theirs,
            }));
        }
    }
    // What `--force-if-includes` would refuse, for a push to another name.
    if let Some(theirs) = force
        && push.to != push.branch
        && !ever_had(&git, branch, theirs)?
    {
        return Err(Error::Failed(format!(
            "{theirs_name} has commits {branch} never had, so parterre won't force push over \
             them. Take them into {branch} first."
        )));
    }
    if run_live(
        &git,
        push_command(catalog, push, force),
        cancel,
        report,
        live,
    )? {
        return Ok(None);
    }
    let pushed = last_output(report);
    if !run_live(
        &git,
        fetch_remote_command(&push.remote),
        cancel,
        report,
        live,
    )? {
        return Err(Error::Failed(format!("{pushed}\n\n{NO_PROMPT}")));
    }
    match remote_tip(&git, &tracking)? {
        // The lease held, so it was `--force-if-includes`: the remote's commits were fetched,
        // but never on the branch.
        now if force.is_some() && push.to == push.branch && now == theirs => {
            Err(Error::Failed(format!(
                "{theirs_name} has commits {branch} never had, so git won't force push over \
                 them. Take them into {branch} first.\n\n{pushed}"
            )))
        }
        Some(now) if !is_ancestor(&git, now, push.tip)? => Err(Error::Failed(format!(
            "{theirs_name} has commits you don't have."
        ))),
        _ => Err(Error::Failed(pushed)),
    }
}

/// Deletes branches on their remotes, one by one. It always asks first ([`Warning`]): a
/// confirmation when the commits are reached from elsewhere, else a warning listing those only
/// they reach. A branch the remote moved since the last fetch is kept, and a fetch suggested;
/// one already gone there is as asked, and comes back as [`Report::attention`].
pub(crate) fn delete_remote_branches(
    catalog: &Catalog,
    action: &crate::branches::Action,
    branches: &[RemoteBranchTip],
    approval: Option<&Warning>,
    cancel: &CancelTree,
    report: &mut Report,
    live: Option<&Live>,
) -> Result<Option<Warning>, Error> {
    if branches.is_empty() {
        return Err(Error::Invalid("Choose a remote branch to delete.".into()));
    }
    for (i, b) in branches.iter().enumerate() {
        let name = b.name();
        if branches[..i].contains(b) {
            return Err(Error::Invalid(format!(
                "Remote branch {name} is listed twice."
            )));
        }
        if !catalog.remote_names.contains(&b.remote)
            || !catalog
                .remotes
                .iter()
                .any(|r| r.name == name && r.tip == b.tip)
        {
            return Err(Error::Invalid(format!(
                "Remote branch {name} moved or is gone since. Reload and try again."
            )));
        }
    }
    let git = Git::new(&catalog.root);
    // A remote's `HEAD` naming one of them goes with it.
    let excluded: Vec<String> = branches
        .iter()
        .flat_map(|b| {
            let name = b.name();
            let head = catalog
                .remote_defaults
                .contains(&name)
                .then(|| format!("refs/remotes/{}/HEAD", b.remote));
            std::iter::once(format!("refs/remotes/{name}")).chain(head)
        })
        .collect();
    let deletions = branches
        .iter()
        .map(|b| {
            Ok(Deletion {
                name: b.name(),
                path: None,
                commits: lost_commits(&git, catalog, b.tip, &excluded, &[], None)?,
                files: Vec::new(),
                refusal: None,
                branch: None,
                head: Some(b.tip),
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let commits = all_commits(&deletions);
    let approved = approval.is_some_and(|w| {
        w.action == *action && w.commits == commits && same_losses(&w.deletions, &deletions)
    });
    if !approved {
        return Ok(Some(Warning {
            action: action.clone(),
            commits,
            replaced: Vec::new(),
            deletions,
            repo: Some(Arc::new(git.load()?)),
            changed: false,
            commands: branches.iter().map(delete_remote_command).collect(),
            head: None,
        }));
    }
    let (mut deleted, mut gone) = (Vec::new(), Vec::new());
    for b in branches {
        if run_live(&git, delete_remote_command(b), cancel, report, live)? {
            deleted.push(b.name());
            continue;
        }
        let pushed = last_output(report);
        let Some(now) = ls_remote(&git, b, cancel)? else {
            return Err(Error::Failed(format!("{pushed}\n\n{NO_PROMPT}")));
        };
        let (name, remote) = (b.name(), &b.remote);
        match now {
            // Someone deleted it first: as asked.
            None => {
                gone.push(name);
                continue;
            }
            Some(tip) if tip == b.tip => {
                let went = went(&deleted, &gone);
                return Err(Error::Failed(format!("{pushed}{went}")));
            }
            Some(_) => {
                report.suggest_fetch = true;
                let went = went(&deleted, &gone);
                return Err(Error::Failed(format!(
                    "{name} moved on {remote} since the last fetch. Fetch, and look again.{went}"
                )));
            }
        }
    }
    if !gone.is_empty() {
        // The graph shows them until a fetch.
        report.suggest_fetch = true;
        let (title, it) = match gone.as_slice() {
            [one] => (format!("{one} was already gone"), "it"),
            many => (format!("{} were already gone", many.join(", ")), "them"),
        };
        report.attention = Some(Attention {
            title,
            message: format!(
                "Someone deleted {it} on the remote first. Fetch to update the graph."
            ),
        });
    }
    Ok(None)
}

/// What a remote deletion that stopped partway did, as sentences after its error.
fn went(deleted: &[String], gone: &[String]) -> String {
    let said = |names: &[String], one: &str, many: &str| match names {
        [] => String::new(),
        [name] => format!(" {name} {one}."),
        names => format!(" {} {many}.", names.join(", ")),
    };
    said(deleted, "was deleted", "were deleted")
        + &said(gone, "was already gone", "were already gone")
}

/// Where the branch is on its remote now (`ls-remote`): `None` if git couldn't ask, `Some(None)`
/// if the remote has no such branch.
fn ls_remote(
    git: &Git,
    b: &RemoteBranchTip,
    cancel: &CancelTree,
) -> Result<Option<Option<Oid>>, Error> {
    let args: Vec<String> = ["ls-remote", &b.remote, &format!("refs/heads/{}", b.branch)]
        .map(str::to_owned)
        .to_vec();
    let mut command = network_command(git, &args);
    let child = cancel.start(&mut command).map_err(|e| match e {
        parterre_util::Start::Cancelled => Error::Cancelled,
        parterre_util::Start::Spawn(e) => GitError::Spawn(e).into(),
    })?;
    let out = child.wait_with_output();
    if cancel.finish() {
        return Err(Error::Cancelled);
    }
    let out = out.map_err(GitError::Spawn)?;
    if !out.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(Some(text.split_whitespace().next().and_then(Oid::from_hex)))
}

/// The remote's commits a force push would replace, split into those with no copy on the
/// branch (lost) and those with one (replaced), as the graph colours them.
fn lost_and_replaced(
    git: &Git,
    branch: &str,
    tracking: &str,
) -> Result<(Vec<Oid>, Vec<Oid>), Error> {
    let repo = git.load()?;
    let full = format!("refs/heads/{branch}");
    let upstreams = crate::upstream::load(git, &repo, &[(full, tracking.to_owned())]);
    let (mut lost, mut replaced) = (Vec::new(), Vec::new());
    for u in &upstreams {
        for &(c, side) in &u.commits {
            let oid = repo.commit(c).oid;
            match side {
                crate::upstream::Side::Ahead => {}
                crate::upstream::Side::Replaced => replaced.push(oid),
                crate::upstream::Side::Behind | crate::upstream::Side::Lost => lost.push(oid),
            }
        }
    }
    lost.sort_by_key(|o| o.to_hex());
    replaced.sort_by_key(|o| o.to_hex());
    Ok((lost, replaced))
}

pub(crate) fn set_upstream(
    catalog: &Catalog,
    set: &SetUpstream,
    cancel: &CancelTree,
    report: &mut Report,
) -> Result<(), Error> {
    if !catalog.locals.iter().any(|b| b.name == set.branch) {
        return Err(Error::Invalid(format!(
            "Branch {} no longer exists. Reload and try again.",
            set.branch
        )));
    }
    if !catalog.remotes.iter().any(|r| r.name == set.upstream) {
        return Err(Error::Invalid(format!(
            "There is no remote-tracking branch {}. Fetch, or reload and try again.",
            set.upstream
        )));
    }
    let git = Git::new(&catalog.root);
    if !run_live(&git, set_upstream_command(set), cancel, report, None)? {
        return Err(Error::Failed(last_output(report)));
    }
    Ok(())
}

fn remote_tip(git: &Git, full: &str) -> Result<Option<Oid>, Error> {
    Ok(git
        .query(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{full}^{{commit}}"),
        ])?
        .and_then(|s| Oid::from_hex(&s)))
}

/// Whether local branch `branch` has `commit`, or had it at any point its reflog recalls.
fn ever_had(git: &Git, branch: &str, commit: Oid) -> Result<bool, Error> {
    let full = format!("refs/heads/{branch}");
    let reflog = git.run(&["rev-list", "--walk-reflogs", &full])?;
    let mut input = format!("{}\n", commit.to_hex());
    for oid in reflog.lines() {
        input.push_str(&format!("^{oid}\n"));
    }
    Ok(git
        .run_with_input(&["rev-list", "-n", "1", "--stdin"], input)?
        .trim()
        .is_empty())
}

fn is_ancestor(git: &Git, ancestor: Oid, of: Oid) -> Result<bool, Error> {
    let (a, b) = (ancestor.to_hex(), of.to_hex());
    Ok(git
        .query(&["merge-base", "--is-ancestor", &a, &b])?
        .is_some())
}

/// Whether git's config says how `git pull` reconciles diverged branches.
fn reconcile_configured(git: &Git, branch: &str) -> Result<bool, Error> {
    for key in [
        "pull.rebase".to_owned(),
        "pull.ff".to_owned(),
        format!("branch.{branch}.rebase"),
    ] {
        if git.query(&["config", "--get", &key])?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `rev-list --left-right --count`'s `ahead<TAB>behind`.
fn parse_counts(out: &str) -> Option<(usize, usize)> {
    let mut parts = out.split_whitespace().map(str::parse::<usize>);
    Some((parts.next()?.ok()?, parts.next()?.ok()?))
}

fn last_output(report: &Report) -> String {
    report
        .steps
        .last()
        .map(|s| s.output.clone())
        .unwrap_or_default()
}

/// A network command's failure, in git's words, and what parterre can't do about it.
fn network_failure(report: &Report) -> Error {
    Error::Failed(format!("{}\n\n{NO_PROMPT}", last_output(report)))
}

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// What git printed so far, each command with its output, as a terminal shows it: a carriage
/// return starts its line again, as git's progress meters write it. Shared with the window
/// showing it while the commands run.
#[derive(Clone, Debug, Default)]
pub struct Live(Arc<Mutex<Vec<Shown>>>);

#[derive(Debug, Default)]
struct Shown {
    command: String,
    text: String,
    /// A carriage return was last: the next character starts the line again.
    restart: bool,
    /// The start of a character split between two reads.
    partial: Vec<u8>,
}

impl Shown {
    fn push(&mut self, bytes: &[u8]) {
        self.partial.extend_from_slice(bytes);
        let bytes = std::mem::take(&mut self.partial);
        let (text, rest) = match std::str::from_utf8(&bytes) {
            Ok(text) => (text.to_owned(), &[][..]),
            Err(e) if e.error_len().is_none() => {
                let (valid, rest) = bytes.split_at(e.valid_up_to());
                (String::from_utf8_lossy(valid).into_owned(), rest)
            }
            Err(_) => (String::from_utf8_lossy(&bytes).into_owned(), &[][..]),
        };
        self.partial = rest.to_vec();
        for ch in text.chars() {
            match ch {
                '\r' => self.restart = true,
                '\n' => {
                    self.restart = false;
                    self.text.push('\n');
                }
                ch => {
                    if std::mem::take(&mut self.restart) {
                        let start = self.text.rfind('\n').map_or(0, |i| i + 1);
                        self.text.truncate(start);
                    }
                    self.text.push(ch);
                }
            }
        }
    }
}

impl Live {
    /// Each command so far, as `git …`, with its output.
    pub fn steps(&self) -> Vec<(String, String)> {
        self.lock()
            .iter()
            .map(|s| (s.command.clone(), s.text.clone()))
            .collect()
    }

    /// A command starts.
    pub fn start(&self, args: &[String]) {
        self.lock().push(Shown {
            command: crate::branches::command_text(args),
            ..Shown::default()
        });
    }

    /// Output of the last command started.
    pub fn push(&self, bytes: &[u8]) {
        if let Some(shown) = self.lock().last_mut() {
            shown.push(bytes);
        }
    }

    /// The last command's output, as shown.
    fn last_text(&self) -> String {
        self.lock()
            .last()
            .map(|s| s.text.clone())
            .unwrap_or_default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Shown>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// An operation's git command that may reach a remote. ssh is kept from asking on a terminal
/// parterre's user can't see: it fails instead, unless the user has an askpass program of
/// their own.
fn network_command(git: &Git, args: &[String]) -> std::process::Command {
    let mut command = git.operation_command(args);
    if std::env::var_os("SSH_ASKPASS").is_none() {
        command
            .env("SSH_ASKPASS", "parterre-has-no-askpass")
            .env("SSH_ASKPASS_REQUIRE", "force");
    }
    command
}

/// Runs a git command as [`crate::branches::run`] does, streaming its output to `live` as it
/// comes.
fn run_live(
    git: &Git,
    args: Vec<String>,
    cancel: &CancelTree,
    report: &mut Report,
    live: Option<&Live>,
) -> Result<bool, Error> {
    let own = Live::default();
    let live = live.unwrap_or(&own);
    live.start(&args);
    let mut command = network_command(git, &args);
    let mut child = cancel.start(&mut command).map_err(|e| match e {
        parterre_util::Start::Cancelled => Error::Cancelled,
        parterre_util::Start::Spawn(e) => GitError::Spawn(e).into(),
    })?;
    let pipe = |mut from: Box<dyn Read + Send>, live: Live| {
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match from.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => live.push(&buf[..n]),
                }
            }
        })
    };
    let stdout = pipe(Box::new(child.stdout.take().unwrap()), live.clone());
    let stderr = pipe(Box::new(child.stderr.take().unwrap()), live.clone());
    let _ = stdout.join();
    let _ = stderr.join();
    let status = child.wait();
    let cancelled = cancel.finish();
    let status = status.map_err(GitError::Spawn)?;
    report.steps.push(Step {
        args,
        output: live.last_text().trim().to_owned(),
        success: status.success(),
    });
    if cancelled {
        return Err(Error::Cancelled);
    }
    Ok(status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_split_between_reads_comes_out_whole() {
        let live = Live::default();
        live.start(&["fetch".to_owned()]);
        let bytes = "Über\n".as_bytes();
        live.push(&bytes[..1]);
        live.push(&bytes[1..]);
        assert_eq!(live.steps()[0].1, "Über\n");
    }

    #[test]
    fn counts_are_read_as_git_prints_them() {
        assert_eq!(parse_counts("2\t3"), Some((2, 3)));
        assert_eq!(parse_counts(""), None);
    }
}
