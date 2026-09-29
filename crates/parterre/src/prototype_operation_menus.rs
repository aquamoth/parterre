//! PROTOTYPE — throwaway. Where operations are offered in the menus, for
//! "Operations in the menus: prototype" (aquamoth/parterre#144). Nothing here runs git to change
//! anything: picking an operation shows a dialog saying what would run, and where.
//!
//! Three variants of the graph's node menu (and the log window's row menu), switched with the
//! bar at the bottom of the main window, or `PARTERRE_MENU_VARIANT=A|B|C`:
//! - A, verbs: *Switch*, *Merge into*, *Rebase onto this*… at the top; each lists its targets.
//! - B, refs: a submenu per branch on the node, and one for the commit; merge, rebase,
//!   cherry-pick and revert open a dialog that picks the target.
//! - C, the label clicked: right-clicking a branch label gives that branch's menu, anywhere
//!   else on the node the commit's; the open worktree is the target by default, other ones
//!   under *Another branch*.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText, Ui};
use parterre_core::git::Git;
use parterre_core::{CommitIx, GitRef, RefKind, Repo};

pub const VARIANTS: [(&str, &str); 3] = [
    ("A", "Verbs first, targets in submenus"),
    ("B", "A submenu per branch, dialogs pick the target"),
    ("C", "The label clicked, open worktree by default"),
];

/// What was under the pointer when the node's menu was opened (variant C).
#[derive(Clone, Debug, Default)]
pub enum Clicked {
    #[default]
    Commit,
    Ref(String),
    Worktree(usize),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Level {
    OneClick,
    Confirm,
    Warning,
}

#[derive(Clone, Debug)]
struct Choice {
    /// Shown when an operation offers several ways, or the dialog picks the target.
    label: String,
    /// Where it runs: `None` for the open worktree.
    worktree: Option<usize>,
    commands: Vec<String>,
    note: Option<String>,
}

#[derive(Clone, Debug)]
struct Op {
    title: String,
    level: Level,
    choices: Vec<Choice>,
    picked: usize,
    /// A name the user types, put in place of `{name}` in the commands.
    name: Option<String>,
    /// Pick the target in a list in the dialog, not in the menu (variant B).
    pick_in_dialog: bool,
}

struct State {
    variant: usize,
    clicked: Clicked,
    pending: Option<Op>,
    upstreams: HashMap<PathBuf, HashMap<String, String>>,
    reaches: HashMap<(usize, u32, u32), bool>,
}

thread_local! {
    /// The commit whose menu is open: targets already on it aren't listed.
    static MENU_COMMIT: std::cell::Cell<Option<CommitIx>> = const { std::cell::Cell::new(None) };
    static STATE: RefCell<State> = RefCell::new(State {
        variant: match std::env::var("PARTERRE_MENU_VARIANT").as_deref() {
            Ok("B" | "b") => 1,
            Ok("C" | "c") => 2,
            _ => 0,
        },
        clicked: Clicked::Commit,
        pending: None,
        upstreams: HashMap::new(),
        reaches: HashMap::new(),
    });
}

fn variant() -> usize {
    STATE.with(|s| s.borrow().variant)
}

pub fn set_clicked(c: Clicked) {
    STATE.with(|s| s.borrow_mut().clicked = c);
}

fn open(op: Op) {
    STATE.with(|s| s.borrow_mut().pending = Some(op));
}

/// Forgets the cached upstreams, so a reload shows new ones.
pub fn forget() {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.upstreams.clear();
        s.reaches.clear();
    });
}

// ---------------------------------------------------------------------------------------------
// The repository, as the menus need it.

/// `from` has `to` in its history (or is it).
fn reaches(repo: &Repo, from: CommitIx, to: CommitIx) -> bool {
    let key = (repo as *const Repo as usize, from.0, to.0);
    if let Some(r) = STATE.with(|s| s.borrow().reaches.get(&key).copied()) {
        return r;
    }
    let mut seen = vec![false; repo.commits.len()];
    let mut stack = vec![from];
    let mut found = false;
    while let Some(c) = stack.pop() {
        if c == to {
            found = true;
            break;
        }
        if std::mem::replace(&mut seen[c.ix()], true) {
            continue;
        }
        stack.extend(repo.commit(c).parents.iter().copied());
    }
    STATE.with(|s| s.borrow_mut().reaches.insert(key, found));
    found
}

fn upstream_of(repo: &Repo, branch: &str) -> Option<String> {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let map = s.upstreams.entry(repo.path.clone()).or_insert_with(|| {
            parterre_core::forge::upstreams(&Git::new(&repo.path)).unwrap_or_default()
        });
        map.get(branch).cloned()
    })
}

fn find_ref<'a>(repo: &'a Repo, full_name: &str) -> Option<&'a GitRef> {
    repo.refs.iter().find(|r| r.full_name == full_name)
}

fn short(full: &str) -> &str {
    full.strip_prefix("refs/heads/")
        .or_else(|| full.strip_prefix("refs/remotes/"))
        .or_else(|| full.strip_prefix("refs/tags/"))
        .unwrap_or(full)
}

/// A branch, or a detached HEAD, that an operation can change: checked out in a worktree, or
/// a local branch checked out nowhere.
#[derive(Clone, Debug)]
struct Target {
    /// `refs/heads/…`; `None` for a detached HEAD.
    branch: Option<String>,
    worktree: Option<usize>,
    head: Option<CommitIx>,
    open: bool,
}

