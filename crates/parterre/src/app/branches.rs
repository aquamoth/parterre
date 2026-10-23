//! Git queries and operations run on worker threads. The dialogs are modeless windows, opened
//! over the window they were asked from.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use eframe::egui::{self, Color32, Id, Response, RichText, Ui, ViewportId, vec2};
use parterre_core::banner::BannerTimer;
use parterre_core::branches::{
    Action, AddWorktree, BranchTip, Branches, Catalog, Checkout, Create, CreateDraft, Deletion,
    Outcome, Report, Stuck, Warning, Worktree, command_text,
};
use parterre_core::file_diff::FileDiffSpec;
use parterre_core::merged::MergedBranches;
use parterre_core::remote::{Live, RemoteBranchTip};
use parterre_core::reset::{self, Mode};
use parterre_core::revgraph::GraphOptions;
use parterre_core::text::plural;
use parterre_core::worktree_folder;
use parterre_core::{GitRef, Oid, RefKind, Repo, cherry_pick, glyphs, merge, rebase, revert};
use parterre_util::CancelTree;

use super::cherry_pick::CherryPickDialog;
use super::clone::CloneDialog;
use super::menu_bar::Why;
use super::merge::MergeDialog;
use super::rebase::{RebaseDialog, stuck_color};
use super::remote::{PullDialog, PullRequestCheck, SetUpstreamDialog};
use super::reset::ResetDialog;
use super::revert::{RestoreDialog, RevertDialog};
use super::table_style::Labels;
use crate::file_dialog::{self, Pending};
use crate::theme::Palette;
use crate::usage::{self, Screen};
use crate::{dialogs, menu, widgets, worker};

#[derive(Clone, Debug)]
pub enum Request {
    Create {
        start: Oid,
        track: Option<String>,
        switch: bool,
    },
    AddWorktree {
        start: Oid,
    },
    GoTo(PathBuf),
    Run(Action),
    /// `mode` overrides the default one, for screenshots.
    Reset {
        target: Oid,
        mode: Option<Mode>,
    },
    /// `target` is what the command names `onto` by: a branch's name, or the full hash.
    Rebase {
        onto: Oid,
        target: String,
    },
    /// `target` is what the command names `theirs` by: a branch's name, or the full hash.
    Merge {
        theirs: Oid,
        target: String,
    },
    /// Merging the open worktree's branch into local branch `into`.
    MergeInto {
        into: String,
    },
    /// `name` is what the graph's menu named the picks by.
    CherryPick {
        picks: cherry_pick::Picks,
        name: Option<String>,
    },
    Revert {
        commit: Oid,
    },
    SetUpstream {
        branch: String,
    },
}

/// A log row's menu, as the graph's Git menu has it, except that *Cherry-pick* takes the
/// `selection` rather than everything the branch lacks.
pub fn row_node_menu(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    selection: &[Oid],
    catalog: Option<&Catalog>,
    busy: bool,
    worktrees: bool,
) -> Option<Request> {
    let branch = branch_section(ui, repo, commit, selection, catalog, busy);
    let remote = catalog.and_then(|c| super::remote::section(ui, repo, commit, c, busy));
    let worktree = worktrees
        .then(|| worktree_section(ui, commit, selection, catalog, busy))
        .flatten();
    branch.or(remote).or(worktree)
}

pub(super) const OPERATION_RUNNING: &str = "A Git operation is running";
pub(super) const LOADING_BRANCHES: &str = "Loading branch information";

fn branch_section(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    selection: &[Oid],
    catalog: Option<&Catalog>,
    busy: bool,
) -> Option<Request> {
    let mut request = None;
    menu::separator(ui);
    if ui
        .add_enabled(
            !busy && catalog.is_some(),
            egui::Button::new("Create branch here…"),
        )
        .on_disabled_hover_text(Why::waiting(busy).text())
        .clicked()
    {
        request = Some(Request::Create {
            start: commit,
            track: None,
            switch: false,
        });
        ui.close();
    }
    let Some(catalog) = catalog else {
        return request;
    };
    let refs = refs_at(repo, commit, catalog);
    let switches = switch_targets(repo, commit, &refs, catalog);
    target_menu(ui, "Switch to", &switches, busy, &mut request);
    let deletions = Deletions {
        verb: "Delete branch",
        plural_name: "local branches",
        name: |b: &BranchTip| b.name.clone(),
        delete: Action::DeleteBranches,
    };
    deletions.items(
        ui,
        commit,
        selection,
        |c| deletable_locals(repo, catalog, c),
        busy,
        &mut request,
    );
    let remote_deletions = Deletions {
        verb: "Delete remote branch",
        plural_name: "remote branches",
        name: RemoteBranchTip::name,
        delete: Action::DeleteRemoteBranches,
    };
    remote_deletions.items(
        ui,
        commit,
        selection,
        |c| deletable_remotes(repo, catalog, c),
        busy,
        &mut request,
    );
    if let Some((branch, targets)) = rebase_offer(repo, commit, &refs, catalog) {
        let verb = format!("Rebase {branch} onto");
        target_menu(ui, &verb, &targets, busy, &mut request);
    }
    if let Some((branch, targets)) = merge_offer(repo, commit, &refs, catalog) {
        let into = format!(" into {branch}…");
        target_menu_named(
            ui,
            &format!("Merge into {branch}"),
            |name| format!("Merge {name}{into}"),
            &targets,
            busy,
            &mut request,
        );
    }
    if let Some((source, targets)) = merge_into_offer(repo, commit, &refs, catalog) {
        target_menu_named(
            ui,
            &format!("Merge {source} into"),
            |name| format!("Merge {source} into {name}…"),
            &targets,
            busy,
            &mut request,
        );
    }
    if let Some(pick) = cherry_pick_selection(ui, repo, selection, catalog, busy) {
        request = Some(pick);
    }
    if let Some(reset) = reset_item(ui, commit, Some(catalog), busy) {
        request = Some(reset);
    }
    if let Some(revert) = revert_item(ui, repo, commit, catalog, busy) {
        request = Some(revert);
    }
    request
}

/// The local and remote-tracking branches at `commit`, as the catalog has them there too.
pub(super) fn refs_at<'a>(repo: &'a Repo, commit: Oid, catalog: &Catalog) -> Vec<&'a GitRef> {
    repo.refs
        .iter()
        .filter(|r| is_branch_at(repo, catalog, r, commit))
        .collect()
}

/// Whether `r` is a local or remote-tracking branch at `commit`, as the catalog has it too.
fn is_branch_at(repo: &Repo, catalog: &Catalog, r: &GitRef, commit: Oid) -> bool {
    let at = |name: &str, tip: Oid| name == r.name && tip == commit;
    let in_catalog = || match r.kind {
        RefKind::LocalBranch => catalog.locals.iter().any(|b| at(&b.name, b.tip)),
        RefKind::RemoteBranch => catalog.remotes.iter().any(|b| at(&b.name, b.tip)),
        _ => false,
    };
    matches!(r.kind, RefKind::LocalBranch | RefKind::RemoteBranch)
        && repo.commit(r.target).oid == commit
        && in_catalog()
}

/// *Switch to*: each local branch at `commit` (or tracking a remote one there) that is
/// checked out nowhere, a new branch tracking a remote one nothing tracks, and last the commit
/// itself, detached. Greyed out while the open worktree is stuck.
pub(super) fn switch_targets(
    repo: &Repo,
    commit: Oid,
    refs: &[&GitRef],
    catalog: &Catalog,
) -> Vec<Target> {
    let mut switches: Vec<Target> = Vec::new();
    let mut seen = HashSet::new();
    if catalog.has_working_tree {
        for r in refs {
            let candidates: Vec<&str> = match r.kind {
                RefKind::LocalBranch => vec![r.name.as_str()],
                RefKind::RemoteBranch => catalog.trackers(&r.name),
                _ => continue,
            };
            if r.kind == RefKind::RemoteBranch
                && candidates.is_empty()
                && !r.name.ends_with("/HEAD")
            {
                switches.push((
                    r.name.clone(),
                    Request::Create {
                        start: commit,
                        track: Some(r.name.clone()),
                        switch: true,
                    },
                    None,
                ));
            }
            for name in candidates {
                if catalog.current.as_deref() != Some(name)
                    && !catalog.occupied.contains_key(name)
                    && seen.insert(name.to_owned())
                {
                    switches.push((
                        name.to_owned(),
                        Request::Run(Action::Switch(name.to_owned())),
                        None,
                    ));
                }
            }
        }
    }
    let local_targets: HashSet<_> = switches
        .iter()
        .filter(|(_, request, _)| matches!(request, Request::Run(Action::Switch(_))))
        .map(|(name, _, _)| name.clone())
        .collect();
    for (name, request, _) in &mut switches {
        if matches!(request, Request::Create { .. }) && local_targets.contains(name) {
            name.push_str(" (remote)");
        }
    }
    switches.sort_by(|a, b| a.0.cmp(&b.0));
    if catalog.has_working_tree && !(catalog.current.is_none() && catalog.head == Some(commit)) {
        switches.push((
            format!("{} (detached)", commit.short(repo.abbrev_len.max(4))),
            Request::Run(Action::Detach(commit)),
            None,
        ));
    }
    if let Some(reason) = catalog.stuck().map(Stuck::reason) {
        for target in &mut switches {
            target.2 = Some(reason.clone());
        }
    }
    switches
}

