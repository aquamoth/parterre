//! PROTOTYPE — throwaway. Where operations are offered in the menus, for
//! "Operations in the menus: prototype" (aquamoth/parterre#144). Nothing here runs git to change
//! anything: picking an operation shows a dialog saying what would run.
//!
//! Round 2: operations only ever change the open worktree's branch (merge into it, rebase it,
//! pull it). Other branches can be switched to, pushed, fast-forwarded, given an upstream or
//! deleted. Other worktrees can only be gone to or deleted. Detached checkouts, cherry-pick and
//! revert are only in the log window.
//!
//! The variants differ in how a node with several branches offers them, switched with the bar
//! at the bottom of the main window, or `PARTERRE_MENU_VARIANT=A|B|C`:
//! - A: flat, every item names its branch.
//! - B: one item per verb; a submenu of branches when there are several.
//! - C: the label right-clicked decides: a branch's, a worktree's, or the commit's items.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText, Ui};
use parterre_core::git::Git;
use parterre_core::{CommitIx, GitRef, Head, RefKind, Repo};

#[allow(dead_code)]
pub const VARIANTS: [(&str, &str); 3] = [
    ("A", "Flat, every item names its branch"),
    ("B", "One item per verb, branches in a submenu"),
    ("C", "The label right-clicked decides"),
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
    /// Deleting a worktree: a confirmation, a warning when work would be lost.
    Big,
}

#[derive(Clone, Debug)]
struct Op {
    title: String,
    level: Level,
    commands: Vec<String>,
    note: Option<String>,
    /// A name the user types, put in place of `{name}` in the commands.
    name: Option<String>,
    /// *Also delete branch …*, and whether it's ticked.
    also_delete: Option<(String, bool)>,
}

fn op(title: impl Into<String>, level: Level, command: impl Into<String>) -> Op {
    Op {
        title: title.into(),
        level,
        commands: vec![command.into()],
        note: None,
        name: None,
        also_delete: None,
    }
}

impl Op {
    fn note(mut self, note: impl Into<String>) -> Op {
        self.note = Some(note.into());
        self
    }
}

struct State {
    variant: usize,
    clicked: Clicked,
    pending: Option<Op>,
    upstreams: HashMap<PathBuf, HashMap<String, String>>,
    reaches: HashMap<(usize, u32, u32), bool>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State {
        variant: match std::env::var("PARTERRE_MENU_VARIANT").as_deref() {
            Ok("A" | "a") => 0,
            Ok("B" | "b") => 1,
            _ => 2,
        },
        clicked: Clicked::Commit,
        pending: None,
        upstreams: HashMap::new(),
        reaches: HashMap::new(),
    });
    static CLOSE: Cell<bool> = const { Cell::new(false) };
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

/// Forgets the cached upstreams and ancestry, so a reload shows new ones.
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
        .unwrap_or(full)
}

fn hash(repo: &Repo, c: CommitIx) -> String {
    repo.commit(c).oid.short(repo.abbrev_len)
}

/// What the open worktree has checked out: the only thing merge, rebase, cherry-pick, revert
/// and pull change.
struct Current {
    /// `refs/heads/…`; `None` when detached.
    branch: Option<String>,
    head: Option<CommitIx>,
}

impl Current {
    fn of(repo: &Repo) -> Current {
        match &repo.head {
            Head::Branch { name, target } => Current {
                branch: Some(if name.starts_with("refs/") {
                    name.clone()
                } else {
                    format!("refs/heads/{name}")
                }),
                head: *target,
            },
            Head::Detached(c) => Current {
                branch: None,
                head: Some(*c),
            },
        }
    }

    fn name(&self) -> String {
        self.branch
            .as_deref()
            .map_or_else(|| "HEAD".to_owned(), |b| short(b).to_owned())
    }

    fn is(&self, r: &GitRef) -> bool {
        self.branch.as_deref() == Some(r.full_name.as_str())
    }
}