impl Target {
    fn name(&self) -> String {
        self.branch
            .as_deref()
            .map_or_else(|| "detached HEAD".to_owned(), |b| short(b).to_owned())
    }

    /// Where it is: the hint after its name.
    fn place(&self, repo: &Repo) -> String {
        match self.worktree {
            _ if self.open => "open worktree".to_owned(),
            Some(w) => format!("in {}", repo.worktrees[w].name()),
            None => "not checked out".to_owned(),
        }
    }
}

/// The checked-out targets, the open worktree first; then, with `all`, the local branches
/// checked out nowhere.
fn targets(repo: &Repo, all: bool) -> Vec<Target> {
    let mut out: Vec<Target> = Vec::new();
    if repo.worktrees.is_empty() {
        let (branch, head) = match &repo.head {
            parterre_core::Head::Branch { name, target } => {
                (Some(format!("refs/heads/{name}")), *target)
            }
            parterre_core::Head::Detached(c) => (None, Some(*c)),
        };
        out.push(Target {
            branch,
            worktree: None,
            head,
            open: true,
        });
    }
    for (i, w) in repo.worktrees.iter().enumerate() {
        if w.missing {
            continue;
        }
        out.push(Target {
            branch: w.branch.clone(),
            worktree: Some(i),
            head: w.head,
            open: w.open,
        });
    }
    out.sort_by_key(|t| !t.open);
    if all {
        for r in &repo.refs {
            if r.kind == RefKind::LocalBranch
                && !out
                    .iter()
                    .any(|t| t.branch.as_deref() == Some(r.full_name.as_str()))
            {
                out.push(Target {
                    branch: Some(r.full_name.clone()),
                    worktree: None,
                    head: Some(r.target),
                    open: false,
                });
            }
        }
    }
    out
}

fn open_target(repo: &Repo) -> Option<Target> {
    targets(repo, false).into_iter().find(|t| t.open)
}

fn worktree_of(repo: &Repo, branch: &str) -> Option<usize> {
    repo.worktrees
        .iter()
        .position(|w| w.branch.as_deref() == Some(branch))
}

/// The refs a menu offers operations on: branches and tags on the commit, not `origin/HEAD`.
fn refs_on(repo: &Repo, commit: CommitIx) -> Vec<&GitRef> {
    let mut refs: Vec<&GitRef> = repo
        .refs
        .iter()
        .filter(|r| r.target == commit)
        .filter(|r| {
            matches!(
                r.kind,
                RefKind::LocalBranch | RefKind::RemoteBranch | RefKind::Tag
            )
        })
        .filter(|r| !r.name.ends_with("/HEAD"))
        .collect();
    refs.sort_by_key(|r| (r.kind, r.full_name.clone()));
    refs
}

/// How the commit is named in a command: its best ref, else its short hash.
fn rev_name(repo: &Repo, commit: CommitIx) -> String {
    refs_on(repo, commit).first().map_or_else(
        || repo.commit(commit).oid.short(repo.abbrev_len),
        |r| r.name.clone(),
    )
}

fn one(worktree: Option<usize>, command: String) -> Vec<Choice> {
    vec![Choice {
        label: String::new(),
        worktree,
        commands: vec![command],
        note: None,
    }]
}

fn op(title: String, level: Level, choices: Vec<Choice>) -> Op {
    Op {
        title,
        level,
        choices,
        picked: 0,
        name: None,
        pick_in_dialog: false,
    }
}

// ---------------------------------------------------------------------------------------------
// The operations. Each says what it would run, or why it can't.

type Offer = Result<Op, String>;

/// Switch the open worktree to a local branch, or open the worktree that has it.
fn switch_local(repo: &Repo, r: &GitRef) -> Offer {
    match worktree_of(repo, &r.full_name) {
        Some(w) if repo.worktrees[w].open => Err(format!("{} is checked out here already", r.name)),
        Some(w) => Ok(op(
            format!("Open {} in parterre", repo.worktrees[w].name()),
            Level::OneClick,
            vec![Choice {
                label: String::new(),
                worktree: Some(w),
                commands: vec![],
                note: Some(format!(
                    "{} is checked out in {}. It replaces the open worktree in this window.",
                    r.name,
                    repo.worktrees[w].path.display()
                )),
            }],
        )),
        None => Ok(op(
            format!("Switch to {}", r.name),
            Level::OneClick,
            one(None, format!("git switch {}", r.name)),
        )),
    }
}

/// Switch to a remote branch: to the local branch that tracks it, or a new one.
fn switch_remote(repo: &Repo, r: &GitRef) -> Offer {
    let (_, local) = r.name.split_once('/').unwrap_or(("", &r.name));
    let full = format!("refs/heads/{local}");
    if let Some(l) = find_ref(repo, &full) {
        return if upstream_of(repo, &full).as_deref() == Some(r.full_name.as_str()) {
            switch_local(repo, l).map(|mut o| {
                o.title = format!("{} (tracks {})", o.title, r.name);
                o
            })
        } else {
            Err(format!(
                "A local branch {local} exists already, with another upstream"
            ))
        };
    }
    Ok(op(
        format!("Switch to {} as a new branch {local}", r.name),
        Level::OneClick,
        one(
            None,
            format!("git switch --create {local} --track {}", r.name),
        ),
    ))
}