/// The local branches at `commit` that are checked out nowhere.
pub(super) fn deletable_locals(repo: &Repo, catalog: &Catalog, commit: Oid) -> Vec<BranchTip> {
    let mut found: Vec<BranchTip> = repo
        .refs
        .iter()
        .filter(|r| {
            r.kind == RefKind::LocalBranch
                && catalog.current.as_ref() != Some(&r.name)
                && !catalog.occupied.contains_key(&r.name)
                && is_branch_at(repo, catalog, r, commit)
        })
        .map(|r| BranchTip {
            name: r.name.clone(),
            tip: commit,
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// The remote-tracking branches at `commit`, their remotes' HEADs left out.
pub(super) fn deletable_remotes(
    repo: &Repo,
    catalog: &Catalog,
    commit: Oid,
) -> Vec<RemoteBranchTip> {
    let mut found: Vec<RemoteBranchTip> = repo
        .refs
        .iter()
        .filter(|r| {
            r.kind == RefKind::RemoteBranch
                && !r.name.ends_with("/HEAD")
                && is_branch_at(repo, catalog, r, commit)
        })
        .filter_map(|r| {
            let (remote, branch) = catalog.tracking_parts(&r.name)?;
            Some(RemoteBranchTip {
                remote: remote.to_owned(),
                branch: branch.to_owned(),
                tip: commit,
            })
        })
        .collect();
    found.sort_by_key(RemoteBranchTip::name);
    found
}

/// Deleting what's at a node: one item each (*Delete branch* X), or, when every node of a
/// group has some, one for them all (*Delete 3 branches*).
struct Deletions<T> {
    verb: &'static str,
    plural_name: &'static str,
    name: fn(&T) -> String,
    delete: fn(Vec<T>) -> Action,
}

impl<T: Clone> Deletions<T> {
    fn items(
        &self,
        ui: &mut Ui,
        commit: Oid,
        group: &[Oid],
        at: impl Fn(Oid) -> Vec<T>,
        busy: bool,
        request: &mut Option<Request>,
    ) {
        let each: Vec<Vec<T>> = group.iter().map(|&c| at(c)).collect();
        if group.len() > 1 && each.iter().all(|found| !found.is_empty()) {
            let all: Vec<T> = each.into_iter().flatten().collect();
            let names: Vec<String> = all.iter().map(self.name).collect();
            let label = format!("Delete {} {}", all.len(), self.plural_name);
            let delete = Request::Run((self.delete)(all));
            all_item(ui, label, &names, delete, None, busy, request);
        } else {
            let targets: Vec<Target> = at(commit)
                .into_iter()
                .map(|t| ((self.name)(&t), Request::Run((self.delete)(vec![t])), None))
                .collect();
            target_menu(ui, self.verb, &targets, busy, request);
        }
    }
}

/// The branch names at a node, its remotes' HEADs and the current branch left out, sorted.
fn names_at<'a>(refs: &[&'a GitRef], catalog: &Catalog) -> Vec<&'a str> {
    let mut names: Vec<&str> = refs
        .iter()
        .filter(|r| !r.name.ends_with("/HEAD"))
        .map(|r| r.name.as_str())
        .filter(|name| catalog.current.as_deref() != Some(*name))
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// The branch an operation's item names. While the open worktree is stuck, the item is greyed
/// out rather than hidden, and names the branch it's stuck on; otherwise the item is there only
/// if `offered` gives a branch.
fn offered_branch<S: Into<String>>(
    catalog: &Catalog,
    offered: impl FnOnce() -> Option<S>,
) -> Option<String> {
    match catalog.stuck() {
        Some(_) => Some(stuck_branch(catalog)),
        None => offered().map(Into::into),
    }
}

/// Each branch on the node and, last, the commit itself, as *Switch to* has it (for nodes with
/// no branch, and for log rows), asked for by `request` with the name the command takes.
fn node_targets(
    repo: &Repo,
    commit: Oid,
    refs: &[&GitRef],
    catalog: &Catalog,
    request: impl Fn(String) -> Request,
) -> Vec<Target> {
    let stuck = catalog.stuck().map(Stuck::reason);
    let mut targets: Vec<Target> = names_at(refs, catalog)
        .into_iter()
        .map(|name| (name.to_owned(), request(name.to_owned()), stuck.clone()))
        .collect();
    let short = commit.short(repo.abbrev_len.max(7));
    targets.push((short, request(commit.to_hex()), stuck));
    targets
}

/// *Rebase main onto X*: the branch rebased and its targets, when it would really rebase.
pub(super) fn rebase_offer(
    repo: &Repo,
    commit: Oid,
    refs: &[&GitRef],
    catalog: &Catalog,
) -> Option<(String, Vec<Target>)> {
    let branch = offered_branch(catalog, || rebase::offered(repo, catalog, commit))?;
    let targets = node_targets(repo, commit, refs, catalog, |target| Request::Rebase {
        onto: commit,
        target,
    });
    Some((branch, targets))
}

/// *Merge X into main…*: the branch merged into and what to merge, when there's something to
/// merge.
pub(super) fn merge_offer(
    repo: &Repo,
    commit: Oid,
    refs: &[&GitRef],
    catalog: &Catalog,
) -> Option<(String, Vec<Target>)> {
    let branch = offered_branch(catalog, || merge::offered(repo, catalog, commit))?;
    let targets = node_targets(repo, commit, refs, catalog, |target| Request::Merge {
        theirs: commit,
        target,
    });
    Some((branch, targets))
}

/// *Merge main into feature…*: the open worktree's branch, and each local branch on the node
/// that lacks its commits.
pub(super) fn merge_into_offer(
    repo: &Repo,
    commit: Oid,
    refs: &[&GitRef],
    catalog: &Catalog,
) -> Option<(String, Vec<Target>)> {
    let source = merge::offered_into(repo, catalog, commit)?;
    let stuck = catalog.stuck().map(Stuck::reason);
    let mut names: Vec<&str> = refs
        .iter()
        .filter(|r| r.kind == RefKind::LocalBranch && r.name != source)
        .map(|r| r.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    let targets: Vec<Target> = names
        .into_iter()
        .map(|name| {
            let into = name.to_owned();
            (into.clone(), Request::MergeInto { into }, stuck.clone())
        })
        .collect();
    Some((source.to_owned(), targets))
}

/// *Cherry-pick feature onto main…*: every commit of the node main lacks, named after a branch
/// on the node (a local one first), or its short hash: that name, the branch picked onto, and
/// the request.
pub(super) fn cherry_pick_offer(
    repo: &Repo,
    commit: Oid,
    refs: &[&GitRef],
    catalog: &Catalog,
) -> Option<(String, String, Request)> {
    let branch = offered_branch(catalog, || cherry_pick::offered(repo, catalog, commit))?;
    let mut names: Vec<&GitRef> = refs
        .iter()
        .copied()
        .filter(|r| !r.name.ends_with("/HEAD") && r.name != branch)
        .collect();
    names.sort_by_key(|r| (r.kind != RefKind::LocalBranch, r.name.as_str()));
    let name = names
        .first()
        .map(|r| r.name.clone())
        .unwrap_or_else(|| commit.short(repo.abbrev_len.max(7)));
    let request = Request::CherryPick {
        picks: cherry_pick::Picks::Lacking(commit),
        name: Some(name.clone()),
    };
    Some((name, branch, request))
}

/// *Cherry-pick 3 commits onto main…*: the log's selection, as listed.
fn cherry_pick_selection(
    ui: &mut Ui,
    repo: &Repo,
    selection: &[Oid],
    catalog: &Catalog,
    busy: bool,
) -> Option<Request> {
    let branch = offered_branch(catalog, || {
        cherry_pick::offered_chosen(repo, catalog, selection)
    })?;
    let what = match selection {
        [one] => one.short(repo.abbrev_len.max(7)),
        many => format!("{} commits", many.len()),
    };
    let label = format!("Cherry-pick {what} onto {branch}…");
    let request = Request::CherryPick {
        picks: cherry_pick::Picks::Chosen(selection.to_vec()),
        name: None,
    };
    operation_item(ui, label, catalog, busy)
        .clicked()
        .then_some(request)
}

/// An operation's menu item, greyed out while `busy` or the open worktree is stuck, with why on
/// hover. Closes the menu when clicked.
pub(super) fn operation_item(
    ui: &mut Ui,
    label: String,
    catalog: &Catalog,
    busy: bool,
) -> Response {
    let stuck = catalog.stuck();
    let why: Cow<str> = stuck.map_or(OPERATION_RUNNING.into(), |s| s.reason().into());
    let response = ui
        .add_enabled(!busy && stuck.is_none(), egui::Button::new(label))
        .on_disabled_hover_text(why);
    if response.clicked() {
        ui.close();
    }
    response
}

/// The branch a stuck worktree is rebasing, else its branch, for greyed-out labels.
pub(super) fn stuck_branch(catalog: &Catalog) -> String {
    catalog
        .worktrees
        .iter()
        .find(|w| w.open)
        .and_then(|w| w.rebasing.as_ref())
        .and_then(|r| r.branch.clone())
        .or_else(|| catalog.current.clone())
        .unwrap_or_else(|| "HEAD".into())
}

/// *Add worktree here…*, and going to or deleting the other worktrees at the commit, or at
/// every commit of the `group` it's in.
fn worktree_section(
    ui: &mut Ui,
    commit: Oid,
    group: &[Oid],
    catalog: Option<&Catalog>,
    busy: bool,
) -> Option<Request> {
    let mut request = None;
    menu::separator(ui);
    if ui
        .add_enabled(
            !busy && catalog.is_some(),
            egui::Button::new("Add worktree here…"),
        )
        .on_disabled_hover_text(Why::waiting(busy).text())
        .clicked()
    {
        request = Some(Request::AddWorktree { start: commit });
        ui.close();
    }
    let Some(catalog) = catalog else {
        return request;
    };
    let go_to = go_to_targets(catalog, commit);
    target_menu(ui, "Go to worktree", &go_to, false, &mut request);
    let any_deletable_at = |c: Oid| catalog.worktrees.iter().any(|w| is_deletable_at(w, c));
    if group.len() > 1 && group.iter().all(|&c| any_deletable_at(c)) {
        let all: Vec<&Worktree> = group
            .iter()
            .flat_map(|&c| deletable_worktrees(catalog, c))
            .collect();
        let blocked = all.iter().find_map(|w| {
            let reason = w.locked.as_ref()?;
            Some(if reason.is_empty() {
                format!("{} is locked", w.label())
            } else {
                format!("{} is locked: {reason}", w.label())
            })
        });
        let names: Vec<String> = all.iter().map(|w| w.label()).collect();
        let delete = Request::Run(Action::DeleteWorktrees {
            paths: all.iter().map(|w| w.path.clone()).collect(),
            branches: false,
        });
        let label = format!("Delete {} worktrees", all.len());
        all_item(ui, label, &names, delete, blocked, busy, &mut request);
        return request;
    }
    let deletions = worktree_deletions(catalog, commit);
    target_menu(ui, "Delete worktree", &deletions, busy, &mut request);
    request
}

/// *Go to worktree*: the other worktrees at `commit`; greyed out where the folder is gone.
pub(super) fn go_to_targets(catalog: &Catalog, commit: Oid) -> Vec<Target> {
    let mut others: Vec<_> = catalog
        .worktrees
        .iter()
        .filter(|w| !w.open && w.head == Some(commit))
        .collect();
    others.sort_by_key(|w| w.label());
    others
        .iter()
        .map(|w| {
            (
                w.label(),
                Request::GoTo(w.path.clone()),
                w.missing.then(|| "Its folder is gone".to_owned()),
            )
        })
        .collect()
}

/// The worktrees at `commit` that can be deleted: neither the open one nor the main one.
pub(super) fn deletable_worktrees(catalog: &Catalog, commit: Oid) -> Vec<&Worktree> {
    let mut found: Vec<_> = catalog
        .worktrees
        .iter()
        .filter(|w| is_deletable_at(w, commit))
        .collect();
    found.sort_by_key(|w| w.label());
    found
}

fn is_deletable_at(worktree: &Worktree, commit: Oid) -> bool {
    !worktree.open && !worktree.main && worktree.head == Some(commit)
}

/// *Delete worktree*: each of [`deletable_worktrees`], greyed out while locked. An operation in
/// progress there doesn't keep one: deleting it ends that too.
pub(super) fn worktree_deletions(catalog: &Catalog, commit: Oid) -> Vec<Target> {
    let locked = |w: &Worktree| match &w.locked {
        Some(reason) if reason.is_empty() => Some("Locked".to_owned()),
        Some(reason) => Some(format!("Locked: {reason}")),
        None => None,
    };
    deletable_worktrees(catalog, commit)
        .into_iter()
        .map(|w| {
            (
                w.label(),
                Request::Run(Action::DeleteWorktrees {
                    paths: vec![w.path.clone()],
                    branches: false,
                }),
                locked(w),
            )
        })
        .collect()
}

/// One item for what every node of a group has, such as *Delete 3 worktrees*, naming them on
/// hover.
fn all_item(
    ui: &mut Ui,
    label: String,
    names: &[String],
    value: Request,
    blocked: Option<String>,
    busy: bool,
    request: &mut Option<Request>,
) {
    let response = ui
        .add_enabled(!busy && blocked.is_none(), egui::Button::new(label))
        .on_hover_text(names.join(", "))
        .on_disabled_hover_text(blocked.map_or(Cow::Borrowed(OPERATION_RUNNING), Cow::Owned));
    if response.clicked() {
        *request = Some(value);
        ui.close();
    }
}

/// *Reset `<branch>` to here…*: only the open worktree's branch, and only where there's a
/// reset to offer.
pub fn reset_item(
    ui: &mut Ui,
    commit: Oid,
    catalog: Option<&Catalog>,
    busy: bool,
) -> Option<Request> {
    let catalog = catalog?;
    let branch = offered_branch(catalog, || reset::branch(catalog, commit).ok())?;
    let request = Request::Reset {
        target: commit,
        mode: None,
    };
    operation_item(ui, format!("Reset {branch} to here…"), catalog, busy)
        .clicked()
        .then_some(request)
}

/// *Revert in `<branch>`…*: in the log's rows only, where commits are acted on one by one, for
/// a commit the open worktree's HEAD reaches.
pub fn revert_item(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    catalog: &Catalog,
    busy: bool,
) -> Option<Request> {
    let name = revert::offered(repo, catalog, commit)?;
    let branch = offered_branch(catalog, || Some(name))?;
    operation_item(ui, format!("Revert in {branch}…"), catalog, busy)
        .clicked()
        .then_some(Request::Revert { commit })
}

/// A menu target: its name, what choosing it asks for, and why it's greyed out, if it is.
pub(super) type Target = (String, Request, Option<String>);

fn target_menu(
    ui: &mut Ui,
    verb: &str,
    targets: &[Target],
    busy: bool,
    request: &mut Option<Request>,
) {
    target_menu_named(
        ui,
        verb,
        |name| format!("{verb} {name}"),
        targets,
        busy,
        request,
    );
}

/// [`target_menu`], with a lone target's item labelled by `single`.
pub(super) fn target_menu_named(
    ui: &mut Ui,
    verb: &str,
    single: impl Fn(&str) -> String,
    targets: &[Target],
    busy: bool,
    request: &mut Option<Request>,
) {
    let mut item = |ui: &mut Ui, label: String, value: &Request, blocked: &Option<String>| {
        let enabled = !busy && blocked.is_none();
        let response = ui.add_enabled(enabled, egui::Button::new(label));
        let response = match (busy, blocked) {
            (_, Some(reason)) => response.on_disabled_hover_text(reason),
            (true, None) => response.on_disabled_hover_text(OPERATION_RUNNING),
            _ => response,
        };
        if response.clicked() {
            *request = Some(value.clone());
            ui.close();
        }
    };
    // Every target blocked for the same reason greys the submenu itself.
    let same = targets
        .first()
        .and_then(|t| t.2.clone())
        .filter(|r| targets.iter().all(|t| t.2.as_ref() == Some(r)));
    match targets {
        [] => {}
        [(name, target, blocked)] => item(ui, single(name), target, blocked),
        [_, _, ..] if let Some(reason) = same => {
            ui.add_enabled(false, egui::Button::new(verb))
                .on_disabled_hover_text(reason);
        }
        many => menu::plain_submenu(ui, verb, |ui| {
            for (name, target, blocked) in many {
                item(ui, name.clone(), target, blocked);
            }
        }),
    }
}

#[derive(Debug)]
struct Form {
    repo: Arc<Repo>,
    catalog: Arc<Catalog>,
    start: Oid,
    draft: CreateDraft,
    switch: bool,
    fresh: bool,
    opener: ViewportId,
    worktree: Option<WorktreeFields>,
}

#[derive(Debug)]
struct WorktreeFields {
    /// Always ends in the platform's separator.
    root: String,
    folder_name: String,
    folder_name_edited: bool,
    go_to: bool,
    /// The folder last checked, and the working tree it's unignored in, if any.
    enclosing: Option<(PathBuf, Option<worktree_folder::Inside>)>,
    browse: Option<Pending<()>>,
}

impl WorktreeFields {
    /// Unless typed in, the folder name follows `base`, numbered when that folder is taken.
    fn follow(&mut self, base: &str, registered: &[PathBuf]) {
        if !self.folder_name_edited {
            self.folder_name = worktree_folder::free_name(Path::new(&self.root), base, registered);
        }
    }
}

/// What the branch name of a new worktree amounts to.
enum WorktreeBranch<'a> {
    Detached,
    Existing(&'a str),
    New,
}

fn with_separator(mut path: String) -> String {
    if !path.ends_with(std::path::MAIN_SEPARATOR) {
        path.push(std::path::MAIN_SEPARATOR);
    }
    path
}

impl Form {
    fn new(
        repo: Arc<Repo>,
        catalog: Arc<Catalog>,
        start: Oid,
        prefer: Option<String>,
        switch: bool,
        opener: ViewportId,
    ) -> Self {
        let draft = CreateDraft::new(&catalog, start, prefer.as_deref());
        Self {
            repo,
            catalog,
            start,
            draft,
            switch,
            fresh: true,
            opener,
            worktree: None,
        }
    }

    /// The form with its worktree section. The branch field starts with the commit's first
    /// free local branch, else a remote one's local name, else a suggested new name.
    fn new_worktree(
        repo: Arc<Repo>,
        catalog: Arc<Catalog>,
        start: Oid,
        opener: ViewportId,
    ) -> Self {
        let mut form = Self::new(repo, catalog, start, None, false, opener);
        let root = worktree_folder::default_root(&form.catalog.main, &form.catalog.root);
        form.worktree = Some(WorktreeFields {
            root: with_separator(root.to_string_lossy().into_owned()),
            folder_name: String::new(),
            folder_name_edited: false,
            go_to: false,
            enclosing: None,
            browse: None,
        });
        if let Some(local) = form.free_locals().first() {
            form.draft.set_name(local.clone());
        } else if form.draft.name().is_empty() {
            let name = form.suggested_new_name();
            form.draft.set_name(name);
        }
        form.follow_branch();
        form
    }

    /// The commit's taken branch with `-2`, or the commit's subject as a slug.
    fn suggested_new_name(&self) -> String {
        let taken = self
            .catalog
            .locals
            .iter()
            .find(|b| b.tip == self.start)
            .map(|b| b.name.clone());
        let base = taken.clone().unwrap_or_else(|| {
            self.repo
                .lookup(&self.start)
                .map(|ix| worktree_folder::subject_slug(&self.repo.commit(ix).subject))
                .unwrap_or_default()
        });
        if base.is_empty() {
            return base;
        }
        if taken.is_none() && self.catalog.name_error(&base).is_none() {
            return base;
        }
        (2..100)
            .map(|k| format!("{base}-{k}"))
            .find(|n| self.catalog.name_error(n).is_none())
            .unwrap_or_default()
    }

    /// Local branches at the commit that a new worktree can check out.
    fn free_locals(&self) -> Vec<String> {
        self.catalog
            .locals
            .iter()
            .filter(|b| {
                b.tip == self.start
                    && self.catalog.current.as_deref() != Some(&b.name)
                    && !self.catalog.occupied.contains_key(&b.name)
            })
            .map(|b| b.name.clone())
            .collect()
    }

    /// Remote branches at the commit whose local name is free.
    fn free_remotes(&self) -> Vec<String> {
        self.catalog
            .remotes
            .iter()
            .filter(|r| r.tip == self.start)
            .filter(|r| {
                let local = self.catalog.suggested_name(&r.name);
                !local.is_empty() && !self.catalog.locals.iter().any(|b| b.name == local)
            })
            .map(|r| r.name.clone())
            .collect()
    }

    fn worktree_branch(&self) -> WorktreeBranch<'_> {
        let name = self.draft.name().trim();
        if name.is_empty() {
            WorktreeBranch::Detached
        } else if self.catalog.locals.iter().any(|b| b.name == name) {
            WorktreeBranch::Existing(name)
        } else {
            WorktreeBranch::New
        }
    }

    fn registered(&self) -> Vec<PathBuf> {
        self.catalog
            .worktrees
            .iter()
            .map(|w| w.path.clone())
            .collect()
    }

    /// The folder name the branch gives: `/` becomes `-`, and the short hash when detached.
    fn folder_base(&self) -> String {
        match self.draft.name().trim() {
            "" => self.start.short(self.repo.abbrev_len.max(4)),
            name => worktree_folder::folder_name(name),
        }
    }

    fn follow_branch(&mut self) {
        let (base, registered) = (self.folder_base(), self.registered());
        if let Some(wt) = &mut self.worktree {
            wt.follow(&base, &registered);
        }
    }

    fn folder(&self) -> Option<PathBuf> {
        self.worktree
            .as_ref()
            .map(|wt| Path::new(&wt.root).join(wt.folder_name.trim()))
    }

    fn action(&self) -> Action {
        let Some(path) = self.folder() else {
            return Action::Create(Create {
                start: self.start,
                name: self.draft.name().to_owned(),
                track: self.draft.upstream(),
                switch: self.switch,
            });
        };
        let checkout = match self.worktree_branch() {
            WorktreeBranch::Detached => Checkout::Detached,
            WorktreeBranch::Existing(name) => Checkout::Existing(name.to_owned()),
            WorktreeBranch::New => Checkout::New {
                name: self.draft.name().trim().to_owned(),
                track: self.draft.upstream(),
            },
        };
        Action::AddWorktree(AddWorktree {
            start: self.start,
            path,
            checkout,
        })
    }

    fn worktree_error(&self) -> Option<String> {
        let wt = self.worktree.as_ref()?;
        if !Path::new(&wt.root).is_absolute() {
            return Some("Enter the full path of a folder.".into());
        }
        if let Some(error) = worktree_folder::name_error(&wt.folder_name) {
            return Some(error.into());
        }
        let folder = self.folder()?;
        if wt.folder_name_edited && worktree_folder::taken(&folder, &self.registered()) {
            return Some(format!(
                "The folder '{}' already exists.",
                wt.folder_name.trim()
            ));
        }
        None
    }

    fn branch_error(&self) -> Option<String> {
        match self.worktree_branch() {
            WorktreeBranch::Detached if self.worktree.is_some() => None,
            WorktreeBranch::Existing(name) if self.worktree.is_some() => {
                if self.catalog.current.as_deref() == Some(name) {
                    Some(format!("{name} is checked out here."))
                } else if let Some(at) = self.catalog.occupied.get(name) {
                    Some(format!("{name} is checked out in {}.", at.display()))
                } else if !self.free_locals().iter().any(|b| b == name) {
                    Some(format!("A branch {name} already exists at another commit."))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn commands(&self) -> Result<Vec<Vec<String>>, String> {
        if let Some(error) = self.branch_error().or_else(|| self.worktree_error()) {
            return Err(error);
        }
        Branches::commands(&self.catalog, &self.action()).map_err(|e| e.to_string())
    }

    /// Returns the answer, and whether the commit's hash was clicked.
    fn show(&mut self, ctx: &egui::Context, busy: bool) -> (dialogs::Answer, bool) {
        self.browsed();
        let mut log = false;
        let adding = self.worktree.is_some();
        let (id, title, screen) = if adding {
            ("add-worktree", "Add a worktree", Screen::AddWorktree)
        } else {
            ("create-branch", "Create branch", Screen::CreateBranch)
        };
        let shown = dialogs::Dialog::new(id, title)
            .screen(screen)
            .opener(self.opener)
            .raise(self.fresh)
            .resizable()
            .show(ctx, |ui| {
                let commands = dialogs::fields(ui, |ui| {
                    log = dialogs::commit_line(ui, &self.repo, self.start);
                    if adding {
                        self.worktree_branch_field(ui);
                    } else {
                        self.name_field(ui);
                    }
                    ui.add_space(8.0);
                    if adding && !matches!(self.worktree_branch(), WorktreeBranch::New) {
                        self.frozen_track_field(ui);
                    } else {
                        self.track_field(ui);
                    }
                    if adding {
                        ui.separator();
                        self.folder_fields(ui);
                    }
                    ui.add_space(8.0);
                    match &mut self.worktree {
                        Some(wt) => {
                            ui.checkbox(&mut wt.go_to, "Go to new worktree");
                        }
                        None => {
                            // A stuck worktree switches nowhere.
                            let stuck = self.catalog.stuck();
                            if stuck.is_some() {
                                self.switch = false;
                            }
                            let response = ui.add_enabled(
                                self.catalog.has_working_tree && stuck.is_none(),
                                egui::Checkbox::new(&mut self.switch, "Switch to new branch"),
                            );
                            if let Some(stuck) = stuck {
                                response.on_disabled_hover_text(stuck.reason());
                            }
                        }
                    }
                    self.commands()
                });
                let lines: Vec<String> = commands
                    .as_ref()
                    .map(|cmds| cmds.iter().map(|a| command_text(a)).collect())
                    .unwrap_or_default();
                dialogs::command_box(ui, &lines);
                let label = match &self.worktree {
                    Some(wt) if wt.go_to => "Add and go to",
                    Some(_) => "Add",
                    None if self.switch => "Create and switch",
                    None => "Create",
                };
                let primary = dialogs::Primary::new(label).enabled(commands.is_ok() && !busy);
                dialogs::actions(ui, primary)
            });
        self.fresh = false;
        let mut answer = shown.inner;
        if shown.should_close() && answer == dialogs::Answer::Open {
            answer = dialogs::Answer::Cancel;
        }
        (answer, log)
    }

    /// The branch field of a new worktree: a field with a dropdown of the commit's usable
    /// branches. A new name makes a branch; empty means detached.
    fn worktree_branch_field(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Local branch name").strong());
        let mut choices = self.free_locals();
        choices.extend(self.free_remotes());
        let mut text = self.draft.name().to_owned();
        let response =
            dialogs::editable_choice(ui, "worktree-branch", &mut text, "(detached)", &choices);
        if self.fresh {
            // A suggested name is selected, to be typed over or cleared.
            response.request_focus();
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), response.id) {
                let all = egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(text.chars().count()),
                );
                state.cursor.set_char_range(Some(all));
                state.store(ui.ctx(), response.id);
            }
        }
        if response.changed() {
            if self.catalog.remotes.iter().any(|r| r.name == text.trim()) {
                // A remote branch becomes a local branch that tracks it, as Switch does.
                self.draft = CreateDraft::new(&self.catalog, self.start, Some(text.trim()));
            } else {
                self.draft.set_name(text);
            }
            self.follow_branch();
        }
        let error = match self.worktree_branch() {
            WorktreeBranch::New => self
                .catalog
                .name_error(self.draft.name().trim())
                .map(str::to_owned),
            _ => self.branch_error(),
        };
        if let Some(error) = error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    /// An existing branch keeps its own upstream, and detached has none: the tracking row
    /// shows that, disabled.
    fn frozen_track_field(&mut self, ui: &mut Ui) {
        let upstream = match self.worktree_branch() {
            WorktreeBranch::Existing(name) => self
                .catalog
                .locals
                .iter()
                .find(|b| b.name == name)
                .and_then(|b| b.upstream.clone()),
            _ => None,
        };
        let mut shown = CreateDraft::new(&self.catalog, self.start, upstream.as_deref());
        if upstream.is_none() {
            shown.set_remote(None);
        }
        let draft = std::mem::replace(&mut self.draft, shown);
        ui.add_enabled_ui(false, |ui| self.track_row(ui));
        self.draft = draft;
    }

    fn folder_fields(&mut self, ui: &mut Ui) {
        let base = self.folder_base();
        let registered = self.registered();
        let folder = self.folder();
        let error = self.worktree_error();
        let Some(wt) = &mut self.worktree else { return };
        ui.label(RichText::new("Worktree root").strong());
        let browsing = wt.browse.is_some();
        let (field, browse) = widgets::folder_field(ui, &mut wt.root, "Folder", browsing);
        if field.changed() {
            wt.root = with_separator(std::mem::take(&mut wt.root));
            wt.follow(&base, &registered);
        }
        if browse {
            wt.browse = Some(file_dialog::pick_folder(
                ui.ctx(),
                "Worktree root",
                &wt.root,
            ));
        }
        // A root inside a working tree, where git would see the worktree as untracked.
        if let Some(folder) = &folder {
            if wt.enclosing.as_ref().is_none_or(|(f, _)| f != folder) {
                let inside = worktree_folder::inside_repository(folder);
                wt.enclosing = Some((folder.clone(), inside));
            }
            if let Some((_, Some(inside))) = &wt.enclosing {
                let place = match self
                    .catalog
                    .worktrees
                    .iter()
                    .find(|w| worktree_folder::same_path_on_disk(&w.path, &inside.top))
                {
                    Some(w) => format!("this repository's worktree {}", w.name()),
                    None => format!("another repository, {}", inside.top.display()),
                };
                let mut exclude = false;
                dialogs::caution(ui, |ui| {
                    ui.label(format!(
                        "This folder is inside {place}. It will show there as untracked, \
                         git add . there would record it as an embedded repository, and \
                         git clean -ffdx there would delete it."
                    ));
                    exclude = ui.button("Exclude it").clicked();
                });
                if exclude {
                    match worktree_folder::exclude(inside) {
                        Ok(()) => wt.enclosing = None,
                        Err(e) => {
                            ui.colored_label(ui.visuals().error_fg_color, e.to_string());
                        }
                    }
                }
            }
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Worktree name").strong());
        let name = &mut wt.folder_name;
        let follow_tip = "Follow the branch name";
        if widgets::folder_name_field(ui, name, &mut wt.folder_name_edited, follow_tip) {
            wt.follow(&base, &registered);
        }
        if let Some(error) = error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    /// Takes the folder picked with *Browse…*.
    fn browsed(&mut self) {
        let Some(wt) = &mut self.worktree else { return };
        let Some(answer) = wt.browse.as_ref().and_then(Pending::answer) else {
            return;
        };
        wt.browse = None;
        if let Some(dir) = answer {
            wt.root = with_separator(dir.to_string_lossy().into_owned());
            self.follow_branch();
        }
    }

    fn name_field(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Local branch name").strong());
        let suggestion = self.draft.suggested_name(&self.catalog);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let width = ui.available_width() - widgets::BUTTON_SIZE - 4.0;
            let mut name = self.draft.name().to_owned();
            let response = widgets::text_field(ui, &mut name, "Enter a branch name", width);
            if self.fresh {
                response.request_focus();
            }
            if response.changed() {
                self.draft.set_name(name);
            }
            if ui
                .add_enabled_ui(
                    !suggestion.is_empty() && self.draft.name() != suggestion,
                    |ui| widgets::icon_button(ui, glyphs::RESET, false),
                )
                .inner
                .on_hover_text(format!("Use suggested name: {suggestion}"))
                .clicked()
            {
                self.draft.restore_suggested_name(&self.catalog);
            }
        });
        if !self.draft.name().is_empty()
            && let Some(error) = self.catalog.name_error(self.draft.name())
        {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    fn track_field(&mut self, ui: &mut Ui) {
        self.track_row(ui);
        self.track_notes(ui);
    }

    fn track_row(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Track branch").strong());
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), widgets::BUTTON_SIZE),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let mut selected = self.draft.remote().map(str::to_owned);
                let mut remotes = self.catalog.remote_names.clone();
                remotes.sort();
                let chosen = ui
                    .allocate_ui_with_layout(
                        vec2(110.0, widgets::BUTTON_SIZE),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| dialogs::choice(ui, "track-remote", &mut selected, "None", &remotes),
                    )
                    .inner;
                if chosen {
                    self.draft.set_remote(selected);
                }
                let width = ui.available_width() - widgets::BUTTON_SIZE - 4.0;
                ui.allocate_ui_with_layout(
                    vec2(width, widgets::BUTTON_SIZE),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        let choices = self.draft.remote_branches(&self.catalog);
                        let mut branch = self.draft.track_name().to_owned();
                        let response = ui
                            .add_enabled_ui(self.draft.remote().is_some(), |ui| {
                                dialogs::editable_choice(
                                    ui,
                                    "track-branch",
                                    &mut branch,
                                    "Branch name",
                                    &choices,
                                )
                            })
                            .inner;
                        if response.changed() {
                            self.draft.set_track_name(&self.catalog, branch);
                        }
                    },
                );
                if ui
                    .add_enabled_ui(self.draft.can_restore_track_name(), |ui| {
                        widgets::icon_button(ui, glyphs::RESET, false)
                    })
                    .inner
                    .on_hover_text(format!("Use local branch name: {}", self.draft.name()))
                    .clicked()
                {
                    self.draft.restore_track_name();
                }
            },
        );
    }

    /// Errors in the tracking row, and a note when another branch already tracks it.
    fn track_notes(&mut self, ui: &mut Ui) {
        let upstream = self.draft.upstream();
        let track_error =
            if self.draft.remote().is_some() && self.draft.track_name().trim().is_empty() {
                (!self.draft.name().is_empty()).then_some("Enter a branch name to track.")
            } else {
                upstream
                    .as_deref()
                    .and_then(|s| self.catalog.track_error(s))
            };
        if let Some(error) = track_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        let trackers = upstream
            .as_deref()
            .map(|s| self.catalog.trackers(s))
            .unwrap_or_default();
        if !trackers.is_empty() {
            dialogs::caution(ui, |ui| {
                ui.label(format!(
                    "Already tracked by {}. This creates another local branch tracking {}.",
                    trackers.join(", "),
                    upstream.as_deref().unwrap_or_default()
                ));
            });
        }
    }
}