/// Another worktree that has `branch` checked out.
fn worktree_of(repo: &Repo, branch: &str) -> Option<usize> {
    repo.worktrees
        .iter()
        .position(|w| !w.open && w.branch.as_deref() == Some(branch))
}

/// The branches a node's menu offers: local and remote branches on the commit, not
/// `origin/HEAD`, local ones first.
fn branches_on(repo: &Repo, commit: CommitIx) -> Vec<&GitRef> {
    let mut refs: Vec<&GitRef> = repo
        .refs
        .iter()
        .filter(|r| r.target == commit)
        .filter(|r| matches!(r.kind, RefKind::LocalBranch | RefKind::RemoteBranch))
        .filter(|r| !r.name.ends_with("/HEAD"))
        .collect();
    refs.sort_by_key(|r| (r.kind, r.full_name.clone()));
    refs
}

/// Other worktrees with a detached HEAD on the commit.
fn detached_worktrees_on(repo: &Repo, commit: CommitIx) -> Vec<usize> {
    (0..repo.worktrees.len())
        .filter(|&w| {
            let wt = &repo.worktrees[w];
            !wt.open && wt.branch.is_none() && wt.head == Some(commit)
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The operations. Each says what it would run, or why it can't.

type Offer = Result<Op, String>;

fn switch_local(repo: &Repo, r: &GitRef) -> Offer {
    if let Some(w) = worktree_of(repo, &r.full_name) {
        return go_to_worktree(repo, w);
    }
    Ok(op(
        format!("Switch to {}", r.name),
        Level::OneClick,
        format!("git switch {}", r.name),
    ))
}

/// To the local branch that tracks it, or a new one.
fn switch_remote(repo: &Repo, r: &GitRef, cur: &Current) -> Offer {
    let (_, local) = r.name.split_once('/').unwrap_or(("", &r.name));
    let full = format!("refs/heads/{local}");
    if let Some(l) = find_ref(repo, &full) {
        if upstream_of(repo, &full).as_deref() != Some(r.full_name.as_str()) {
            return Err(format!(
                "A local branch {local} exists already, with another upstream"
            ));
        }
        if cur.is(l) {
            return Err(format!("{local} is checked out here already"));
        }
        return switch_local(repo, l);
    }
    Ok(op(
        format!("Switch to {} as a new branch {local}", r.name),
        Level::OneClick,
        format!("git switch --create {local} --track {}", r.name),
    ))
}

fn merge(repo: &Repo, commit: CommitIx, what: &str, cur: &Current) -> Offer {
    let into = cur.name();
    let head = cur.head.ok_or("Nothing is committed here yet")?;
    if reaches(repo, head, commit) {
        return Err(format!("{into} contains {what} already"));
    }
    Ok(op(
        format!("Merge {what} into {into}"),
        Level::Confirm,
        format!("git merge {what}"),
    ))
}

fn rebase(repo: &Repo, onto: CommitIx, what: &str, cur: &Current) -> Offer {
    let name = cur.name();
    let head = cur.head.ok_or("Nothing is committed here yet")?;
    if reaches(repo, head, onto) {
        return Err(format!("{name} is on top of {what} already"));
    }
    Ok(op(
        format!("Rebase {name} onto {what}"),
        Level::Confirm,
        format!("git rebase {what}"),
    ))
}

fn cherry_pick(repo: &Repo, commit: CommitIx, cur: &Current) -> Offer {
    let h = hash(repo, commit);
    let head = cur.head.ok_or("Nothing is committed here yet")?;
    if reaches(repo, head, commit) {
        return Err(format!("{} contains {h} already", cur.name()));
    }
    Ok(op(
        format!("Cherry-pick {h} into {}", cur.name()),
        Level::Confirm,
        format!("git cherry-pick {h}"),
    ))
}

fn revert(repo: &Repo, commit: CommitIx, cur: &Current) -> Offer {
    let h = hash(repo, commit);
    let head = cur.head.ok_or("Nothing is committed here yet")?;
    if !reaches(repo, head, commit) {
        return Err(format!("{} doesn't contain {h}", cur.name()));
    }
    Ok(op(
        format!("Revert {h} in {}", cur.name()),
        Level::Confirm,
        format!("git revert --no-edit {h}"),
    ))
}

fn switch_detached(repo: &Repo, commit: CommitIx, cur: &Current) -> Offer {
    let h = hash(repo, commit);
    if cur.branch.is_none() && cur.head == Some(commit) {
        return Err("HEAD is here already".to_owned());
    }
    Ok(op(
        format!("Switch to {h}, detached"),
        Level::OneClick,
        format!("git switch --detach {h}"),
    ))
}

fn create_branch(repo: &Repo, commit: CommitIx) -> Offer {
    let h = hash(repo, commit);
    let mut o = op(
        format!("Create a branch at {h}"),
        Level::Confirm,
        format!("git branch {{name}} {h}"),
    );
    o.name = Some(String::new());
    Ok(o)
}

fn add_worktree(repo: &Repo, commit: CommitIx) -> Offer {
    let h = hash(repo, commit);
    let mut o = op(
        format!("Add a worktree at {h}"),
        Level::Confirm,
        format!("git worktree add -b {{name}} <folder> {h}"),
    )
    .note("Always with a branch. The Add worktree dialog is its own prototype (#158).");
    o.name = Some(String::new());
    Ok(o)
}

/// Push, first push, force push: whichever the branch needs.
fn push(repo: &Repo, r: &GitRef) -> Offer {
    let Some(up) = upstream_of(repo, &r.full_name) else {
        let mut o = op(
            format!("Push {} to origin…", r.name),
            Level::Confirm,
            "git push -u origin {name}",
        )
        .note(format!(
            "{} has no upstream yet. Its name on the remote can be changed.",
            r.name
        ));
        o.name = Some(r.name.clone());
        return Ok(o);
    };
    let u = find_ref(repo, &up).ok_or_else(|| format!("Its upstream {} is gone", short(&up)))?;
    let (remote, remote_branch) = short(&up).split_once('/').unwrap_or(("origin", short(&up)));
    if reaches(repo, u.target, r.target) {
        return Err(format!("Nothing to push: {} has it all", u.name));
    }
    if reaches(repo, r.target, u.target) {
        return Ok(op(
            format!("Push {} to {}", r.name, u.name),
            Level::OneClick,
            format!("git push {remote} {}:{remote_branch}", r.name),
        ));
    }
    if remote_branch != r.name {
        return Err(format!(
            "It has diverged from {}, which has another name: no force push",
            u.name
        ));
    }
    Ok(op(
        format!("Force push {} to {}…", r.name, u.name),
        Level::Confirm,
        format!(
            "git push --force-with-lease=refs/heads/{0} --force-if-includes {remote} {0}",
            r.name
        ),
    ))
}

/// Pull the current branch; fast-forward one checked out nowhere.
fn pull(repo: &Repo, r: &GitRef, cur: &Current) -> Offer {
    let up =
        upstream_of(repo, &r.full_name).ok_or_else(|| format!("{} has no upstream", r.name))?;
    let u = find_ref(repo, &up).ok_or_else(|| format!("Its upstream {} is gone", short(&up)))?;
    if reaches(repo, r.target, u.target) {
        return Err(format!("Up to date with {}", u.name));
    }
    if cur.is(r) {
        return Ok(op(
            format!("Pull {} from {}", r.name, u.name),
            Level::OneClick,
            "git pull",
        ));
    }
    if let Some(w) = worktree_of(repo, &r.full_name) {
        return Err(format!(
            "Checked out in {}: go to that worktree to pull",
            repo.worktrees[w].name()
        ));
    }
    if !reaches(repo, u.target, r.target) {
        return Err(format!(
            "It has diverged from {}: switch to it to pull",
            u.name
        ));
    }
    Ok(op(
        format!("Fast-forward {} to {}", r.name, u.name),
        Level::OneClick,
        format!("git fetch . {}:{}", u.full_name, r.full_name),
    )
    .note("Changes no worktree: only the branch moves."))
}

fn set_upstream(repo: &Repo, r: &GitRef) -> Offer {
    let now = upstream_of(repo, &r.full_name).map_or("none".to_owned(), |u| short(&u).to_owned());
    let mut o = op(
        format!("Set the upstream of {} (now: {now})", r.name),
        Level::Confirm,
        format!("git branch --set-upstream-to={{name}} {}", r.name),
    );
    o.name = Some(format!("origin/{}", r.name));
    Ok(o)
}

fn delete_local(repo: &Repo, r: &GitRef, cur: &Current) -> Offer {
    if cur.is(r) {
        return Err(format!("{} is checked out here", r.name));
    }
    match worktree_of(repo, &r.full_name) {
        Some(w) => delete_worktree(repo, w),
        None => Ok(op(
            format!("Delete {}", r.name),
            Level::OneClick,
            format!("git branch -d {}", r.name),
        )),
    }
}

fn delete_remote(repo: &Repo, r: &GitRef) -> Offer {
    let (remote, b) = r.name.split_once('/').unwrap_or(("origin", &r.name));
    Ok(op(
        format!("Delete {b} on {remote}…"),
        Level::Confirm,
        format!(
            "git push --force-with-lease=refs/heads/{b}:{} {remote} --delete {b}",
            hash(repo, r.target)
        ),
    )
    .note(format!("Deletes {b} on {remote} for everyone.")))
}

fn go_to_worktree(repo: &Repo, w: usize) -> Offer {
    let wt = &repo.worktrees[w];
    if wt.missing {
        return Err("Its folder is gone".to_owned());
    }
    let mut o = op(
        format!("Go to worktree {}", wt.name()),
        Level::OneClick,
        String::new(),
    )
    .note(format!(
        "This window shows {} as the open worktree instead. Nothing runs in git.",
        wt.path.display()
    ));
    o.commands.clear();
    Ok(o)
}

fn delete_worktree(repo: &Repo, w: usize) -> Offer {
    let wt = &repo.worktrees[w];
    if w == 0 {
        return Err("The main worktree can't be deleted".to_owned());
    }
    if wt.locked {
        return Err("It's locked (git worktree unlock)".to_owned());
    }
    let mut o = op(
        format!("Delete worktree {}…", wt.name()),
        Level::Big,
        format!("git worktree remove {}", wt.path.display()),
    )
    .note(format!(
        "Deletes the folder {} and everything in it.",
        wt.path.display()
    ));
    o.also_delete = wt.branch.as_deref().map(|b| (short(b).to_owned(), false));
    Ok(o)
}

pub fn fetch_op() {
    open(op(
        "Fetch all remotes",
        Level::OneClick,
        "git fetch --all --prune",
    ));
}

// ---------------------------------------------------------------------------------------------
// Entries: every item a node's menu could show, before a variant arranges them.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Verb {
    Switch,
    Merge,
    Rebase,
    Push,
    Pull,
    Upstream,
    Delete,
}

const VERBS: [Verb; 7] = [
    Verb::Switch,
    Verb::Merge,
    Verb::Rebase,
    Verb::Push,
    Verb::Pull,
    Verb::Upstream,
    Verb::Delete,
];

struct Entry {
    verb: Verb,
    /// The whole label, naming the branch: *Merge feature/login into main*.
    full: String,
    /// The label in a submenu of branches: *feature/login*.
    short: String,
    offer: Offer,
}

/// A branch's entries; the current branch has none that change it from here.
fn entries(repo: &Repo, r: &GitRef, cur: &Current) -> Vec<Entry> {
    let n = r.name.clone();
    let into = cur.name();
    let mut out = Vec::new();
    let mut add = |verb, full: String, offer: Offer| {
        out.push(Entry {
            verb,
            full,
            short: n.clone(),
            offer,
        })
    };
    let current = cur.is(r);
    match r.kind {
        RefKind::LocalBranch => {
            if !current {
                let switch = switch_local(repo, r);
                let label = match worktree_of(repo, &r.full_name) {
                    Some(w) => format!("Go to worktree {}", repo.worktrees[w].name()),
                    None => format!("Switch to {n}"),
                };
                add(Verb::Switch, label, switch);
                add(
                    Verb::Merge,
                    format!("Merge {n} into {into}"),
                    merge(repo, r.target, &n, cur),
                );
                add(
                    Verb::Rebase,
                    format!("Rebase {into} onto {n}"),
                    rebase(repo, r.target, &n, cur),
                );
            }
            let push = push(repo, r);
            let label = match &push {
                Ok(o) if o.title.starts_with("Force") => format!("Force push {n}…"),
                Ok(o) if o.title.ends_with('…') => format!("Push {n}…"),
                _ => format!("Push {n}"),
            };
            add(Verb::Push, label, push);
            let label = if current {
                format!("Pull {n}")
            } else {
                format!("Fast-forward {n} to upstream")
            };
            add(Verb::Pull, label, pull(repo, r, cur));
            add(
                Verb::Upstream,
                format!("Set upstream of {n}…"),
                set_upstream(repo, r),
            );
            let label = match worktree_of(repo, &r.full_name) {
                Some(w) => format!("Delete worktree {}…", repo.worktrees[w].name()),
                None => format!("Delete {n}"),
            };
            add(Verb::Delete, label, delete_local(repo, r, cur));
        }
        RefKind::RemoteBranch => {
            add(
                Verb::Switch,
                format!("Switch to {n}"),
                switch_remote(repo, r, cur),
            );
            add(
                Verb::Merge,
                format!("Merge {n} into {into}"),
                merge(repo, r.target, &n, cur),
            );
            add(
                Verb::Rebase,
                format!("Rebase {into} onto {n}"),
                rebase(repo, r.target, &n, cur),
            );
            add(
                Verb::Delete,
                format!("Delete {n} on the remote…"),
                delete_remote(repo, r),
            );
        }
        _ => {}
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Menu items.

/// An item for an offer: greyed out with the reason when it can't be done. `hint` goes on the
/// right, weak.
fn item(ui: &mut Ui, label: &str, offer: Offer) {
    let button = egui::Button::new(label);
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

fn verb_title(verb: Verb, cur: &Current) -> String {
    match verb {
        Verb::Switch => "Switch to".to_owned(),
        Verb::Merge => format!("Merge into {}", cur.name()),
        Verb::Rebase => format!("Rebase {} onto", cur.name()),
        Verb::Push => "Push".to_owned(),
        Verb::Pull => "Pull or fast-forward".to_owned(),
        Verb::Upstream => "Set upstream of".to_owned(),
        Verb::Delete => "Delete".to_owned(),
    }
}

fn commit_items(ui: &mut Ui, repo: &Repo, commit: CommitIx) {
    item(ui, "Create branch here…", create_branch(repo, commit));
    item(ui, "Add worktree here…", add_worktree(repo, commit));
}

fn worktree_items(ui: &mut Ui, repo: &Repo, w: usize, named: bool) {
    let name = repo.worktrees[w].name();
    let (go, delete) = if named {
        (
            format!("Go to worktree {name}"),
            format!("Delete worktree {name}…"),
        )
    } else {
        ("Go to worktree".to_owned(), "Delete worktree…".to_owned())
    };
    item(ui, &go, go_to_worktree(repo, w));
    item(ui, &delete, delete_worktree(repo, w));
}

// ---------------------------------------------------------------------------------------------
// The variants.

/// The operations at the top of a graph node's menu.
pub fn node_menu(ui: &mut Ui, repo: &Repo, commit: CommitIx) {
    let cur = Current::of(repo);
    let branches = branches_on(repo, commit);
    match variant() {
        0 => flat(ui, repo, commit, &branches, &cur),
        1 => by_verb(ui, repo, commit, &branches, &cur),
        _ => {
            let clicked = STATE.with(|s| s.borrow().clicked.clone());
            label_clicked(ui, repo, commit, &branches, &cur, clicked)
        }
    }
    crate::menu::separator(ui);
}

/// A: every branch's items, one after the other.
fn flat(ui: &mut Ui, repo: &Repo, commit: CommitIx, branches: &[&GitRef], cur: &Current) {
    for r in branches {
        let list = entries(repo, r, cur);
        if list.is_empty() {
            continue;
        }
        for e in list {
            item(ui, &e.full, e.offer);
        }
        crate::menu::separator(ui);
    }
    for w in detached_worktrees_on(repo, commit) {
        worktree_items(ui, repo, w, true);
        crate::menu::separator(ui);
    }
    commit_items(ui, repo, commit);
}

/// B: an item per verb; several branches make it a submenu.
fn by_verb(ui: &mut Ui, repo: &Repo, commit: CommitIx, branches: &[&GitRef], cur: &Current) {
    let mut all: Vec<Entry> = branches
        .iter()
        .flat_map(|r| entries(repo, r, cur))
        .collect();
    for (i, verb) in VERBS.into_iter().enumerate() {
        if i == 3 && !all.is_empty() {
            crate::menu::separator(ui);
        }
        let mine: Vec<Entry> = {
            let (mine, rest) = all.into_iter().partition(|e| e.verb == verb);
            all = rest;
            mine
        };
        match mine.len() {
            0 => {}
            1 => {
                let e = mine.into_iter().next().unwrap();
                item(ui, &e.full, e.offer);
            }
            _ => crate::menu::plain_submenu(ui, &verb_title(verb, cur), |ui| {
                for e in mine {
                    let label =
                        if e.full.starts_with("Go to") || e.full.starts_with("Delete worktree") {
                            e.full.clone()
                        } else {
                            e.short.clone()
                        };
                    item(ui, &label, e.offer);
                }
            }),
        }
    }
    let detached = detached_worktrees_on(repo, commit);
    if !detached.is_empty() {
        crate::menu::separator(ui);
        for w in detached {
            worktree_items(ui, repo, w, true);
        }
    }
    crate::menu::separator(ui);
    commit_items(ui, repo, commit);
}

/// C: the label right-clicked decides.
fn label_clicked(
    ui: &mut Ui,
    repo: &Repo,
    commit: CommitIx,
    branches: &[&GitRef],
    cur: &Current,
    clicked: Clicked,
) {
    let header = |ui: &mut Ui, text: String| {
        ui.add_space(2.0);
        ui.label(RichText::new(text).weak().size(12.0));
        ui.add_space(2.0);
    };
    match clicked {
        Clicked::Worktree(w) => {
            header(ui, format!("Worktree {}", repo.worktrees[w].name()));
            worktree_items(ui, repo, w, false);
        }
        Clicked::Ref(name) if branches.iter().any(|r| r.name == name) => {
            let r = branches.iter().find(|r| r.name == name).unwrap();
            header(ui, format!("Branch {}", r.name));
            let list = entries(repo, r, cur);
            if list.is_empty() {
                ui.label(RichText::new("The current branch").weak());
            }
            let mut last = None;
            let (advanced, list): (Vec<Entry>, Vec<Entry>) =
                list.into_iter().partition(|e| e.verb == Verb::Upstream);
            let mut advanced = Some(advanced).filter(|a| !a.is_empty());
            for e in list {
                if e.verb == Verb::Delete
                    && let Some(advanced) = advanced.take()
                {
                    crate::menu::plain_submenu(ui, "Advanced", |ui| {
                        for e in advanced {
                            item(ui, &e.full, e.offer);
                        }
                    });
                }
                let group = matches!(e.verb, Verb::Push | Verb::Pull | Verb::Upstream);
                if last.is_some_and(|g| g != group) || last.is_some() && e.verb == Verb::Delete {
                    crate::menu::separator(ui);
                }
                last = Some(group);
                item(ui, &e.full, e.offer);
            }
            if let Some(advanced) = advanced {
                crate::menu::plain_submenu(ui, "Advanced", |ui| {
                    for e in advanced {
                        item(ui, &e.full, e.offer);
                    }
                });
            }
        }
        _ => {
            header(
                ui,
                format!(
                    "Commit {}   (right-click a label for its menu)",
                    hash(repo, commit)
                ),
            );
            commit_items(ui, repo, commit);
        }
    }
}

/// The operations at the top of a log row's menu: the commit's, the same in every variant.
pub fn row_menu(ui: &mut Ui, repo: &Repo, commit: CommitIx) {
    let cur = Current::of(repo);
    let h = hash(repo, commit);
    // Hidden, not greyed out, where they don't apply: a commit is either in the current
    // branch (revert) or not (cherry-pick).
    let mut any = false;
    for (label, offer) in [
        (
            format!("Cherry-pick into {}", cur.name()),
            cherry_pick(repo, commit, &cur),
        ),
        (
            format!("Revert in {}", cur.name()),
            revert(repo, commit, &cur),
        ),
    ] {
        if offer.is_ok() {
            item(ui, &label, offer);
            any = true;
        }
    }
    if any {
        crate::menu::separator(ui);
    }
    commit_items(ui, repo, commit);
    crate::menu::plain_submenu(ui, "Advanced", |ui| {
        item(
            ui,
            &format!("Switch to {h} (detached)"),
            switch_detached(repo, commit, &cur),
        );
    });
    crate::menu::separator(ui);
}

// ---------------------------------------------------------------------------------------------
// The dialog and the variant bar.

/// The pending operation's dialog: what would run. Nothing runs.
pub fn dialog(ctx: &egui::Context, repo: Option<&Repo>) {
    let Some(repo) = repo else { return };
    let Some(mut o) = STATE.with(|s| s.borrow_mut().pending.take()) else {
        return;
    };
    CLOSE.with(|c| c.set(false));
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
                "One click: runs straight from the menu; only the prototype shows this",
                Color32::GRAY,
            ),
            Level::Confirm => ("Confirmation", Color32::from_rgb(60, 130, 200)),
            Level::Big => (
                "Confirmation, or a warning listing what would be lost",
                Color32::from_rgb(200, 60, 60),
            ),
        };
        ui.label(RichText::new(level).small().color(colour));
        ui.add_space(8.0);
        if let Some(name) = &mut o.name {
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.text_edit_singleline(name);
            });
        }
        if let Some((branch, tick)) = &mut o.also_delete {
            ui.checkbox(tick, format!("Also delete branch {branch}"));
        }
        if !o.commands.is_empty() {
            ui.label(RichText::new(format!("Runs in {}", repo.path.display())).strong());
            let mut commands = o.commands.clone();
            if let Some((branch, true)) = &o.also_delete {
                commands.push(format!("git branch -d {branch}"));
            }
            for cmd in commands {
                let cmd = cmd.replace("{name}", o.name.as_deref().unwrap_or_default());
                ui.label(RichText::new(cmd).monospace());
            }
        }
        if let Some(note) = &o.note {
            ui.add_space(4.0);
            ui.label(note);
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button("Run (does nothing)").clicked() || ui.button("Cancel").clicked() {
                CLOSE.with(|c| c.set(true));
            }
        });
    });
    if !modal.should_close() && !CLOSE.with(|c| c.get()) {
        STATE.with(|s| s.borrow_mut().pending = Some(o));
    }
}

/// The bar that switches between variants, bottom centre.
#[allow(dead_code)]
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