fn switch_detached(repo: &Repo, commit: CommitIx) -> Offer {
    let hash = repo.commit(commit).oid.short(repo.abbrev_len);
    if repo.head_commit() == Some(commit)
        && !matches!(repo.head, parterre_core::Head::Branch { .. })
    {
        return Err("HEAD is here already".to_owned());
    }
    Ok(op(
        format!("Switch to {hash}, detached"),
        Level::OneClick,
        one(None, format!("git switch --detach {hash}")),
    ))
}

/// The two ways into a branch checked out nowhere that can't simply be fast-forwarded.
fn elsewhere(branch: &str, then: &str) -> Vec<Choice> {
    vec![
        Choice {
            label: format!("Switch the open worktree to {branch}, then {then}"),
            worktree: None,
            commands: vec![format!("git switch {branch}"), format!("git {then}")],
            note: None,
        },
        Choice {
            label: format!("Add a worktree for {branch}, then {then} there"),
            worktree: None,
            commands: vec![
                format!("git worktree add <folder> {branch}"),
                format!("git -C <folder> {then}"),
            ],
            note: Some("The Add worktree dialog opens first.".to_owned()),
        },
    ]
}

fn merge_into(repo: &Repo, source: CommitIx, what: &str, t: &Target) -> Offer {
    let into = t.name();
    let Some(head) = t.head else {
        return Err("Nothing is committed there yet".to_owned());
    };
    if reaches(repo, head, source) {
        return Err(format!("{into} contains {what} already"));
    }
    if t.worktree.is_some() || t.open {
        let title = format!("Merge {what} into {into}");
        return Ok(op(
            title,
            Level::Confirm,
            one(t.worktree.filter(|_| !t.open), format!("git merge {what}")),
        ));
    }
    if reaches(repo, source, head) {
        return Ok(op(
            format!("Fast-forward {into} to {what}"),
            Level::OneClick,
            one(
                None,
                format!(
                    "git fetch . {what}:{}",
                    t.branch.as_deref().unwrap_or_default()
                ),
            ),
        ));
    }
    Ok(op(
        format!("Merge {what} into {into}"),
        Level::Confirm,
        elsewhere(&into, &format!("merge {what}")),
    ))
}

fn rebase_onto(repo: &Repo, onto: CommitIx, what: &str, t: &Target) -> Offer {
    let name = t.name();
    let Some(head) = t.head else {
        return Err("Nothing is committed there yet".to_owned());
    };
    if reaches(repo, head, onto) {
        return Err(format!("{name} is on top of {what} already"));
    }
    if t.worktree.is_some() || t.open {
        return Ok(op(
            format!("Rebase {name} onto {what}"),
            Level::Confirm,
            one(t.worktree.filter(|_| !t.open), format!("git rebase {what}")),
        ));
    }
    Ok(op(
        format!("Rebase {name} onto {what}"),
        Level::Confirm,
        elsewhere(&name, &format!("rebase {what}")),
    ))
}

fn cherry_pick_into(repo: &Repo, commit: CommitIx, t: &Target) -> Offer {
    let hash = repo.commit(commit).oid.short(repo.abbrev_len);
    let Some(head) = t.head else {
        return Err("Nothing is committed there yet".to_owned());
    };
    if reaches(repo, head, commit) {
        return Err(format!("{} contains {hash} already", t.name()));
    }
    Ok(op(
        format!("Cherry-pick {hash} into {}", t.name()),
        Level::Confirm,
        one(
            t.worktree.filter(|_| !t.open),
            format!("git cherry-pick {hash}"),
        ),
    ))
}

fn revert_in(repo: &Repo, commit: CommitIx, t: &Target) -> Offer {
    let hash = repo.commit(commit).oid.short(repo.abbrev_len);
    let Some(head) = t.head else {
        return Err("Nothing is committed there yet".to_owned());
    };
    if !reaches(repo, head, commit) {
        return Err(format!("{} doesn't contain {hash}", t.name()));
    }
    Ok(op(
        format!("Revert {hash} in {}", t.name()),
        Level::Confirm,
        one(
            t.worktree.filter(|_| !t.open),
            format!("git revert --no-edit {hash}"),
        ),
    ))
}

fn create_branch(repo: &Repo, commit: CommitIx) -> Offer {
    let hash = repo.commit(commit).oid.short(repo.abbrev_len);
    let mut o = op(
        format!("Create a branch at {hash}"),
        Level::Confirm,
        one(None, format!("git branch {{name}} {hash}")),
    );
    o.name = Some(String::new());
    Ok(o)
}

fn add_worktree(repo: &Repo, commit: CommitIx) -> Offer {
    let hash = repo.commit(commit).oid.short(repo.abbrev_len);
    let mut o = op(
        format!("Add a worktree at {hash}"),
        Level::Confirm,
        one(
            None,
            format!("git worktree add <folder> -b {{name}} {hash}"),
        ),
    );
    o.choices[0].note = Some("The Add worktree dialog is its own prototype (#158).".to_owned());
    o.name = Some(String::new());
    Ok(o)
}