/// What an operation's dialog asks for in a frame.
#[derive(Debug)]
pub struct Asked {
    pub answer: dialogs::Answer,
    pub show_in_log: Option<Oid>,
}

#[derive(Debug)]
struct Job {
    path: PathBuf,
    label: String,
    opener: ViewportId,
    cancel: CancelTree,
    rx: mpsc::Receiver<Outcome>,
    go_to: Option<PathBuf>,
    cloned: Option<PathBuf>,
    /// Where a reverted branch was, for the log to follow it to the new commit.
    revert_from: Option<Oid>,
    live: Option<Live>,
}

/// A revert done: the log follows the branch from `from` to the new commit `to`, which is all
/// it says. A log that can't follow it gets the notification instead.
#[derive(Debug)]
pub struct Reverted {
    pub path: PathBuf,
    pub from: Oid,
    pub to: Oid,
    pub label: String,
}

const NOTICE_MAX_ERROR_ROWS: usize = 6;

#[derive(Debug)]
struct Notice {
    id: u64,
    title: String,
    path: PathBuf,
    report: Report,
    error: Option<String>,
    at: f64,
    /// Done, but needing the user's attention: orange, with its message in `error`.
    attention: bool,
}

impl Notice {
    fn has_details(&self) -> bool {
        self.error.is_some() || !self.report.steps.is_empty()
    }
}