/// Push, first push, force push: whichever the branch needs.
fn push(repo: &Repo, r: &GitRef) -> Offer {
    let Some(up) = upstream_of(repo, &r.full_name) else {
        let mut o = op(
            format!("Push {} to origin…", r.name),
            Level::Confirm,
            one(None, "git push -u origin {name}".to_owned()),
        );
        o.choices[0].note = Some(format!(
            "{} has no upstream yet. The name on the remote can be changed.",
            r.name
        ));
        o.name = Some(r.name.clone());
        return Ok(o);
    };
    let Some(u) = find_ref(repo, &up) else {
        return Err(format!("Its upstream {} is gone", short(&up)));
    };
    let (remote, remote_branch) = short(&up).split_once('/').unwrap_or(("origin", short(&up)));
    if reaches(repo, u.target, r.target) {
        return Err(format!("Nothing to push: {} has it all", u.name));
    }
    if reaches(repo, r.target, u.target) {
        return Ok(op(
            format!("Push {} to {}", r.name, u.name),
            Level::OneClick,
            one(
                None,
                format!("git push {remote} {}:{remote_branch}", r.name),
            ),
        ));
    }
    if remote_branch != r.name {
        return Err(format!(
            "{} has diverged from {}, which has another name: no force push",
            r.name, u.name
        ));
    }
    Ok(op(
        format!("Force push {} to {}…", r.name, u.name),
        Level::Confirm,
        one(
            None,
            format!(
                "git push --force-with-lease=refs/heads/{0} --force-if-includes {remote} {0}",
                r.name
            ),
        ),
    ))
}

/// Pull where it's checked out; fast-forward where it isn't.
fn pull(repo: &Repo, r: &GitRef) -> Offer {
    let up =
        upstream_of(repo, &r.full_name).ok_or_else(|| format!("{} has no upstream", r.name))?;
    let u = find_ref(repo, &up).ok_or_else(|| format!("Its upstream {} is gone", short(&up)))?;
    if reaches(repo, r.target, u.target) {
        return Err(format!("Up to date with {}", u.name));
    }
    match worktree_of(repo, &r.full_name) {
        Some(w) => Ok(op(
            format!("Pull {} from {}", r.name, u.name),
            Level::OneClick,
            one(
                Some(w).filter(|&w| !repo.worktrees[w].open),
                "git pull".to_owned(),
            ),
        )),
        None if reaches(repo, u.target, r.target) => Ok(op(
            format!("Fast-forward {} to {}", r.name, u.name),
            Level::OneClick,
            one(None, format!("git fetch . {}:{}", u.full_name, r.full_name)),
        )),
        None => Err(format!(
            "{} has diverged from {}: check it out to pull",
            r.name, u.name
        )),
    }
}

fn set_upstream(repo: &Repo, r: &GitRef) -> Offer {
    let now = upstream_of(repo, &r.full_name).map_or("none".to_owned(), |u| short(&u).to_owned());
    let mut o = op(
        format!("Set the upstream of {} (now: {now})", r.name),
        Level::Confirm,
        one(
            None,
            format!("git branch --set-upstream-to={{name}} {}", r.name),
        ),
    );
    o.name = Some(format!("origin/{}", r.name));
    Ok(o)
}

/// Delete a local branch; a branch checked out elsewhere offers removing that worktree.
fn delete_local(repo: &Repo, r: &GitRef) -> Offer {
    match worktree_of(repo, &r.full_name) {
        Some(w) if repo.worktrees[w].open => {
            Err(format!("{} is checked out in the open worktree", r.name))
        }
        Some(0) => Err(format!("{} is checked out in the main worktree", r.name)),
        Some(w) => remove_worktree(repo, w).map(|mut o| {
            o.title = format!("{} (checked out there: {})", o.title, r.name);
            o
        }),
        None => Ok(op(
            format!("Delete {}", r.name),
            Level::OneClick,
            one(None, format!("git branch -d {}", r.name)),
        )),
    }
}

fn delete_remote(repo: &Repo, r: &GitRef) -> Offer {
    let (remote, b) = r.name.split_once('/').unwrap_or(("origin", &r.name));
    let hash = repo.commit(r.target).oid.short(repo.abbrev_len);
    let mut o = op(
        format!("Delete {b} on {remote}…"),
        Level::Warning,
        one(
            None,
            format!("git push --force-with-lease=refs/heads/{b}:{hash} {remote} --delete {b}"),
        ),
    );
    o.choices[0].note = Some(format!("Deletes {b} on {remote} for everyone."));
    Ok(o)
}

fn remove_worktree(repo: &Repo, w: usize) -> Offer {
    let wt = &repo.worktrees[w];
    if w == 0 {
        return Err("The main worktree can't be removed".to_owned());
    }
    if wt.open {
        return Err("This is the open worktree: open another one first".to_owned());
    }
    if wt.locked {
        return Err("It's locked (git worktree unlock)".to_owned());
    }
    Ok(op(
        format!("Remove worktree {}…", wt.name()),
        Level::Confirm,
        one(None, format!("git worktree remove {}", wt.path.display())),
    ))
}

fn open_worktree(repo: &Repo, w: usize) -> Offer {
    let wt = &repo.worktrees[w];
    if wt.open {
        return Err("It's open".to_owned());
    }
    if wt.missing {
        return Err("Its folder is gone".to_owned());
    }
    Ok(op(
        format!("Open {} in parterre", wt.name()),
        Level::OneClick,
        vec![Choice {
            label: String::new(),
            worktree: Some(w),
            commands: vec![],
            note: Some("It replaces the open worktree in this window.".to_owned()),
        }],
    ))
}

pub fn fetch_op() {
    open(op(
        "Fetch all remotes".to_owned(),
        Level::OneClick,
        one(None, "git fetch --all --prune".to_owned()),
    ));
}

// ---------------------------------------------------------------------------------------------
// Menu items.

/// An item for an offer: greyed out with the reason when it can't be done. `hint` goes on the
/// right, weak.
fn item(ui: &mut Ui, label: &str, hint: &str, offer: Offer) {
    let button = egui::Button::new(label).shortcut_text(hint);
    match offer {
        Ok(o) => {
            if ui.add(button).clicked() {
                open(o);
                ui.close();
            }
        }
        Err(why) => {
            ui.add_enabled(false, button).on_disabled_hover_text(why);
        }
    }
}

/// A dialog item for variant B: the dialog lists the targets.
fn dialog_item(ui: &mut Ui, label: &str, title: String, offers: Vec<(String, Offer)>) {
    let choices: Vec<Choice> = offers
        .into_iter()
        .filter_map(|(target, o)| {
            let o = o.ok()?;
            Some(o.choices.into_iter().map(move |mut c| {
                c.label = if c.label.is_empty() {
                    target.clone()
                } else {
                    format!("{target}: {}", c.label)
                };
                c
            }))
        })
        .flatten()
        .collect();
    let button = egui::Button::new(label);
    if choices.is_empty() {
        ui.add_enabled(false, button)
            .on_disabled_hover_text("Nowhere to do it");
        return;
    }
    if ui.add(button).clicked() {
        let mut o = op(title, Level::Confirm, choices);
        o.pick_in_dialog = true;
        open(o);
        ui.close();
    }
}

/// The targets as menu items, each running `offer` for it.
fn target_items(
    ui: &mut Ui,
    repo: &Repo,
    all: bool,
    skip: Option<&str>,
    offer: impl Fn(&Target) -> Offer,
) {
    let list = targets(repo, all);
    let here = MENU_COMMIT.with(|c| c.get());
    let mut nowhere = false;
    for t in &list {
        if skip.is_some() && t.branch.as_deref() == skip
            || !t.open && t.head.is_some() && t.head == here
        {
            continue;
        }
        if t.worktree.is_none() && !t.open && !nowhere {
            nowhere = true;
            crate::menu::separator(ui);
        }
        item(ui, &t.name(), &t.place(repo), offer(t));
    }
}

fn offers(
    repo: &Repo,
    all: bool,
    skip: Option<&str>,
    offer: impl Fn(&Target) -> Offer,
) -> Vec<(String, Offer)> {
    targets(repo, all)
        .into_iter()
        .filter(|t| skip.is_none() || t.branch.as_deref() != skip)
        .filter(|t| t.open || t.head.is_none() || t.head != MENU_COMMIT.with(|c| c.get()))
        .map(|t| (format!("{} ({})", t.name(), t.place(repo)), offer(&t)))
        .collect()
}

fn branch_ops(ui: &mut Ui, repo: &Repo, r: &GitRef, prefix: &str) {
    if r.kind == RefKind::LocalBranch {
        let label = |s: &str| format!("{s}{prefix}");
        match push(repo, r) {
            Ok(o) if o.title.starts_with("Force") => item(ui, &label("Force push…"), "", Ok(o)),
            o => item(ui, &label("Push"), "", o),
        }
        match pull(repo, r) {
            Ok(o) if o.title.starts_with("Fast") => {
                item(ui, &label("Fast-forward to upstream"), "", Ok(o))
            }
            o => item(ui, &label("Pull"), "", o),
        }
        item(ui, &label("Set upstream…"), "", set_upstream(repo, r));
    }
}

fn delete_item(ui: &mut Ui, repo: &Repo, r: &GitRef, label: &str) {
    match r.kind {
        RefKind::LocalBranch => match worktree_of(repo, &r.full_name) {
            Some(w) if w != 0 && !repo.worktrees[w].open => item(
                ui,
                &format!("Remove worktree {}…", repo.worktrees[w].name()),
                "",
                remove_worktree(repo, w),
            ),
            _ => item(ui, label, "", delete_local(repo, r)),
        },
        RefKind::RemoteBranch => item(ui, label, "", delete_remote(repo, r)),
        _ => {}
    }
}

fn switch_offer(repo: &Repo, r: &GitRef) -> Offer {
    match r.kind {
        RefKind::LocalBranch => switch_local(repo, r),
        RefKind::RemoteBranch => switch_remote(repo, r),
        _ => switch_detached(repo, r.target),
    }
}

// ---------------------------------------------------------------------------------------------
// The variants.

/// The operations at the top of a node's or a row's menu. `clicked` is what was under the
/// pointer (the graph only).
pub fn menu(ui: &mut Ui, repo: &Repo, commit: CommitIx, graph: bool) {
    MENU_COMMIT.with(|c| c.set(Some(commit)));
    let clicked = if graph {
        STATE.with(|s| s.borrow().clicked.clone())
    } else {
        Clicked::Commit
    };
    match variant() {
        0 => verbs(ui, repo, commit),
        1 => nouns(ui, repo, commit),
        _ => label_clicked(ui, repo, commit, clicked),
    }
    crate::menu::separator(ui);
}