fn usage_action(action: &Action) -> usage::Action {
    match action {
        Action::Create(_) => usage::Action::CreateBranch,
        Action::Switch(_) => usage::Action::SwitchBranch,
        Action::Detach(_) => usage::Action::SwitchDetached,
        Action::DeleteBranches(_) => usage::Action::DeleteBranch,
        Action::AddWorktree(_) => usage::Action::AddWorktree,
        Action::DeleteWorktrees { .. } => usage::Action::DeleteWorktree,
        Action::Reset(_) => usage::Action::Reset,
        Action::Rebase(_) => usage::Action::Rebase,
        Action::Merge(_) => usage::Action::Merge,
        Action::CherryPick(_) => usage::Action::CherryPick,
        Action::Revert(_) => usage::Action::Revert,
        Action::RestoreStash(_) => usage::Action::RestoreStash,
        Action::Resolve(_) => usage::Action::ResolveConflict,
        Action::Fetch => usage::Action::Fetch,
        Action::Pull(_) => usage::Action::Pull,
        Action::Push(_) => usage::Action::Push,
        Action::SetUpstream(_) => usage::Action::SetUpstream,
        Action::DeleteRemoteBranches(_) => usage::Action::DeleteRemoteBranch,
        Action::Clone(_) => usage::Action::Clone,
    }
}

/// A warning before losing work, or a confirmation, waiting for an answer.
#[derive(Debug)]
struct Loss {
    path: PathBuf,
    warning: Warning,
    fresh: bool,
    opener: ViewportId,
    /// Indices of the deletions whose changed files are listed.
    listing_files: HashSet<usize>,
    pull_request_check: Option<PullRequestCheck>,
}

/// A dialog's preview being read on a worker, and what else the dialog was asked for with.
#[derive(Debug)]
struct PreviewLoading<P, K> {
    request: K,
    opener: ViewportId,
    rx: mpsc::Receiver<Result<P, String>>,
}

impl<P: Send + 'static, K> PreviewLoading<P, K> {
    fn start<E: ToString>(
        ctx: &egui::Context,
        request: K,
        opener: ViewportId,
        load: impl FnOnce() -> Result<P, E> + Send + 'static,
    ) -> Self {
        let rx = worker::spawn(ctx, move || load().map_err(|e| e.to_string()));
        PreviewLoading {
            request,
            opener,
            rx,
        }
    }
}

/// Takes the preview loading in `slot` once it's read. `what` names it, should its worker stop.
fn previewed<P, K>(
    slot: &mut Option<PreviewLoading<P, K>>,
    what: &str,
) -> Option<(Result<P, String>, PreviewLoading<P, K>)> {
    let stopped = || Err(format!("The {what} preview stopped unexpectedly."));
    let result = worker::received(&slot.as_ref()?.rx, stopped)?;
    slot.take().map(|loading| (result, loading))
}

#[derive(Debug)]
struct ResetPreview {
    target: Oid,
    /// Read again for the open dialog.
    refresh: bool,
    mode: Option<Mode>,
}

/// The dialogs of the repository shown, and the previews read for them: another one's close.
#[derive(Debug, Default)]
struct RepoDialogs {
    form: Option<Form>,
    loss: Option<Loss>,
    reset: Option<ResetDialog>,
    reset_loading: Option<PreviewLoading<reset::Preview, ResetPreview>>,
    rebase: Option<RebaseDialog>,
    rebase_loading: Option<PreviewLoading<rebase::Preview, String>>,
    merge: Option<MergeDialog>,
    merge_loading: Option<PreviewLoading<merge::Preview, String>>,
    cherry_pick: Option<CherryPickDialog>,
    cherry_pick_loading: Option<PreviewLoading<cherry_pick::Preview, ()>>,
    revert: Option<RevertDialog>,
    revert_loading: Option<PreviewLoading<revert::Preview, Oid>>,
    restore_stash: Option<RestoreDialog>,
    pull_diverged: Option<PullDialog>,
    set_upstream: Option<SetUpstreamDialog>,
}

#[derive(Debug, Default)]
pub struct Tool {
    repo: Option<Arc<Repo>>,
    pub catalog: Option<Arc<Catalog>>,
    catalog_loading: Option<mpsc::Receiver<Result<Catalog, String>>>,
    dialogs: RepoDialogs,
    pub reverted: Option<Reverted>,
    pub diff_requests: Vec<(Arc<Repo>, FileDiffSpec)>,
    job: Option<Job>,
    notices: Vec<Notice>,
    last_notice_id: u64,
    notice_details: Option<u64>,
    pub log_request: Option<(Arc<Repo>, Vec<Oid>, bool)>,
    pub reload: Option<PathBuf>,
    pub go_to: Option<PathBuf>,
    pub(super) clone: Option<CloneDialog>,
    pub clone_parent: Option<PathBuf>,
    pub cloned: Option<PathBuf>,
    stuck_banner: BannerTimer,
    /// The next look at the worktree shows the banner at once: it follows an operation of
    /// parterre's own, F5 or opening the repository.
    look_at_once: bool,
    /// An operation ended and the worktree hasn't been looked at since.
    unlooked: bool,
    catalog_asked_at: f64,
    pub conflicts_changed: bool,
    canned_pull_requests: Option<Arc<str>>,
    pub canned_repositories: Option<Arc<str>>,
    /// *Clone repository…* starts with a URL on the clipboard: not in scripted runs or tests.
    pub read_clipboard: bool,
}