/// A: verbs first.
fn verbs(ui: &mut Ui, repo: &Repo, commit: CommitIx) {
    let refs = refs_on(repo, commit);
    let what = rev_name(repo, commit);
    let branches: Vec<&GitRef> = refs
        .iter()
        .copied()
        .filter(|r| r.kind != RefKind::Tag)
        .collect();
    crate::menu::plain_submenu(ui, "Switch to", |ui| {
        for r in &branches {
            let hint = match r.kind {
                RefKind::RemoteBranch => "new local branch",
                _ => "",
            };
            let label = match worktree_of(repo, &r.full_name) {
                Some(w) if !repo.worktrees[w].open => format!("{} (open its worktree)", r.name),
                _ => r.name.clone(),
            };
            item(ui, &label, hint, switch_offer(repo, r));
        }
        if !branches.is_empty() {
            crate::menu::separator(ui);
        }
        item(
            ui,
            "This commit (detached)",
            "",
            switch_detached(repo, commit),
        );
    });
    crate::menu::plain_submenu(ui, &format!("Merge {what} into"), |ui| {
        target_items(ui, repo, true, None, |t| merge_into(repo, commit, &what, t));
    });
    crate::menu::plain_submenu(ui, &format!("Rebase onto {what}"), |ui| {
        target_items(ui, repo, true, None, |t| {
            rebase_onto(repo, commit, &what, t)
        });
    });
    crate::menu::plain_submenu(ui, "Cherry-pick into", |ui| {
        target_items(ui, repo, false, None, |t| cherry_pick_into(repo, commit, t));
    });
    crate::menu::plain_submenu(ui, "Revert in", |ui| {
        target_items(ui, repo, false, None, |t| revert_in(repo, commit, t));
    });
    crate::menu::separator(ui);
    let locals: Vec<&GitRef> = refs
        .iter()
        .copied()
        .filter(|r| r.kind == RefKind::LocalBranch)
        .collect();
    for r in &locals {
        let prefix = if locals.len() > 1 {
            format!(" {}", r.name)
        } else {
            String::new()
        };
        branch_ops(ui, repo, r, &prefix);
    }
    item(ui, "Create branch here…", "", create_branch(repo, commit));
    item(ui, "Add worktree here…", "", add_worktree(repo, commit));
    let deletable: Vec<&GitRef> = branches.clone();
    let worktrees: Vec<usize> = (0..repo.worktrees.len())
        .filter(|&w| {
            repo.worktrees[w].head == Some(commit)
                && repo.worktrees[w].branch.is_none()
                && !repo.worktrees[w].open
        })
        .collect();
    if !deletable.is_empty() || !worktrees.is_empty() {
        crate::menu::plain_submenu(ui, "Delete", |ui| {
            for r in &deletable {
                let label = match r.kind {
                    RefKind::RemoteBranch => format!("{} (on the remote)", r.name),
                    _ => r.name.clone(),
                };
                delete_item(ui, repo, r, &label);
            }
            for &w in &worktrees {
                item(
                    ui,
                    &format!("Worktree {}…", repo.worktrees[w].name()),
                    "",
                    remove_worktree(repo, w),
                );
            }
        });
    }
}

/// B: a submenu per branch, and one for the commit; dialogs pick the targets.
fn nouns(ui: &mut Ui, repo: &Repo, commit: CommitIx) {
    let refs = refs_on(repo, commit);
    for r in refs.iter().filter(|r| r.kind != RefKind::Tag) {
        let place = worktree_of(repo, &r.full_name).map(|w| {
            if repo.worktrees[w].open {
                " (open worktree)".to_owned()
            } else {
                format!(" (in {})", repo.worktrees[w].name())
            }
        });
        crate::menu::plain_submenu(
            ui,
            &format!("{}{}", r.name, place.unwrap_or_default()),
            |ui| {
                let label = match worktree_of(repo, &r.full_name) {
                    Some(w) if !repo.worktrees[w].open => {
                        format!("Open {} in parterre", repo.worktrees[w].name())
                    }
                    _ => "Switch to it".to_owned(),
                };
                item(ui, &label, "", switch_offer(repo, r));
                dialog_item(
                    ui,
                    "Merge into…",
                    format!("Merge {}", r.name),
                    offers(repo, true, Some(&r.full_name), |t| {
                        merge_into(repo, commit, &r.name, t)
                    }),
                );
                dialog_item(
                    ui,
                    "Rebase onto it…",
                    format!("Rebase onto {}", r.name),
                    offers(repo, true, Some(&r.full_name), |t| {
                        rebase_onto(repo, commit, &r.name, t)
                    }),
                );
                crate::menu::separator(ui);
                branch_ops(ui, repo, r, "");
                if r.kind == RefKind::LocalBranch {
                    crate::menu::separator(ui);
                }
                delete_item(
                    ui,
                    repo,
                    r,
                    if r.kind == RefKind::RemoteBranch {
                        "Delete on the remote…"
                    } else {
                        "Delete"
                    },
                );
            },
        );
    }
    for w in (0..repo.worktrees.len())
        .filter(|&w| repo.worktrees[w].head == Some(commit) && repo.worktrees[w].branch.is_none())
    {
        crate::menu::plain_submenu(
            ui,
            &format!("Worktree {}", repo.worktrees[w].name()),
            |ui| {
                item(ui, "Open in parterre", "", open_worktree(repo, w));
                item(ui, "Remove…", "", remove_worktree(repo, w));
            },
        );
    }
    let hash = repo.commit(commit).oid.short(repo.abbrev_len);
    crate::menu::plain_submenu(ui, &format!("Commit {hash}"), |ui| {
        item(
            ui,
            "Switch to it (detached)",
            "",
            switch_detached(repo, commit),
        );
        item(ui, "Create branch here…", "", create_branch(repo, commit));
        item(ui, "Add worktree here…", "", add_worktree(repo, commit));
        crate::menu::separator(ui);
        dialog_item(
            ui,
            "Cherry-pick into…",
            format!("Cherry-pick {hash}"),
            offers(repo, false, None, |t| cherry_pick_into(repo, commit, t)),
        );
        dialog_item(
            ui,
            "Revert in…",
            format!("Revert {hash}"),
            offers(repo, false, None, |t| revert_in(repo, commit, t)),
        );
        if refs.iter().all(|r| r.kind == RefKind::Tag) {
            dialog_item(
                ui,
                "Merge into…",
                format!("Merge {hash}"),
                offers(repo, true, None, |t| {
                    merge_into(repo, commit, &rev_name(repo, commit), t)
                }),
            );
            dialog_item(
                ui,
                "Rebase onto it…",
                format!("Rebase onto {hash}"),
                offers(repo, true, None, |t| {
                    rebase_onto(repo, commit, &rev_name(repo, commit), t)
                }),
            );
        }
    });
}