impl Tool {
    /// A tool that looks up pull requests in `json` (`--pull-requests-from`) rather than on the
    /// forge.
    pub fn with_canned_pull_requests(json: Option<&str>) -> Tool {
        let mut tool = Tool::default();
        tool.canned_pull_requests = json.map(Arc::from);
        tool
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// The next look at the worktree shows the banner at once, if it is stuck: after F5.
    pub fn look_at_once(&mut self) {
        self.look_at_once = true;
    }

    pub fn banner_shown(&mut self) -> bool {
        self.stuck_banner.update(self.busy() || self.unlooked)
    }

    /// Asks for the catalogue of `repo`: its refs and worktrees as the graph shows them.
    fn load_catalog(&mut self, ctx: &egui::Context, repo: &Arc<Repo>) {
        let repo = repo.clone();
        let load = move || Catalog::of(&repo).map_err(|e| e.to_string());
        self.catalog_loading = Some(worker::spawn(ctx, load));
        self.catalog_asked_at = ctx.input(|i| i.time);
    }

    pub fn update(&mut self, ctx: &egui::Context, repo: Option<&Arc<Repo>>) {
        self.follow(ctx, repo);
        // While the worktree is stuck, look again every second: for the banner to show once
        // it lasts, and to go, and for the conflicted files, which change no refs.
        let now = ctx.input(|i| i.time);
        if self.stuck_banner.watching()
            && self.catalog_loading.is_none()
            && let Some(repo) = self.repo.clone()
        {
            let since = now - self.catalog_asked_at;
            if since >= 1.0 {
                self.load_catalog(ctx, &repo);
            } else {
                ctx.request_repaint_after(Duration::from_secs_f64(1.0 - since));
            }
        }
        self.catalog_loaded(ctx, now);
        self.reset_previewed(ctx);
        self.rebase_previewed(ctx);
        self.merge_previewed(ctx);
        self.cherry_pick_previewed(ctx);
        self.revert_previewed(ctx);
        let stopped = || Outcome::Failed {
            error: parterre_core::branches::Error::Failed(
                "Git operation worker stopped unexpectedly. Review the reloaded repository \
                 before retrying."
                    .into(),
            ),
            report: Report::default(),
        };
        let outcome = self
            .job
            .as_ref()
            .and_then(|job| worker::received(&job.rx, stopped));
        if let Some(outcome) = outcome
            && let Some(job) = self.job.take()
        {
            self.job_done(ctx, job, outcome);
        }
    }

    fn follow(&mut self, ctx: &egui::Context, repo: Option<&Arc<Repo>>) {
        let changed = match (&self.repo, repo) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        if !changed {
            return;
        }
        let different_path = self.repo.as_ref().map(|r| &r.path) != repo.map(|r| &r.path);
        if different_path {
            self.dialogs = RepoDialogs::default();
            self.catalog = None;
            self.stuck_banner = BannerTimer::default();
            self.look_at_once = true;
        } else if let Some(dialog) = &self.dialogs.reset
            && self.dialogs.reset_loading.is_none()
        {
            // What the reset does may have changed with the repository.
            let (target, opener) = (dialog.preview.target, dialog.opener);
            self.preview_reset(ctx, target, None, opener, true);
        }
        self.repo = repo.cloned();
        self.catalog_loading = None;
        if let Some(repo) = repo {
            self.load_catalog(ctx, repo);
        }
    }

    fn catalog_loaded(&mut self, ctx: &egui::Context, now: f64) {
        let stopped = || Err("Branch information worker stopped unexpectedly.".into());
        let Some(loaded) = self
            .catalog_loading
            .as_ref()
            .and_then(|rx| worker::received(rx, stopped))
        else {
            return;
        };
        self.catalog_loading = None;
        match loaded {
            Ok(catalog) => {
                crate::startup::mark("branch catalogue loaded");
                let catalog = Arc::new(catalog);
                if let Some(form) = &mut self.dialogs.form {
                    form.catalog = catalog.clone();
                }
                let at_once = std::mem::take(&mut self.look_at_once);
                self.stuck_banner
                    .look(catalog.stuck().is_some(), now, at_once);
                self.unlooked = false;
                self.conflicts_changed |= self
                    .catalog
                    .as_ref()
                    .is_some_and(|old| old.conflicted != catalog.conflicted);
                self.catalog = Some(catalog);
            }
            Err(e) => {
                self.catalog = None;
                if let Some(repo) = &self.repo {
                    let title = "Could not load branch information".into();
                    self.notify_error(ctx, repo.path.clone(), title, e);
                }
            }
        }
    }

    fn job_done(&mut self, ctx: &egui::Context, job: Job, outcome: Outcome) {
        // A question asked before anything ran leaves the repository as it was: nothing to load
        // again while it's asked (#354).
        let unchanged = matches!(&outcome, Outcome::Warning(w) if !w.changed);
        if !unchanged {
            self.reload = Some(job.path.clone());
        }
        let here = self.repo.as_ref().is_some_and(|r| r.path == job.path);
        // What was loaded meanwhile is from before it ended: look at the worktree again at
        // once, for whether it's stuck. The refs follow with the graph's reload.
        if let Some(repo) = self.repo.clone().filter(|_| here && !unchanged) {
            self.unlooked = true;
            self.look_at_once = true;
            self.load_catalog(ctx, &repo);
        }
        // Changes stashed for a revert that didn't stop: the user decides about them.
        if let Outcome::Done(report) | Outcome::Failed { report, .. } = &outcome
            && let Some(stash) = report.stash.clone()
            && here
        {
            self.dialogs.restore_stash = Some(RestoreDialog::new(stash, job.opener));
        }
        if let (Outcome::Done(report), Some(from)) = (&outcome, job.revert_from)
            && report.attention.is_none()
            && let Some(to) = report.created
        {
            self.reverted = Some(Reverted {
                path: job.path,
                from,
                to,
                label: job.label,
            });
            return;
        }
        match outcome {
            Outcome::Warning(warning) if here => {
                let pull_request_check = match &warning.action {
                    Action::DeleteRemoteBranches(branches) => Some(PullRequestCheck::start(
                        ctx,
                        job.path.clone(),
                        branches,
                        self.canned_pull_requests.clone(),
                    )),
                    _ => None,
                };
                self.dialogs.loss = Some(Loss {
                    path: job.path,
                    warning,
                    fresh: true,
                    opener: job.opener,
                    listing_files: HashSet::new(),
                    pull_request_check,
                });
            }
            Outcome::Warning(_) => {
                let error = "Open this repository again to review the commits at risk before \
                             retrying.";
                self.notify_error(ctx, job.path, job.label, error.into());
            }
            Outcome::Done(report) => {
                self.go_to = job.go_to;
                self.cloned = job.cloned;
                match report.attention.clone() {
                    Some(a) => {
                        self.notice(ctx, job.path, a.title, report, Some(a.message));
                        if let Some(n) = self.notices.last_mut() {
                            n.attention = true;
                        }
                    }
                    None => self.notice(ctx, job.path, job.label, report, None),
                }
            }
            Outcome::Failed { error, report } => {
                self.notice(ctx, job.path, job.label, report, Some(error.to_string()))
            }
            Outcome::Diverged(diverged) if here => {
                self.dialogs.pull_diverged = Some(PullDialog::new(*diverged, job.opener));
            }
            Outcome::Diverged(_) => {
                let error = "Open this repository again to pull.".into();
                self.notify_error(ctx, job.path, job.label, error);
            }
        }
    }

    /// `opener` is the window it was asked from, where its dialogs open.
    pub fn request(&mut self, ctx: &egui::Context, request: Request, opener: ViewportId) {
        if let Request::GoTo(path) = request {
            self.go_to = Some(path);
            return;
        }
        if self.busy() {
            return;
        }
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let path = repo.path.clone();
        match request {
            Request::Create {
                start,
                track,
                switch,
            } => {
                if let Some(catalog) = self.catalog.clone() {
                    self.dialogs.form =
                        Some(Form::new(repo, catalog, start, track, switch, opener));
                }
            }
            Request::AddWorktree { start } => {
                if let Some(catalog) = self.catalog.clone() {
                    self.dialogs.form = Some(Form::new_worktree(repo, catalog, start, opener));
                }
            }
            Request::GoTo(_) => unreachable!("handled above"),
            Request::SetUpstream { branch } => {
                if let Some(catalog) = &self.catalog {
                    self.dialogs.set_upstream =
                        Some(SetUpstreamDialog::new(catalog, branch, opener));
                }
            }
            Request::Run(action) => {
                self.run(ctx, path, action, None, opener);
            }
            Request::Reset { target, mode } => self.preview_reset(ctx, target, mode, opener, false),
            Request::Rebase { onto, target } => {
                let load = move || rebase::Preview::load(&path, onto);
                self.dialogs.rebase_loading =
                    Some(PreviewLoading::start(ctx, target, opener, load));
            }
            Request::Merge { theirs, target } => {
                let name = target.clone();
                let load = move || merge::Preview::load(&path, theirs, &name);
                self.dialogs.merge_loading = Some(PreviewLoading::start(ctx, target, opener, load));
            }
            Request::MergeInto { into } => {
                let target = format!("into {into}");
                let load = move || merge::Preview::load_into(&path, &into);
                self.dialogs.merge_loading = Some(PreviewLoading::start(ctx, target, opener, load));
            }
            Request::Revert { commit } => {
                let load = move || revert::Preview::load(&path, commit);
                self.dialogs.revert_loading =
                    Some(PreviewLoading::start(ctx, commit, opener, load));
            }
            Request::CherryPick { picks, name } => {
                let load = move || cherry_pick::Preview::load(&path, &picks, name);
                self.dialogs.cherry_pick_loading =
                    Some(PreviewLoading::start(ctx, (), opener, load));
            }
        }
    }

    fn cherry_pick_previewed(&mut self, ctx: &egui::Context) {
        let Some((result, loading)) =
            previewed(&mut self.dialogs.cherry_pick_loading, "cherry-pick")
        else {
            return;
        };
        let Some(repo) = self.repo.clone() else {
            return;
        };
        match result {
            Ok(preview) => {
                self.dialogs.cherry_pick =
                    Some(CherryPickDialog::new(preview, repo, ctx, loading.opener));
            }
            Err(e) => self.notify_error(ctx, repo.path.clone(), "Cherry-pick".into(), e),
        }
    }

    fn merge_previewed(&mut self, ctx: &egui::Context) {
        let Some((result, loading)) = previewed(&mut self.dialogs.merge_loading, "merge") else {
            return;
        };
        let Some(repo) = self.repo.clone() else {
            return;
        };
        match result {
            Ok(preview) => {
                // Merging the open worktree's branch, the command names it.
                let target = match &preview.outgoing {
                    Some(out) => out.source.clone(),
                    None => loading.request,
                };
                self.dialogs.merge = Some(MergeDialog::new(preview, repo, target, loading.opener));
            }
            Err(e) => {
                let title = format!("Merge {}", loading.request);
                self.notify_error(ctx, repo.path.clone(), title, e);
            }
        }
    }

    fn revert_previewed(&mut self, ctx: &egui::Context) {
        let Some((result, loading)) = previewed(&mut self.dialogs.revert_loading, "revert") else {
            return;
        };
        let Some(repo) = self.repo.clone() else {
            return;
        };
        match result {
            Ok(preview) => {
                self.dialogs.revert = Some(RevertDialog::new(preview, repo, loading.opener));
            }
            Err(e) => {
                let short = loading.request.short(repo.abbrev_len.max(7));
                let title = format!("Revert {short}");
                self.notify_error(ctx, repo.path.clone(), title, e);
            }
        }
    }

    /// The notification for a revert the log couldn't follow, so nothing shows it's done.
    pub fn reverted_unseen(&mut self, ctx: &egui::Context, reverted: Reverted) {
        self.notice(ctx, reverted.path, reverted.label, Report::default(), None);
    }

    fn rebase_previewed(&mut self, ctx: &egui::Context) {
        let Some((result, loading)) = previewed(&mut self.dialogs.rebase_loading, "rebase") else {
            return;
        };
        let Some(repo) = self.repo.clone() else {
            return;
        };
        match result {
            Ok(preview) => {
                let target = loading.request;
                self.dialogs.rebase =
                    Some(RebaseDialog::new(preview, repo, target, loading.opener));
            }
            Err(e) => {
                let title = format!("Rebase onto {}", loading.request);
                self.notify_error(ctx, repo.path.clone(), title, e);
            }
        }
    }

    fn preview_reset(
        &mut self,
        ctx: &egui::Context,
        target: Oid,
        mode: Option<Mode>,
        opener: ViewportId,
        refresh: bool,
    ) {
        let Some(path) = self.repo.as_ref().map(|r| r.path.clone()) else {
            return;
        };
        let request = ResetPreview {
            target,
            refresh,
            mode,
        };
        let load = move || reset::Preview::load(&path, target);
        self.dialogs.reset_loading = Some(PreviewLoading::start(ctx, request, opener, load));
    }

    /// Opens the reset dialog once its preview is read, or brings it up to date.
    fn reset_previewed(&mut self, ctx: &egui::Context) {
        let Some((result, loading)) = previewed(&mut self.dialogs.reset_loading, "reset") else {
            return;
        };
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let ResetPreview {
            target,
            refresh,
            mode,
        } = loading.request;
        match (result, &mut self.dialogs.reset) {
            (Ok(preview), Some(dialog)) if refresh => dialog.refresh(preview, repo),
            // Closed meanwhile.
            (Ok(_), None) if refresh => {}
            (Ok(preview), dialog) => {
                let mut opened = ResetDialog::new(preview, repo, loading.opener, dialog.is_some());
                opened.mode = mode.unwrap_or(opened.mode);
                *dialog = Some(opened);
            }
            // Gone since: reset elsewhere, or the branch is no longer checked out.
            (Err(_), _) if refresh => self.dialogs.reset = None,
            (Err(e), _) => {
                let title = format!("Reset to {}", target.short(repo.abbrev_len.max(7)));
                self.notify_error(ctx, repo.path.clone(), title, e);
            }
        }
    }

    fn run(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        action: Action,
        approval: Option<Warning>,
        opener: ViewportId,
    ) -> &mut Job {
        // Counted when the user starts it, not again when they agree to lose work.
        if approval.is_none() {
            usage::action(usage_action(&action));
        }
        let cancel = CancelTree::default();
        let label = action.label();
        let revert_from = match &action {
            Action::Revert(r) => Some(r.head),
            _ => None,
        };
        let live = action.is_network().then(Live::default);
        let mut branches = Branches::new(path.clone());
        if let Some(live) = &live {
            branches = branches.with_live(live.clone());
        }
        let worker_cancel = cancel.clone();
        let rx = worker::spawn(ctx, move || {
            branches.execute(action, approval.as_ref(), &worker_cancel)
        });
        self.job.insert(Job {
            path,
            label,
            opener,
            cancel,
            rx,
            go_to: None,
            cloned: None,
            revert_from,
            live,
        })
    }

    fn notice(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        title: String,
        report: Report,
        error: Option<String>,
    ) {
        self.last_notice_id += 1;
        self.notices.push(Notice {
            id: self.last_notice_id,
            title,
            path,
            report,
            error,
            at: ctx.input(|i| i.time),
            attention: false,
        });
    }

    fn notify_error(&mut self, ctx: &egui::Context, path: PathBuf, title: String, error: String) {
        self.notice(ctx, path, title, Report::default(), Some(error));
    }

    /// A green notification of something done outside the branch tool, such as a reload. The
    /// same one again shows it anew rather than adding another.
    pub fn inform(&mut self, ctx: &egui::Context, path: PathBuf, title: &str) {
        let at = ctx.input(|i| i.time);
        if let Some(n) = self
            .notices
            .iter_mut()
            .find(|n| n.title == title && n.path == path && !n.has_details())
        {
            n.at = at;
            return;
        }
        self.notice(ctx, path, title.to_owned(), Report::default(), None);
    }

    /// `merged`: the branches found squash- or rebase-merged, striped in the dialogs' commit
    /// lists, and named where they landed when deleting one would lose their commits.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        options: &GraphOptions,
        merged: &MergedBranches,
    ) {
        self.form_dialog(ctx);
        self.reset_dialog(ctx);
        let labels = Labels {
            graph: options,
            merged,
        };
        self.rebase_dialog(ctx, palette, labels);
        self.merge_dialog(ctx, palette, labels);
        self.cherry_pick_dialog(ctx, palette, labels);
        self.revert_dialog(ctx);
        self.restore_stash_dialog(ctx);
        self.pull_diverged_dialog(ctx);
        self.set_upstream_dialog(ctx);
        self.clone_dialog(ctx);
        self.loss_dialog(ctx, merged);
        self.network_window(ctx);
        self.notifications(ctx);
    }