/// C: the label clicked decides the menu; the open worktree is the target unless picked.
fn label_clicked(ui: &mut Ui, repo: &Repo, commit: CommitIx, clicked: Clicked) {
    let open_t = open_target(repo);
    let into_open = open_t.as_ref().map(|t| t.name()).unwrap_or_default();
    let header = |ui: &mut Ui, text: String| {
        ui.add_space(2.0);
        ui.label(RichText::new(text).weak().size(12.0));
        ui.add_space(2.0);
    };
    let r = match &clicked {
        Clicked::Ref(name) => refs_on(repo, commit).into_iter().find(|r| &r.name == name),
        _ => None,
    };
    if let Clicked::Worktree(w) = clicked {
        header(ui, format!("Worktree {}", repo.worktrees[w].name()));
        item(ui, "Open in parterre", "", open_worktree(repo, w));
        item(ui, "Remove worktree…", "", remove_worktree(repo, w));
        return;
    }
    let Some(r) = r.filter(|r| r.kind != RefKind::Tag) else {
        let hash = repo.commit(commit).oid.short(repo.abbrev_len);
        header(
            ui,
            format!("Commit {hash}   (right-click a branch label for its menu)"),
        );
        item(
            ui,
            "Switch here (detached)",
            "",
            switch_detached(repo, commit),
        );
        item(ui, "Create branch here…", "", create_branch(repo, commit));
        item(ui, "Add worktree here…", "", add_worktree(repo, commit));
        crate::menu::separator(ui);
        let what = rev_name(repo, commit);
        match &open_t {
            Some(t) => {
                item(
                    ui,
                    &format!("Cherry-pick into {into_open}"),
                    "",
                    cherry_pick_into(repo, commit, t),
                );
                item(
                    ui,
                    &format!("Revert in {into_open}"),
                    "",
                    revert_in(repo, commit, t),
                );
                item(
                    ui,
                    &format!("Merge {what} into {into_open}"),
                    "",
                    merge_into(repo, commit, &what, t),
                );
                item(
                    ui,
                    &format!("Rebase {into_open} onto {what}"),
                    "",
                    rebase_onto(repo, commit, &what, t),
                );
            }
            None => {}
        }
        crate::menu::plain_submenu(ui, "Another branch", |ui| {
            crate::menu::plain_submenu(ui, "Cherry-pick into", |ui| {
                target_items(
                    ui,
                    repo,
                    false,
                    open_t.as_ref().and_then(|t| t.branch.as_deref()),
                    |t| cherry_pick_into(repo, commit, t),
                )
            });
            crate::menu::plain_submenu(ui, "Revert in", |ui| {
                target_items(
                    ui,
                    repo,
                    false,
                    open_t.as_ref().and_then(|t| t.branch.as_deref()),
                    |t| revert_in(repo, commit, t),
                )
            });
            crate::menu::plain_submenu(ui, &format!("Merge {what} into"), |ui| {
                target_items(
                    ui,
                    repo,
                    true,
                    open_t.as_ref().and_then(|t| t.branch.as_deref()),
                    |t| merge_into(repo, commit, &what, t),
                )
            });
            crate::menu::plain_submenu(ui, &format!("Rebase onto {what}"), |ui| {
                target_items(
                    ui,
                    repo,
                    true,
                    open_t.as_ref().and_then(|t| t.branch.as_deref()),
                    |t| rebase_onto(repo, commit, &what, t),
                )
            });
        });
        return;
    };
    header(ui, format!("Branch {}", r.name));
    let switch_label = match worktree_of(repo, &r.full_name) {
        Some(w) if !repo.worktrees[w].open => {
            format!("Open {} in parterre", repo.worktrees[w].name())
        }
        _ => format!("Switch to {}", r.name),
    };
    item(ui, &switch_label, "", switch_offer(repo, r));
    if let Some(t) = &open_t {
        item(
            ui,
            &format!("Merge {} into {into_open}", r.name),
            "open worktree",
            merge_into(repo, commit, &r.name, t),
        );
        item(
            ui,
            &format!("Rebase {into_open} onto {}", r.name),
            "open worktree",
            rebase_onto(repo, commit, &r.name, t),
        );
    }
    let skip = open_t.as_ref().and_then(|t| t.branch.clone());
    crate::menu::plain_submenu(ui, "Another branch", |ui| {
        crate::menu::plain_submenu(ui, &format!("Merge {} into", r.name), |ui| {
            target_items(ui, repo, true, skip.as_deref(), |t| {
                merge_into(repo, commit, &r.name, t)
            })
        });
        crate::menu::plain_submenu(ui, &format!("Rebase onto {}", r.name), |ui| {
            target_items(ui, repo, true, skip.as_deref(), |t| {
                rebase_onto(repo, commit, &r.name, t)
            })
        });
    });
    crate::menu::separator(ui);
    branch_ops(ui, repo, r, "");
    crate::menu::separator(ui);
    delete_item(
        ui,
        repo,
        r,
        if r.kind == RefKind::RemoteBranch {
            "Delete on the remote…"
        } else {
            "Delete"
        },
    );
}