    fn form_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut form) = self.dialogs.form.take() else {
            return;
        };
        let (answer, log) = form.show(ctx, self.busy());
        if log {
            self.log_request = Some((form.repo.clone(), vec![form.start], false));
        }
        match answer {
            dialogs::Answer::Primary => {
                let go_to = form
                    .worktree
                    .as_ref()
                    .filter(|w| w.go_to)
                    .and(form.folder());
                let path = form.repo.path.clone();
                self.run(ctx, path, form.action(), None, form.opener).go_to = go_to;
            }
            dialogs::Answer::Cancel => {}
            dialogs::Answer::Open => self.dialogs.form = Some(form),
        }
    }

    /// Opens *Clone repository…* over `opener`, or brings it forward if it's open.
    /// `known` are the repositories opened before, whose remotes name Azure DevOps
    /// organisations to list.
    pub fn clone_repository(
        &mut self,
        ctx: &egui::Context,
        opener: ViewportId,
        known: Vec<PathBuf>,
    ) {
        match &mut self.clone {
            Some(dialog) => dialog.raise(),
            None => {
                self.clone = Some(CloneDialog::new(
                    ctx,
                    self.clone_parent.as_deref(),
                    self.canned_repositories.clone(),
                    known,
                    self.read_clipboard,
                    opener,
                ));
            }
        }
    }

    fn clone_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.clone.take() else {
            return;
        };
        let answer = dialog.show(ctx, self.busy());
        if let Some(existing) = dialog.open_instead.take() {
            self.cloned = Some(existing);
            return;
        }
        match answer {
            dialogs::Answer::Primary => {
                let cloning = dialog.cloning();
                self.clone_parent = Some(cloning.parent.clone());
                let job = self.run(ctx, cloning.path(), dialog.action(), None, dialog.opener);
                job.cloned = Some(cloning.path());
            }
            dialogs::Answer::Cancel => {}
            dialogs::Answer::Open => self.clone = Some(dialog),
        }
    }

    /// Whether a fetch can start now, or why not.
    pub fn fetch_blocked(&self) -> Option<&'static str> {
        match &self.catalog {
            _ if self.busy() => Some(OPERATION_RUNNING),
            None => Some(LOADING_BRANCHES),
            Some(c) if c.remote_names.is_empty() => Some("This repository has no remote"),
            Some(_) => None,
        }
    }

    /// The running fetch, pull or push, once git has started: its output, in a window of its
    /// own that locks the others.
    fn network_window(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        let Some(live) = job.live.as_ref().filter(|l| !l.steps().is_empty()) else {
            return;
        };
        if super::remote::network_window(ctx, &job.label, live, job.opener) {
            job.cancel.cancel();
        }
    }

    /// Shows the log a dialog asked for, and runs `action` once the dialog is accepted. Returns
    /// whether the dialog stays open.
    fn answered(
        &mut self,
        ctx: &egui::Context,
        asked: Asked,
        opener: ViewportId,
        action: impl FnOnce() -> Option<Action>,
    ) -> bool {
        if let (Some(oid), Some(repo)) = (asked.show_in_log, &self.repo) {
            self.log_request = Some((repo.clone(), vec![oid], false));
        }
        match asked.answer {
            dialogs::Answer::Primary if !self.busy() => {
                if let Some(repo) = self.repo.clone()
                    && let Some(action) = action()
                {
                    self.run(ctx, repo.path.clone(), action, None, opener);
                }
                false
            }
            dialogs::Answer::Cancel => false,
            _ => true,
        }
    }

    fn pull_diverged_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialogs.pull_diverged.take() else {
            return;
        };
        let asked = Asked {
            answer: dialog.show(ctx, self.busy()),
            show_in_log: None,
        };
        let pull = || Some(Action::Pull(Box::new(dialog.pull())));
        if self.answered(ctx, asked, dialog.opener, pull) {
            self.dialogs.pull_diverged = Some(dialog);
        }
    }

    fn set_upstream_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialogs.set_upstream.take() else {
            return;
        };
        let asked = Asked {
            answer: dialog.show(ctx, self.busy()),
            show_in_log: None,
        };
        if self.answered(ctx, asked, dialog.opener, || dialog.action()) {
            self.dialogs.set_upstream = Some(dialog);
        }
    }

    fn reset_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialogs.reset.take() else {
            return;
        };
        let asked = dialog.show(ctx, self.busy());
        let repo = self.repo.clone();
        if let Some(repo) = &repo {
            if let Some((commits, exact)) = asked.log {
                self.log_request = Some((repo.clone(), commits, exact));
            }
            self.diff_requests
                .extend(asked.diffs.into_iter().map(|spec| (repo.clone(), spec)));
        }
        match asked.answer {
            dialogs::Answer::Primary if !self.busy() => {
                if let Some(repo) = &repo {
                    let action = Action::Reset(Box::new(dialog.preview.reset(dialog.mode)));
                    self.dialogs.reset_loading = None;
                    self.run(ctx, repo.path.clone(), action, None, dialog.opener);
                }
            }
            dialogs::Answer::Cancel => self.dialogs.reset_loading = None,
            _ => self.dialogs.reset = Some(dialog),
        }
    }

    fn rebase_dialog(&mut self, ctx: &egui::Context, palette: &Palette, labels: Labels<'_>) {
        let Some(mut dialog) = self.dialogs.rebase.take() else {
            return;
        };
        let asked = dialog.show(ctx, self.busy(), palette, labels);
        let rebase = || Some(Action::Rebase(Box::new(dialog.rebase())));
        if self.answered(ctx, asked, dialog.opener, rebase) {
            self.dialogs.rebase = Some(dialog);
        }
    }

    fn merge_dialog(&mut self, ctx: &egui::Context, palette: &Palette, labels: Labels<'_>) {
        let Some(mut dialog) = self.dialogs.merge.take() else {
            return;
        };
        let asked = dialog.show(ctx, self.busy(), palette, labels);
        let merge = || Some(Action::Merge(Box::new(dialog.merge())));
        if self.answered(ctx, asked, dialog.opener, merge) {
            self.dialogs.merge = Some(dialog);
        }
    }

    fn cherry_pick_dialog(&mut self, ctx: &egui::Context, palette: &Palette, labels: Labels<'_>) {
        let Some(mut dialog) = self.dialogs.cherry_pick.take() else {
            return;
        };
        let asked = dialog.show(ctx, self.busy(), palette, labels);
        let cherry_pick = || Some(Action::CherryPick(Box::new(dialog.cherry_pick())));
        if self.answered(ctx, asked, dialog.opener, cherry_pick) {
            self.dialogs.cherry_pick = Some(dialog);
        }
    }

    fn revert_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialogs.revert.take() else {
            return;
        };
        let asked = dialog.show(ctx, self.busy());
        let revert = || Some(Action::Revert(Box::new(dialog.revert())));
        if self.answered(ctx, asked, dialog.opener, revert) {
            self.dialogs.revert = Some(dialog);
        }
    }

    fn restore_stash_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialogs.restore_stash.take() else {
            return;
        };
        let asked = Asked {
            answer: dialog.show(ctx, self.busy()),
            show_in_log: None,
        };
        let restore = || Some(Action::RestoreStash(dialog.stash.oid));
        if self.answered(ctx, asked, dialog.opener, restore) {
            self.dialogs.restore_stash = Some(dialog);
        }
    }

    /// The confirmation or warning an operation came back with.
    fn loss_dialog(&mut self, ctx: &egui::Context, merged: &MergedBranches) {
        let Some(mut loss) = self.dialogs.loss.take() else {
            return;
        };
        let repo = self.repo.clone();
        // A local branch squash- or rebase-merged: where its commits' changes landed (#384).
        let landed = |name: &str, tip: Oid| -> Option<String> {
            let m = merged.of_branch(&format!("refs/heads/{name}"), tip)?;
            Some(format!("{name} was {}", m.describe(repo.as_deref()?)))
        };
        let warning = &loss.warning;
        // Nothing of the work is lost: asked as calmly as when nothing is lost at all.
        let upstream = all_upstream(warning, &landed);
        let confirmation = warning.is_confirmation() || upstream;
        let (title, button) = if upstream {
            (upstream_title(warning), "Delete")
        } else {
            loss_title(warning)
        };
        let screen = match &warning.action {
            Action::Push(_) => Screen::ForcePush,
            Action::DeleteRemoteBranches(_) => Screen::DeleteRemoteBranch,
            _ => Screen::LostWork,
        };
        let mut dialog = dialogs::Dialog::new("branch-loss", &title)
            .screen(screen)
            .opener(loss.opener)
            .raise(loss.fresh)
            .resizable();
        if upstream {
            dialog = dialog.icon(glyphs::CHECK, false);
        } else if !confirmation {
            dialog = dialog.icon(LOSS_ICON, true);
        }
        let busy = self.busy();
        let catalog = self.catalog.clone();
        let mut show_log = None;
        let mut deletes_branches = warning.deletes_branches();
        let listing_files = &mut loss.listing_files;
        let check = &mut loss.pull_request_check;
        let shown = dialog.show(ctx, |ui| {
            dialogs::fields(ui, |ui| {
                if let (Action::DeleteRemoteBranches(branches), Some(check)) =
                    (&warning.action, check.as_mut())
                {
                    super::remote::deletion_notes(ui, branches, catalog.as_deref(), check);
                }
                show_log = if upstream {
                    upstream_fields(ui, warning, &landed)
                } else {
                    loss_fields(ui, warning, listing_files, &mut deletes_branches, &landed)
                };
            });
            let commands: Vec<String> = warning.commands.iter().map(|a| command_text(a)).collect();
            dialogs::command_box(ui, &commands);
            // Changing a remote for everyone: Cancel has the focus even when nothing is lost.
            let remote = check.is_some();
            let allowed = check.as_ref().is_none_or(PullRequestCheck::allow);
            let focus = loss.fresh && (remote || !confirmation);
            let primary = dialogs::Primary::new(button)
                .enabled(!busy && allowed)
                .destructive(!confirmation)
                .focus_cancel(focus);
            dialogs::actions(ui, primary)
        });
        if let Some(commits) = show_log
            && let Some(repo) = warning.repo.clone().or_else(|| self.repo.clone())
        {
            self.log_request = Some((repo, commits, true));
        }
        if deletes_branches != loss.warning.deletes_branches() {
            loss.warning.set_deletes_branches(deletes_branches);
            ctx.request_repaint();
        }
        match shown.inner {
            dialogs::Answer::Primary => {
                let action = loss.warning.action.clone();
                self.run(ctx, loss.path, action, Some(loss.warning), loss.opener);
            }
            dialogs::Answer::Cancel => {}
            dialogs::Answer::Open if !shown.should_close() => {
                self.dialogs.loss = Some(Loss {
                    fresh: false,
                    ..loss
                })
            }
            _ => {}
        }
    }

    fn notifications(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        self.notices
            .retain(|n| n.error.is_some() || self.notice_details == Some(n.id) || now - n.at < 5.0);
        if !self.notices.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        egui::Area::new(Id::new("branch-notifications"))
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-16.0, -40.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(360.0);
                let networking = self
                    .job
                    .as_ref()
                    .and_then(|j| j.live.as_ref())
                    .is_some_and(|l| !l.steps().is_empty());
                if let Some(job) = self.job.as_ref().filter(|_| !networking) {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(&job.label);
                            if ui.button("Cancel").clicked() {
                                job.cancel.cancel();
                            }
                        });
                    });
                }
                let mut remove = None;
                let mut fetch = None;
                for n in &self.notices {
                    let color = if n.attention {
                        stuck_color(ui)
                    } else if n.error.is_some() {
                        ui.visuals().error_fg_color
                    } else if ui.visuals().dark_mode {
                        Color32::from_rgb(75, 165, 105)
                    } else {
                        Color32::from_rgb(35, 120, 65)
                    };
                    egui::Frame::popup(ui.style())
                        .stroke(egui::Stroke::new(1.0, color))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let title = RichText::new(&n.title).color(color);
                                if !n.has_details() {
                                    ui.label(title);
                                } else if ui.link(title).clicked() {
                                    self.notice_details = Some(n.id);
                                }
                                if ui.small_button("×").clicked() {
                                    remove = Some(n.id);
                                }
                            });
                            // Its first rows only, so the × stays on screen however much git
                            // printed (#283); the details dialog has the rest (#295).
                            if let Some(error) = &n.error {
                                let mut job = Arc::unwrap_or_clone(
                                    egui::WidgetText::from(error).into_layout_job(
                                        ui.style(),
                                        egui::FontSelection::Default,
                                        egui::Align::Center,
                                    ),
                                );
                                job.wrap.max_rows = NOTICE_MAX_ERROR_ROWS;
                                job.wrap.max_width = ui.available_width();
                                let galley = ui.fonts_mut(|f| f.layout_job(job));
                                let cut = galley.elided;
                                let text = ui.add(egui::Label::new(galley).sense(if cut {
                                    egui::Sense::click()
                                } else {
                                    egui::Sense::hover()
                                }));
                                if cut
                                    && (text
                                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                                        .clicked()
                                        || ui.link("Show all").clicked())
                                {
                                    self.notice_details = Some(n.id);
                                }
                            }
                            // What failed is likely out of date.
                            if n.report.suggest_fetch
                                && ui
                                    .add_enabled(self.job.is_none(), egui::Link::new("Fetch"))
                                    .clicked()
                            {
                                fetch = Some((n.id, n.path.clone()));
                            }
                        });
                }
                if let Some(id) = remove {
                    self.notices.retain(|n| n.id != id);
                }
                if let Some((id, path)) = fetch {
                    self.notices.retain(|n| n.id != id);
                    self.run(ctx, path, Action::Fetch, None, ViewportId::ROOT);
                }
            });
        if let Some(n) = self
            .notices
            .iter()
            .find(|n| Some(n.id) == self.notice_details)
        {
            let shown = dialogs::Dialog::new("git-operation-details", &n.title)
                .screen(Screen::OperationDetails)
                .width(600.0)
                .resizable()
                .show(ctx, |ui| {
                    dialogs::fields(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.weak(n.path.display().to_string());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let id = Id::new("copied-operation-details").with(n.id);
                                    let done = widgets::COPIED_GREEN;
                                    widgets::copy_button(ui, id, || details_text(n), "Copy", done);
                                },
                            );
                        });
                        if let Some(error) = &n.error {
                            let color = if n.attention {
                                stuck_color(ui)
                            } else {
                                ui.visuals().error_fg_color
                            };
                            ui.colored_label(color, error);
                        }
                        for step in &n.report.steps {
                            ui.label(RichText::new(command_text(&step.args)).monospace());
                            if !step.output.is_empty() {
                                ui.add(
                                    egui::Label::new(RichText::new(&step.output).monospace())
                                        .wrap(),
                                );
                            }
                        }
                    });
                    ui.separator();
                    dialogs::close_only(ui)
                });
            if shown.answer() == dialogs::Answer::Cancel {
                self.notice_details = None;
            }
        }
    }
}

const LOSS_ICON: glyphs::Glyph = &[glyphs::Part::Path("M12 3 2 21h20ZM12 9v5m0 3v1")];