// ---------------------------------------------------------------------------------------------
// The dialog and the variant bar.

/// The pending operation's dialog: what would run, and where. Nothing runs.
pub fn dialog(ctx: &egui::Context, repo: Option<&Repo>) {
    let Some(repo) = repo else { return };
    let Some(mut o) = STATE.with(|s| s.borrow_mut().pending.take()) else {
        return;
    };
    let mut keep = true;
    let modal = egui::Modal::new(egui::Id::new("prototype-operation")).show(ctx, |ui| {
        ui.set_max_width(560.0);
        ui.label(
            RichText::new("PROTOTYPE: nothing runs")
                .small()
                .color(Color32::from_rgb(200, 120, 0)),
        );
        ui.heading(&o.title);
        let (level, colour) = match o.level {
            Level::OneClick => (
                "One click: runs straight from the menu, this dialog is only the prototype's",
                Color32::GRAY,
            ),
            Level::Confirm => ("Confirmation", Color32::from_rgb(60, 130, 200)),
            Level::Warning => (
                "Warning (when something would be lost)",
                Color32::from_rgb(200, 60, 60),
            ),
        };
        ui.label(RichText::new(level).small().color(colour));
        ui.add_space(8.0);
        if o.choices.len() > 1 {
            ui.label(if o.pick_in_dialog {
                "Into:"
            } else {
                "It's checked out nowhere and can't be fast-forwarded:"
            });
            if o.pick_in_dialog && o.choices.len() > 4 {
                egui::ComboBox::from_id_salt("prototype-target")
                    .width(480.0)
                    .selected_text(o.choices[o.picked].label.clone())
                    .show_ui(ui, |ui| {
                        for (i, c) in o.choices.iter().enumerate() {
                            ui.selectable_value(&mut o.picked, i, &c.label);
                        }
                    });
            } else {
                for (i, c) in o.choices.iter().enumerate() {
                    ui.radio_value(&mut o.picked, i, &c.label);
                }
            }
            ui.add_space(8.0);
        }
        if let Some(name) = &mut o.name {
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.text_edit_singleline(name);
            });
        }
        let c = &o.choices[o.picked];
        let place = match c.worktree {
            Some(w) => format!(
                "{} — another worktree, not the open one",
                repo.worktrees[w].path.display()
            ),
            None => format!("{} — the open worktree", repo.path.display()),
        };
        if !c.commands.is_empty() {
            ui.label(RichText::new(format!("Runs in {place}")).strong());
            for cmd in &c.commands {
                let cmd = cmd.replace("{name}", o.name.as_deref().unwrap_or_default());
                ui.label(RichText::new(cmd).monospace());
            }
        }
        if let Some(note) = &c.note {
            ui.add_space(4.0);
            ui.label(note);
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button("Run (does nothing)").clicked() || ui.button("Cancel").clicked() {
                keep = false;
            }
        });
    });
    if modal.should_close() {
        keep = false;
    }
    if keep {
        STATE.with(|s| s.borrow_mut().pending = Some(o));
    }
}

/// The bar that switches between variants, bottom centre.
pub fn variant_bar(ctx: &egui::Context) {
    egui::Area::new(egui::Id::new("prototype-variant-bar"))
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -40.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgb(30, 30, 30))
                .corner_radius(20)
                .inner_margin(egui::Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let v = variant();
                        let n = VARIANTS.len();
                        let text = |s: &str| RichText::new(s).color(Color32::WHITE);
                        let mut to = None;
                        if ui.add(egui::Button::new(text("◀")).frame(false)).clicked() {
                            to = Some((v + n - 1) % n);
                        }
                        let (key, name) = VARIANTS[v];
                        ui.label(text(&format!("Menus {key} — {name}")));
                        if ui.add(egui::Button::new(text("▶")).frame(false)).clicked() {
                            to = Some((v + 1) % n);
                        }
                        if let Some(to) = to {
                            STATE.with(|s| s.borrow_mut().variant = to);
                        }
                    });
                });
        });
}