/// The loss dialog's title, and its button's label.
fn loss_title(warning: &Warning) -> (String, &'static str) {
    let commits = plural(warning.commits.len(), "commit");
    let confirmation = warning.is_confirmation();
    match &warning.action {
        Action::DeleteBranches(_) => {
            let what = match warning.deletions.as_slice() {
                [one] => format!("branch {}", one.name),
                many => format!("{} local branches", many.len()),
            };
            (
                format!("Delete {what} and lose {commits}?"),
                "Delete anyway",
            )
        }
        Action::DeleteWorktrees { .. } => {
            let what = match warning.deletions.as_slice() {
                [one] => format!("worktree {}", one.name),
                many => format!("{} worktrees", many.len()),
            };
            let changed: usize = warning.deletions.iter().map(|d| d.files.len()).sum();
            let files = plural(changed, "changed file");
            let refused = warning.deletions.iter().any(|d| d.refusal.is_some());
            let cost = match (changed == 0, warning.commits.is_empty()) {
                (false, false) => format!(" and lose {files} and {commits}"),
                (false, true) => format!(" and lose {files}"),
                (true, false) => format!(" and lose {commits}"),
                (true, true) if refused => " anyway".to_owned(),
                (true, true) => String::new(),
            };
            let button = if confirmation {
                "Delete"
            } else {
                "Delete anyway"
            };
            (format!("Delete {what}{cost}?"), button)
        }
        Action::DeleteRemoteBranches(_) => {
            let what = match warning.deletions.as_slice() {
                [one] => format!("remote branch {}", one.name),
                many => format!("{} remote branches", many.len()),
            };
            if confirmation {
                (format!("Delete {what}?"), "Delete")
            } else {
                (
                    format!("Delete {what} and lose {commits}?"),
                    "Delete anyway",
                )
            }
        }
        Action::Push(push) if confirmation => (
            format!("Force push {} to {}?", push.branch, push.target()),
            "Force push",
        ),
        Action::Push(push) => (
            format!(
                "Force push {} to {} and lose {commits}?",
                push.branch,
                push.target()
            ),
            "Force push anyway",
        ),
        _ => (
            format!(
                "Switch {} and lose {}?",
                if matches!(warning.action, Action::Detach(_)) {
                    "to a detached HEAD"
                } else {
                    "branches"
                },
                plural(warning.commits.len(), "detached commit")
            ),
            "Switch anyway",
        ),
    }
}

/// Whether `warning` deletes local branches and every commit it would lose is a deleted
/// branch's whose changes landed upstream, as `landed` says (#384).
fn all_upstream(warning: &Warning, landed: &dyn Fn(&str, Oid) -> Option<String>) -> bool {
    if !matches!(warning.action, Action::DeleteBranches(_)) || warning.commits.is_empty() {
        return false;
    }
    let mut covered: HashSet<Oid> = HashSet::new();
    for d in &warning.deletions {
        if d.refusal.is_some() || !d.files.is_empty() {
            return false;
        }
        if d.commits.is_empty() {
            continue;
        }
        if d.tip().and_then(|tip| landed(&d.name, tip)).is_none() {
            return false;
        }
        covered.extend(&d.commits);
    }
    warning.commits.iter().all(|c| covered.contains(c))
}

/// The title deleting branches whose changes are all upstream asks with.
fn upstream_title(warning: &Warning) -> String {
    match warning.deletions.as_slice() {
        [one] => format!("Delete branch {}?", one.name),
        many => format!("Delete {} local branches?", many.len()),
    }
}

/// What deleting branches whose changes are all upstream takes: where each one landed, and
/// their own commits, which no branch will reach any more. Returns the commits whose *Show in
/// log* was clicked.
fn upstream_fields(
    ui: &mut Ui,
    warning: &Warning,
    landed: &dyn Fn(&str, Oid) -> Option<String>,
) -> Option<Vec<Oid>> {
    let merged: Vec<String> = warning
        .deletions
        .iter()
        .filter(|d| !d.commits.is_empty())
        .filter_map(|d| landed(&d.name, d.tip()?))
        .collect();
    for note in &merged {
        ui.label(format!("{note}."));
    }
    ui.label(if merged.len() == 1 {
        "Its changes are already upstream, so no work is lost."
    } else {
        "Their changes are already upstream, so no work is lost."
    });
    let mut show_log = None;
    egui::Frame::new()
        .fill(widgets::tones(ui).seg_bg)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(plural(warning.commits.len(), "commit"));
                if ui.link("Show in log").clicked() {
                    show_log = Some(warning.commits.clone());
                }
            })
        });
    show_log
}

/// What the loss dialog says is lost: by each deletion, its changed files and its commits, and
/// the commits lost or replaced. `landed` says where a merged local branch's changes are, given
/// its name and tip. Returns the commits whose *Show in log* was clicked.
fn loss_fields(
    ui: &mut Ui,
    warning: &Warning,
    listing_files: &mut HashSet<usize>,
    deletes_branches: &mut bool,
    landed: &dyn Fn(&str, Oid) -> Option<String>,
) -> Option<Vec<Oid>> {
    let mut show_log = None;
    let worktrees = matches!(warning.action, Action::DeleteWorktrees { .. });
    match warning.deletions.as_slice() {
        [one] if worktrees => {
            if let Some(path) = &one.path {
                ui.label(format!(
                    "The folder {} and everything in it will be deleted.",
                    path.display()
                ));
            }
        }
        _ if worktrees => {
            ui.label("These folders and everything in them will be deleted.");
        }
        _ => {}
    }
    let owned: Vec<&str> = warning
        .deletions
        .iter()
        .filter_map(|d| d.branch.as_ref())
        .map(|b| b.name.as_str())
        .collect();
    if worktrees && !owned.is_empty() {
        let label = match owned.as_slice() {
            [one] => format!("Also delete local branch {one}"),
            _ => "Also delete their local branches".to_owned(),
        };
        ui.checkbox(deletes_branches, label);
    }
    let deletes_branches = *deletes_branches;
    let several = warning.deletions.len() > 1;
    let unreachable = match &warning.action {
        Action::Push(push) => format!(
            "These commits on {} are not on {}.",
            push.remote_branch(),
            push.branch
        ),
        _ => {
            "These commits are not reachable from any surviving branch, tag or worktree.".to_owned()
        }
    };
    let row = |ui: &mut Ui, content: &mut dyn FnMut(&mut Ui)| {
        egui::Frame::new()
            .fill(widgets::tones(ui).seg_bg)
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| ui.horizontal(|ui| content(ui)));
    };
    // What one deletion loses: one row each for its files and its commits.
    let mut losses = |ui: &mut Ui, i: usize, d: &Deletion| {
        if let Some(refusal) = &d.refusal {
            ui.label("Git refused to delete it:");
            ui.label(RichText::new(refusal).monospace());
        }
        if !d.files.is_empty() {
            let listed = listing_files.contains(&i);
            row(ui, &mut |ui| {
                ui.label(plural(d.files.len(), "changed file"));
                if ui.link(if listed { "Hide" } else { "Show" }).clicked()
                    && !listing_files.remove(&i)
                {
                    listing_files.insert(i);
                }
            });
            if listed {
                for file in &d.files {
                    ui.label(RichText::new(file).monospace());
                }
            }
        }
        let branch = d
            .branch
            .as_ref()
            .filter(|b| deletes_branches && !b.commits.is_empty());
        if !several && (!d.commits.is_empty() || branch.is_some()) {
            ui.label(&unreachable);
        }
        if !d.commits.is_empty() {
            row(ui, &mut |ui| {
                ui.label(plural(d.commits.len(), "commit"));
                if ui.link("Show in log").clicked() {
                    show_log = Some(d.commits.clone());
                }
            });
            let note = match (&warning.action, d.tip()) {
                (Action::DeleteBranches(_), Some(tip)) => landed(&d.name, tip),
                _ => None,
            };
            if let Some(note) = note {
                ui.weak(format!("Their changes are upstream: {note}."));
            }
        }
        if let Some(b) = branch {
            row(ui, &mut |ui| {
                ui.label(format!(
                    "{} on {}",
                    plural(b.commits.len(), "commit"),
                    b.name
                ));
                if ui.link("Show in log").clicked() {
                    show_log = Some(b.commits.clone());
                }
            });
            if let Some(note) = landed(&b.name, b.tip) {
                ui.weak(format!("Their changes are upstream: {note}."));
            }
        }
    };
    if several {
        for (i, d) in warning.deletions.iter().enumerate() {
            ui.separator();
            let name = d
                .path
                .as_ref()
                .map_or(d.name.clone(), |p| p.display().to_string());
            ui.label(RichText::new(name).strong());
            losses(ui, i, d);
        }
        if !warning.commits.is_empty() {
            ui.separator();
            ui.label("The commits are not reachable from any surviving branch, tag or worktree.");
        }
    } else if let [one] = warning.deletions.as_slice() {
        losses(ui, 0, one);
    } else if !warning.commits.is_empty() {
        ui.label(&unreachable);
        row(ui, &mut |ui| {
            ui.label(plural(warning.commits.len(), "commit"));
            if ui.link("Show in log").clicked() {
                show_log = Some(warning.commits.clone());
            }
        });
    }
    // A force push's copies: rebased, amended or resolved on the branch.
    if !warning.replaced.is_empty() {
        row(ui, &mut |ui| {
            ui.label(plural(warning.replaced.len(), "replaced commit"))
                .on_hover_text("The branch has a copy of each");
            if ui.link("Show in log").clicked() {
                show_log = Some(warning.replaced.clone());
            }
        });
    }
    show_log
}

/// What the details dialog of `n` shows, to copy: its error, then each command with what it
/// printed.
fn details_text(n: &Notice) -> String {
    let mut text = n.error.clone().unwrap_or_default();
    for step in &n.report.steps {
        for part in [command_text(&step.args), step.output.clone()] {
            if !part.is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&part);
            }
        }
    }
    text
}

impl Drop for Tool {
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use eframe::egui::{self, Pos2};
    use parterre_core::Oid;
    use parterre_core::branches::{Action, BranchTip, Report, Step};

    use super::super::tool_harness::{Harness, git, load, menu, write};
    use super::Request;

    /// main: base → tip. Worktrees in `others`: `a` at base with a changed file, `b` at the
    /// tip, and `c` detached at a commit of its own on base.
    fn repository() -> (tempfile::TempDir, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let others = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        write(p, "file", "base\n");
        git(p, &["add", "file"]);
        git(p, &["commit", "-q", "-m", "base"]);
        let add = |name: &str, extra: &[&str]| {
            let path = others.path().join(name);
            let mut args = vec!["worktree", "add", "-q"];
            args.extend(extra);
            let path = path.to_string_lossy().into_owned();
            args.push(&path);
            args.push("HEAD");
            git(p, &args);
        };
        add("a", &["-b", "a"]);
        write(&others.path().join("a"), "file", "changed\n");
        add("c", &["--detach"]);
        git(
            &others.path().join("c"),
            &["commit", "-q", "--allow-empty", "-m", "only c"],
        );
        git(p, &["commit", "-q", "--allow-empty", "-m", "tip"]);
        add("b", &["-b", "b"]);
        (dir, others)
    }

    fn rev(dir: &Path, r: &str) -> Oid {
        Oid::from_hex(&git(dir, &["rev-parse", r])).unwrap()
    }

    fn names(request: &Option<Request>) -> Vec<String> {
        match request {
            Some(Request::Run(Action::DeleteWorktrees { paths, .. })) => paths
                .iter()
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect(),
            other => panic!("not a deletion: {other:?}"),
        }
    }

    #[test]
    fn a_group_of_worktree_nodes_deletes_them_all_with_one_item() {
        let (dir, others) = repository();
        let (repo, catalog) = load(dir.path());
        let (base, tip) = (rev(dir.path(), "HEAD~1"), rev(dir.path(), "HEAD"));
        let c = rev(&others.path().join("c"), "HEAD");
        let item = |commit, group: &[Oid], click| {
            menu(
                |ui| super::row_node_menu(ui, &repo, commit, group, Some(&catalog), false, true),
                click,
            )
        };
        let (_, asked) = item(tip, &[tip, base, c], Some("Delete 3 worktrees"));
        assert_eq!(names(&asked), ["b", "a", "c"]);
        // One node alone: its own worktrees, as before.
        let (texts, _) = item(base, &[base], None);
        assert!(texts.iter().any(|t| t == "Delete worktree a"), "{texts:?}");
        // A node with no worktree to delete in the group: only the clicked node's.
        let none =
            Oid::from_hex(&git(dir.path(), &["commit-tree", "-m", "x", "HEAD^{tree}"])).unwrap();
        let (texts, _) = item(base, &[base, none], None);
        assert!(
            !texts.iter().any(|t| t.starts_with("Delete 2")),
            "{texts:?}"
        );
        assert!(texts.iter().any(|t| t == "Delete worktree a"), "{texts:?}");
        // The open, main worktree at the tip is never among them.
        let (_, asked) = item(tip, &[tip, base], Some("Delete 2 worktrees"));
        assert_eq!(names(&asked), ["b", "a"]);
    }

    #[test]
    fn a_locked_worktree_greys_the_group_item() {
        let (dir, others) = repository();
        git(
            dir.path(),
            &[
                "worktree",
                "lock",
                &others.path().join("a").to_string_lossy(),
            ],
        );
        let (repo, catalog) = load(dir.path());
        let (base, tip) = (rev(dir.path(), "HEAD~1"), rev(dir.path(), "HEAD"));
        let (texts, asked) = menu(
            |ui| super::row_node_menu(ui, &repo, tip, &[tip, base], Some(&catalog), false, true),
            Some("Delete 2 worktrees"),
        );
        assert!(texts.iter().any(|t| t == "Delete 2 worktrees"), "{texts:?}");
        assert!(asked.is_none(), "greyed out");
    }

    #[test]
    fn deleting_several_shows_what_each_loses_then_deletes_them_all() {
        let (dir, others) = repository();
        let mut h = Harness::new(dir);
        let paths: Vec<_> = ["a", "b", "c"]
            .iter()
            .map(|n| others.path().join(n))
            .collect();
        let catalog = load(h.path()).1;
        let listed: Vec<_> = paths
            .iter()
            .map(|p| {
                catalog
                    .worktrees
                    .iter()
                    .find(|w| parterre_core::worktree_folder::same_path_on_disk(&w.path, p))
                    .unwrap()
                    .path
                    .clone()
            })
            .collect();
        let c = rev(&paths[2], "HEAD");
        h.ask(
            Request::Run(Action::DeleteWorktrees {
                paths: listed,
                branches: false,
            }),
            "Delete 3 worktrees and lose 1 changed file and 1 commit?",
        );
        assert!(h.shows("These folders and everything in them will be deleted."));
        for p in &paths {
            assert!(h.shows_part(&p.file_name().unwrap().to_string_lossy()));
        }
        assert!(
            h.shows("1 changed file") && h.shows("1 commit"),
            "{:?}",
            h.texts
        );
        h.click("Show");
        assert!(h.shows("file") && h.shows("Hide"), "{:?}", h.texts);
        h.click("Show in log");
        let (_, commits, exact) = h.tool.log_request.take().expect("the log is asked for");
        assert_eq!((commits, exact), (vec![c], true));
        assert!(paths.iter().all(|p| p.exists()), "asking deletes nothing");
        h.click("Delete anyway");
        h.until("all three are gone", |_| paths.iter().all(|p| !p.exists()));
    }

    #[test]
    fn deleting_several_that_lose_nothing_is_a_confirmation() {
        let (dir, others) = repository();
        let a = others.path().join("a");
        git(&a, &["checkout", "--", "file"]);
        let mut h = Harness::new(dir);
        let catalog = load(h.path()).1;
        let listed: Vec<_> = catalog
            .worktrees
            .iter()
            .filter(|w| w.name() == "a" || w.name() == "b")
            .map(|w| w.path.clone())
            .collect();
        h.ask(
            Request::Run(Action::DeleteWorktrees {
                paths: listed,
                branches: false,
            }),
            "Delete 2 worktrees?",
        );
        assert!(!h.shows("Delete anyway") && h.shows("Delete"));
        h.click("Delete");
        h.until("both are gone", |_| {
            !a.exists() && !others.path().join("b").exists()
        });
    }

    /// The worktree `name` in `others`, as the catalogue lists it.
    fn listed(dir: &Path, others: &Path, name: &str) -> PathBuf {
        load(dir)
            .1
            .worktrees
            .into_iter()
            .find(|w| {
                parterre_core::worktree_folder::same_path_on_disk(&w.path, &others.join(name))
            })
            .unwrap()
            .path
    }

    fn worktree_deletion(path: PathBuf) -> Request {
        Request::Run(Action::DeleteWorktrees {
            paths: vec![path],
            branches: false,
        })
    }

    #[test]
    fn a_worktree_s_branch_goes_with_it_only_when_ticked() {
        let (dir, others) = repository();
        let b = others.path().join("b");
        git(&b, &["commit", "-q", "--allow-empty", "-m", "only on b"]);
        let only = rev(&b, "HEAD");
        let path = listed(dir.path(), others.path(), "b");
        let mut h = Harness::new(dir);
        h.ask(worktree_deletion(path.clone()), "Delete worktree b?");
        assert!(
            h.tool.reload.is_none(),
            "asking changes nothing to load again"
        );
        // Unticked, the branch keeps its commit: a confirmation.
        assert!(h.shows("Delete") && !h.shows("Delete anyway"));
        h.click("Also delete local branch b");
        h.until("the branch's commit is listed", |h| {
            h.shows("Delete worktree b and lose 1 commit?")
                && h.shows("1 commit on b")
                && h.shows("Delete anyway")
        });
        h.click("Show in log");
        let (repo, commits, _) = h.tool.log_request.take().expect("the log is asked for");
        assert_eq!(commits, [only]);
        // In the graph on screen.
        assert!(repo.lookup(&only).is_some());
        // Unticked again, it's the confirmation again.
        h.click("Also delete local branch b");
        h.until("back to the confirmation", |h| {
            h.shows("Delete worktree b?")
        });
        h.key(egui::Key::Escape);
        h.until("closed", |h| !h.shows("Delete worktree b?"));
        // Nothing is remembered: asked again, it's unticked.
        h.ask(worktree_deletion(path), "Delete worktree b?");
        h.click("Also delete local branch b");
        h.until("ticked", |h| h.shows("Delete anyway"));
        h.click("Delete anyway");
        h.until("both are gone", |h| {
            !b.exists() && git(h.path(), &["branch", "--list", "b"]).is_empty()
        });
    }

    #[test]
    fn a_detached_worktree_offers_no_branch_to_delete() {
        let (dir, others) = repository();
        let path = listed(dir.path(), others.path(), "c");
        let mut h = Harness::new(dir);
        h.ask(
            worktree_deletion(path),
            "Delete worktree c and lose 1 commit?",
        );
        assert!(!h.shows_part("Also delete"), "{:?}", h.texts);
    }

    /// main: base → tip; `one` and `two` at base, `three` at a commit of its own on base,
    /// `merged` at the tip.
    fn branches() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["commit", "-q", "--allow-empty", "-m", "base"]);
        git(p, &["branch", "one"]);
        git(p, &["branch", "two"]);
        git(p, &["switch", "-q", "-c", "three"]);
        git(p, &["commit", "-q", "--allow-empty", "-m", "only three"]);
        git(p, &["switch", "-q", "main"]);
        git(p, &["commit", "-q", "--allow-empty", "-m", "tip"]);
        git(p, &["branch", "merged"]);
        dir
    }

    fn branch_names(request: &Option<Request>) -> Vec<String> {
        match request {
            Some(Request::Run(Action::DeleteBranches(branches))) => {
                branches.iter().map(|b| b.name.clone()).collect()
            }
            other => panic!("not a deletion: {other:?}"),
        }
    }

    #[test]
    fn a_group_of_branch_nodes_deletes_them_all_with_one_item() {
        let dir = branches();
        let (repo, catalog) = load(dir.path());
        let p = dir.path();
        let (base, three, tip) = (rev(p, "one"), rev(p, "three"), rev(p, "main"));
        let item = |commit, group: &[Oid], click| {
            menu(
                |ui| super::row_node_menu(ui, &repo, commit, group, Some(&catalog), false, false),
                click,
            )
        };
        let (_, asked) = item(three, &[three, base, tip], Some("Delete 4 local branches"));
        assert_eq!(branch_names(&asked), ["three", "one", "two", "merged"]);
        let (texts, _) = item(base, &[base], None);
        assert!(texts.iter().any(|t| t == "Delete branch"), "{texts:?}");
        // A node with no branch in the group: only the clicked node's.
        let none = Oid::from_hex(&git(p, &["commit-tree", "-m", "x", "HEAD^{tree}"])).unwrap();
        let (texts, _) = item(three, &[three, none], Some("Delete branch three"));
        assert!(
            !texts.iter().any(|t| t.starts_with("Delete 2")),
            "{texts:?}"
        );
    }

    #[test]
    fn deleting_several_branches_warns_with_each_ones_commits_then_deletes_them_all() {
        let dir = branches();
        let mut h = Harness::new(dir);
        let three = h.rev("three");
        let tip = |name: &str| BranchTip {
            name: name.into(),
            tip: h.rev(name),
        };
        let action = Action::DeleteBranches(vec![tip("one"), tip("three"), tip("merged")]);
        h.ask(
            Request::Run(action),
            "Delete 3 local branches and lose 1 commit?",
        );
        for name in ["one", "three", "merged", "1 commit"] {
            assert!(h.shows(name), "{name}: {:?}", h.texts);
        }
        h.click("Show in log");
        let (_, commits, _) = h.tool.log_request.take().expect("the log is asked for");
        assert_eq!(commits, [three]);
        h.click("Delete anyway");
        h.until("all three are gone", |h| {
            git(h.path(), &["branch", "--format=%(refname:short)"]) == "main\ntwo"
        });
    }

    /// `name` squash-merged as main's tip, as the forge or the patch ids would say (#384).
    fn squash_merged(h: &mut Harness, name: &str) {
        let answer = parterre_core::merged::Merged {
            how: Some(parterre_core::merged::How::Squash),
            landed: Some(h.rev("main")),
            pull_request: None,
        };
        let tip = h.rev(name);
        h.merged.insert(&format!("refs/heads/{name}"), tip, answer);
    }

    /// #384: its commits' changes are upstream, so nothing is lost: an assurance, not a warning.
    #[test]
    fn deleting_a_squash_merged_branch_is_a_calm_confirmation() {
        let mut h = Harness::new(branches());
        squash_merged(&mut h, "three");
        let action = Action::DeleteBranches(vec![BranchTip {
            name: "three".into(),
            tip: h.rev("three"),
        }]);
        h.ask(Request::Run(action), "Delete branch three?");
        assert!(
            h.shows_part("three was squash-merged as") && h.shows("1 commit"),
            "{:?}",
            h.texts
        );
        assert!(!h.shows("Delete anyway") && !h.shows_part("lose"));
        h.click("Delete");
        h.until("it is gone", |h| {
            !git(h.path(), &["branch", "--format=%(refname:short)"]).contains("three")
        });
    }

    /// A merged branch with one that isn't: the other's commits are still lost.
    #[test]
    fn a_merged_branch_with_an_unmerged_one_is_still_a_warning() {
        let mut h = Harness::new(branches());
        squash_merged(&mut h, "three");
        let tree = git(h.path(), &["rev-parse", "main^{tree}"]);
        let only = git(
            h.path(),
            &["commit-tree", &tree, "-p", "main", "-m", "only"],
        );
        git(h.path(), &["branch", "four", &only]);
        let tip = |name: &str| BranchTip {
            name: name.into(),
            tip: h.rev(name),
        };
        let action = Action::DeleteBranches(vec![tip("three"), tip("four")]);
        h.ask(
            Request::Run(action),
            "Delete 2 local branches and lose 2 commits?",
        );
        assert!(h.shows("Delete anyway"), "{:?}", h.texts);
    }

    #[test]
    fn several_branches_that_lose_nothing_go_without_a_question() {
        let mut h = Harness::new(branches());
        let tip = |name: &str| BranchTip {
            name: name.into(),
            tip: h.rev(name),
        };
        let action = Action::DeleteBranches(vec![tip("one"), tip("merged")]);
        let ctx = h.ctx.clone();
        h.tool
            .request(&ctx, Request::Run(action), egui::ViewportId::ROOT);
        h.until("both are gone", |h| {
            git(h.path(), &["branch", "--format=%(refname:short)"]) == "main\nthree\ntwo"
        });
        h.until("the notification", |h| {
            h.shows("Delete 2 local branches") && !h.shows("Cancel")
        });
        assert!(!h.shows("Delete anyway"));
    }

    /// A failed operation's notification with `error`, placed and sized.
    fn failed(error: &str) -> Harness {
        let mut h = Harness::new(branches());
        let (ctx, path) = (h.ctx.clone(), h.path().to_owned());
        h.tool.notice(
            &ctx,
            path,
            "Switch to main failed".into(),
            Default::default(),
            Some(error.into()),
        );
        for _ in 0..5 {
            h.frame();
        }
        h
    }

    fn opens_details(h: &mut Harness, click: Pos2) {
        h.click_at(click, 1);
        h.until("the details", |h| h.shows("Close"));
        h.click("Close");
        h.until("the details close", |h| !h.shows("Close"));
    }

    #[test]
    fn a_long_error_keeps_its_notification_in_the_window() {
        let output: String = (0..200)
            .map(|i| format!("\tweb/Scripts/controllers/file{i}.js\n"))
            .collect();
        let error = format!("error: would be overwritten:\n{output}");
        let mut h = failed(&error);
        assert!(h.at("×").y > 0.0 && h.at("Switch to main failed").y > 0.0);
        // The rest is a click away: on the title, on Show all, or on the cut text (#295).
        for click in ["Switch to main failed", "Show all", error.as_str()] {
            let at = h.at(click);
            opens_details(&mut h, at);
        }
        h.click("×");
        assert!(!h.shows("Switch to main failed"), "{:?}", h.texts);
    }

    #[test]
    fn a_short_error_shows_whole_without_show_all() {
        let h = failed("error: pathspec 'x' did not match");
        assert!(h.shows("error: pathspec 'x' did not match"));
        assert!(!h.shows("Show all"), "{:?}", h.texts);
    }

    #[test]
    fn the_details_copy_the_error_then_each_command_and_its_output() {
        let step = |args: &[&str], output: &str| Step {
            args: args.iter().map(|a| a.to_string()).collect(),
            output: output.into(),
            success: false,
        };
        let n = super::Notice {
            id: 1,
            title: "Switch to main failed".into(),
            path: "/repo".into(),
            report: Report {
                steps: vec![
                    step(&["stash"], ""),
                    step(
                        &["switch", "main"],
                        "error: would be overwritten:\n\ta.js\n",
                    ),
                ],
                ..Default::default()
            },
            error: Some("error: would be overwritten:\n\ta.js\n".into()),
            at: 0.0,
            attention: false,
        };
        assert_eq!(
            super::details_text(&n),
            "error: would be overwritten:\n\ta.js\ngit stash\ngit switch main\n\
             error: would be overwritten:\n\ta.js\n"
        );
    }

    /// F5 says it reloaded: once, however often it is pressed, and for a few seconds.
    #[test]
    fn a_reload_notification_shows_once_and_goes() {
        let (dir, _others) = repository();
        let mut h = Harness::new(dir);
        let path = h.path().to_owned();
        for _ in 0..3 {
            let ctx = h.ctx.clone();
            h.tool.inform(&ctx, path.clone(), "Reloaded");
            h.frame();
        }
        let shown = h.texts.iter().filter(|(t, _)| t == "Reloaded").count();
        assert_eq!(shown, 1, "{:?}", h.texts);
        h.time += 6.0;
        h.frame();
        assert!(!h.shows("Reloaded"));
    }
}
