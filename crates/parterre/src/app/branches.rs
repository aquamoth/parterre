//! The local-branch and worktree tools. Graph nodes and log rows share the same menu and
//! controller. Slow Git queries and all mutations run on workers; forms retain their selected
//! commit. Its dialogs are modeless windows, opened over the window they were asked from.

use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use eframe::egui::{self, Color32, Id, RichText, Ui, ViewportId, vec2};
use parterre_core::banner::BannerTimer;
use parterre_core::branches::{
    Action, AddWorktree, BranchTip, Branches, Catalog, Checkout, Create, CreateDraft, Outcome,
    Report, Warning, command_text,
};
use parterre_core::file_diff::FileDiffSpec;
use parterre_core::remote::Live;
use parterre_core::reset::{Mode, Preview};
use parterre_core::worktree_folder;
use parterre_core::{Oid, RefKind, Repo};
use parterre_util::CancelTree;

use super::cherry_pick::CherryPickDialog;
use super::merge::MergeDialog;
use super::rebase::{RebaseDialog, stuck_color};
use super::remote::{PullDialog, SetUpstreamDialog};
use super::reset::ResetDialog;
use super::revert::{RestoreDialog, RevertDialog};
use crate::theme::Palette;
use crate::usage::{self, Screen};
use crate::{dialogs, menu, widgets};
use parterre_core::cherry_pick;
use parterre_core::merge;
use parterre_core::rebase;
use parterre_core::revert;
use parterre_core::revgraph::GraphOptions;

#[derive(Clone, Debug)]
pub enum Request {
    Create {
        start: Oid,
        track: Option<String>,
        switch: bool,
    },
    /// The creation form with its worktree section.
    AddWorktree {
        start: Oid,
    },
    /// Make this worktree the open one. Nothing runs in git.
    GoTo(PathBuf),
    Run(Action),
    /// The dialog for resetting the open worktree's branch to a commit, with a mode picked
    /// rather than the default (for screenshots).
    Reset {
        target: Oid,
        mode: Option<Mode>,
    },
    /// The confirmation for rebasing the open worktree's branch onto `onto`, which the command
    /// names by `target`: a branch's name, or the full hash.
    Rebase {
        onto: Oid,
        target: String,
    },
    /// The dialog for merging `theirs` into the open worktree's branch, which the command
    /// names by `target`: a branch's name, or the full hash.
    Merge {
        theirs: Oid,
        target: String,
    },
    /// The dialog for merging the open worktree's branch into local branch `into`, as a pull
    /// request does.
    MergeInto {
        into: String,
    },
    /// The confirmation for cherry-picking `picks` onto the open worktree's branch; `name` is
    /// what the graph's menu named them by.
    CherryPick {
        picks: cherry_pick::Picks,
        name: Option<String>,
    },
    /// The dialog for reverting `commit` on the open worktree's branch.
    Revert {
        commit: Oid,
    },
    /// The dialog for setting local branch `branch`'s upstream.
    SetUpstream {
        branch: String,
    },
}

/// One target gets a direct named item; several get the existing app's submenu treatment.
/// The worktree section follows the branch section while `worktrees` are shown. `group` is the
/// selection the node is in: when every node of several has worktrees to delete, one item
/// deletes them all.
pub fn node_menu(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    group: &[Oid],
    catalog: Option<&Catalog>,
    busy: bool,
    worktrees: bool,
) -> Option<Request> {
    menu_for(ui, repo, commit, group, None, catalog, busy, worktrees)
}

/// [`node_menu`] for a row of the log, where *Cherry-pick* takes the `selection`, as listed,
/// rather than everything the branch lacks.
pub fn row_node_menu(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    selection: &[Oid],
    catalog: Option<&Catalog>,
    busy: bool,
    worktrees: bool,
) -> Option<Request> {
    menu_for(
        ui,
        repo,
        commit,
        selection,
        Some(selection),
        catalog,
        busy,
        worktrees,
    )
}

#[allow(clippy::too_many_arguments)]
fn menu_for(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    group: &[Oid],
    selection: Option<&[Oid]>,
    catalog: Option<&Catalog>,
    busy: bool,
    worktrees: bool,
) -> Option<Request> {
    let branch = branch_section(ui, repo, commit, group, selection, catalog, busy);
    let remote = catalog.and_then(|c| super::remote::section(ui, repo, commit, c, busy));
    let worktree = worktrees
        .then(|| worktree_section(ui, commit, group, catalog, busy))
        .flatten();
    branch.or(remote).or(worktree)
}

pub(super) fn loading_reason(busy: bool) -> &'static str {
    if busy {
        "A Git operation is running"
    } else {
        "Loading branch information"
    }
}

fn branch_section(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    group: &[Oid],
    selection: Option<&[Oid]>,
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
        .on_disabled_hover_text(loading_reason(busy))
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
    let refs: Vec<_> = repo
        .refs
        .iter()
        .filter(|r| repo.commit(r.target).oid == commit)
        .filter(|r| match r.kind {
            RefKind::LocalBranch => catalog
                .locals
                .iter()
                .any(|b| b.name == r.name && b.tip == commit),
            RefKind::RemoteBranch => catalog
                .remotes
                .iter()
                .any(|b| b.name == r.name && b.tip == commit),
            _ => false,
        })
        .collect();
    let mut switches: Vec<Target> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if catalog.has_working_tree {
        for r in &refs {
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
    let local_targets: std::collections::HashSet<_> = switches
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
    // A stuck worktree switches nowhere until its operation is finished with git.
    let stuck = catalog.stuck().map(|s| s.reason());
    // The commit itself, detached, last.
    if catalog.has_working_tree && !(catalog.current.is_none() && catalog.head == Some(commit)) {
        let hex = commit.to_hex();
        let short = &hex[..repo.abbrev_len.clamp(4, hex.len())];
        switches.push((
            format!("{short} (detached)"),
            Request::Run(Action::Detach(commit)),
            None,
        ));
    }
    if let Some(reason) = &stuck {
        for target in &mut switches {
            target.2 = Some(reason.clone());
        }
    }
    target_menu(ui, "Switch to", &switches, busy, &mut request);
    // The local branches at a commit that are checked out nowhere.
    let deletable = |commit: Oid| {
        let mut found: Vec<BranchTip> = repo
            .refs
            .iter()
            .filter(|r| {
                r.kind == RefKind::LocalBranch
                    && repo.commit(r.target).oid == commit
                    && catalog
                        .locals
                        .iter()
                        .any(|b| b.name == r.name && b.tip == commit)
                    && catalog.current.as_ref() != Some(&r.name)
                    && !catalog.occupied.contains_key(&r.name)
            })
            .map(|r| BranchTip {
                name: r.name.clone(),
                tip: commit,
            })
            .collect();
        found.sort_by(|a, b| a.name.cmp(&b.name));
        found
    };
    if group.len() > 1 && group.iter().all(|&c| !deletable(c).is_empty()) {
        let all: Vec<BranchTip> = group.iter().flat_map(|&c| deletable(c)).collect();
        let names: Vec<String> = all.iter().map(|b| b.name.clone()).collect();
        let label = format!("Delete {} local branches", all.len());
        let delete = Request::Run(Action::DeleteBranches(all));
        all_item(ui, label, &names, delete, None, busy, &mut request);
    } else {
        let deletions: Vec<Target> = deletable(commit)
            .into_iter()
            .map(|b| {
                (
                    b.name.clone(),
                    Request::Run(Action::DeleteBranches(vec![b])),
                    None,
                )
            })
            .collect();
        target_menu(ui, "Delete branch", &deletions, busy, &mut request);
    }
    rebase_targets(ui, repo, commit, &refs, catalog, busy, &mut request);
    merge_targets(ui, repo, commit, &refs, catalog, busy, &mut request);
    merge_into_targets(ui, repo, commit, &refs, catalog, busy, &mut request);
    let picked = match selection {
        Some(selection) => cherry_pick_selection(ui, repo, selection, catalog, busy),
        None => cherry_pick_lacking(ui, repo, commit, &refs, catalog, busy),
    };
    if let Some(pick) = picked {
        request = Some(pick);
    }
    if let Some(reset) = reset_item(ui, commit, Some(catalog), busy) {
        request = Some(reset);
    }
    // A log row's: commits are acted on one by one there.
    if selection.is_some()
        && let Some(revert) = revert_item(ui, repo, commit, catalog, busy)
    {
        request = Some(revert);
    }
    request
}

/// *Rebase main onto X* for each branch on the node and, last, the commit itself, when it
/// would really rebase; greyed out while the open worktree is stuck. The log's rows have it
/// too, through [`node_menu`].
fn rebase_targets(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    refs: &[&parterre_core::GitRef],
    catalog: &Catalog,
    busy: bool,
    request: &mut Option<Request>,
) {
    let mut names: Vec<&str> = refs
        .iter()
        .filter(|r| !r.name.ends_with("/HEAD"))
        .map(|r| r.name.as_str())
        .filter(|name| catalog.current.as_deref() != Some(*name))
        .collect();
    names.sort_unstable();
    names.dedup();
    let stuck = catalog.stuck().map(|s| s.reason());
    let branch = match &stuck {
        Some(_) => stuck_branch(catalog),
        None => match rebase::offered(repo, catalog, commit) {
            Some(branch) => branch.to_owned(),
            None => return,
        },
    };
    let mut targets: Vec<Target> = names
        .iter()
        .map(|name| {
            (
                (*name).to_owned(),
                Request::Rebase {
                    onto: commit,
                    target: (*name).to_owned(),
                },
                stuck.clone(),
            )
        })
        .collect();
    // The commit itself, last, as Switch to has it: for nodes with no branch on them, and
    // for log rows.
    targets.push((
        commit.short(repo.abbrev_len.max(7)),
        Request::Rebase {
            onto: commit,
            target: commit.to_hex(),
        },
        stuck,
    ));
    target_menu(
        ui,
        &format!("Rebase {branch} onto"),
        &targets,
        busy,
        request,
    );
}

/// *Merge X into main…* for each branch on the node and, last, the commit itself, when there's
/// something to merge; greyed out while the open worktree is stuck. The log's rows have it
/// too, through [`node_menu`].
fn merge_targets(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    refs: &[&parterre_core::GitRef],
    catalog: &Catalog,
    busy: bool,
    request: &mut Option<Request>,
) {
    let mut names: Vec<&str> = refs
        .iter()
        .filter(|r| !r.name.ends_with("/HEAD"))
        .map(|r| r.name.as_str())
        .filter(|name| catalog.current.as_deref() != Some(*name))
        .collect();
    names.sort_unstable();
    names.dedup();
    let stuck = catalog.stuck().map(|s| s.reason());
    let branch = match &stuck {
        Some(_) => stuck_branch(catalog),
        None => match merge::offered(repo, catalog, commit) {
            Some(branch) => branch.to_owned(),
            None => return,
        },
    };
    let mut targets: Vec<Target> = names
        .iter()
        .map(|name| {
            (
                (*name).to_owned(),
                Request::Merge {
                    theirs: commit,
                    target: (*name).to_owned(),
                },
                stuck.clone(),
            )
        })
        .collect();
    targets.push((
        commit.short(repo.abbrev_len.max(7)),
        Request::Merge {
            theirs: commit,
            target: commit.to_hex(),
        },
        stuck,
    ));
    let into = format!(" into {branch}…");
    target_menu_named(
        ui,
        &format!("Merge into {branch}"),
        |name| format!("Merge {name}{into}"),
        &targets,
        busy,
        request,
    );
}

/// *Merge feature into main…* for each local branch on the node that lacks commits of the open
/// worktree's branch, as a pull request merges; greyed out while the open worktree is stuck.
fn merge_into_targets(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    refs: &[&parterre_core::GitRef],
    catalog: &Catalog,
    busy: bool,
    request: &mut Option<Request>,
) {
    let Some(source) = merge::offered_into(repo, catalog, commit) else {
        return;
    };
    let stuck = catalog.stuck().map(|s| s.reason());
    let mut names: Vec<&str> = refs
        .iter()
        .filter(|r| r.kind == RefKind::LocalBranch && r.name != source)
        .map(|r| r.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    let targets: Vec<Target> = names
        .iter()
        .map(|name| {
            let into = (*name).to_owned();
            (into.clone(), Request::MergeInto { into }, stuck.clone())
        })
        .collect();
    target_menu_named(
        ui,
        &format!("Merge {source} into"),
        |name| format!("Merge {source} into {name}…"),
        &targets,
        busy,
        request,
    );
}

/// *Cherry-pick feature onto main…*: every commit of the node main lacks, named after a branch
/// on the node (a local one first), or its short hash; greyed out while the open worktree is
/// stuck.
fn cherry_pick_lacking(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    refs: &[&parterre_core::GitRef],
    catalog: &Catalog,
    busy: bool,
) -> Option<Request> {
    let branch = match catalog.stuck() {
        Some(_) => stuck_branch(catalog),
        None => cherry_pick::offered(repo, catalog, commit)?.to_owned(),
    };
    let mut names: Vec<&parterre_core::GitRef> = refs
        .iter()
        .copied()
        .filter(|r| !r.name.ends_with("/HEAD") && r.name != branch)
        .collect();
    names.sort_by_key(|r| (r.kind != RefKind::LocalBranch, r.name.as_str()));
    let name = names
        .first()
        .map(|r| r.name.clone())
        .unwrap_or_else(|| commit.short(repo.abbrev_len.max(7)));
    let label = format!("Cherry-pick {name} onto {branch}…");
    let request = Request::CherryPick {
        picks: cherry_pick::Picks::Lacking(commit),
        name: Some(name),
    };
    cherry_pick_item(ui, label, request, catalog, busy)
}

/// *Cherry-pick 3 commits onto main…*: the log's selection, as listed; greyed out while the
/// open worktree is stuck.
fn cherry_pick_selection(
    ui: &mut Ui,
    repo: &Repo,
    selection: &[Oid],
    catalog: &Catalog,
    busy: bool,
) -> Option<Request> {
    let branch = match catalog.stuck() {
        Some(_) => stuck_branch(catalog),
        None => cherry_pick::offered_chosen(repo, catalog, selection)?.to_owned(),
    };
    let what = match selection {
        [one] => one.short(repo.abbrev_len.max(7)),
        many => format!("{} commits", many.len()),
    };
    let label = format!("Cherry-pick {what} onto {branch}…");
    let request = Request::CherryPick {
        picks: cherry_pick::Picks::Chosen(selection.to_vec()),
        name: None,
    };
    cherry_pick_item(ui, label, request, catalog, busy)
}

fn cherry_pick_item(
    ui: &mut Ui,
    label: String,
    request: Request,
    catalog: &Catalog,
    busy: bool,
) -> Option<Request> {
    let stuck = catalog.stuck().map(|s| s.reason());
    let response = ui.add_enabled(!busy && stuck.is_none(), egui::Button::new(label));
    let response = match stuck {
        Some(reason) => response.on_disabled_hover_text(capitalized(&reason)),
        None => response.on_disabled_hover_text(loading_reason(true)),
    };
    if response.clicked() {
        ui.close();
        return Some(request);
    }
    None
}

/// The branch a stuck worktree is rebasing, else its branch, for greyed-out labels.
fn stuck_branch(catalog: &Catalog) -> String {
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
        .on_disabled_hover_text(loading_reason(busy))
        .clicked()
    {
        request = Some(Request::AddWorktree { start: commit });
        ui.close();
    }
    let Some(catalog) = catalog else {
        return request;
    };
    let mut others: Vec<_> = catalog
        .worktrees
        .iter()
        .filter(|w| !w.open && w.head == Some(commit))
        .collect();
    others.sort_by_key(|w| w.name());
    let go_to: Vec<Target> = others
        .iter()
        .map(|w| {
            (
                w.name(),
                Request::GoTo(w.path.clone()),
                w.missing.then(|| "Its folder is gone".to_owned()),
            )
        })
        .collect();
    target_menu(ui, "Go to worktree", &go_to, false, &mut request);
    // An operation in progress there doesn't keep one: deleting it ends that too.
    let locked = |w: &parterre_core::branches::Worktree| match &w.locked {
        Some(reason) if reason.is_empty() => Some("Locked".to_owned()),
        Some(reason) => Some(format!("Locked: {reason}")),
        None => None,
    };
    let deletable = |commit: Oid| {
        let mut found: Vec<_> = catalog
            .worktrees
            .iter()
            .filter(|w| !w.open && !w.main && w.head == Some(commit))
            .collect();
        found.sort_by_key(|w| w.name());
        found
    };
    if group.len() > 1 && group.iter().all(|&c| !deletable(c).is_empty()) {
        let all: Vec<_> = group.iter().flat_map(|&c| deletable(c)).collect();
        let blocked = all.iter().find_map(|w| {
            let reason = w.locked.as_ref()?;
            Some(if reason.is_empty() {
                format!("{} is locked", w.name())
            } else {
                format!("{} is locked: {reason}", w.name())
            })
        });
        let names: Vec<String> = all.iter().map(|w| w.name()).collect();
        let delete = Request::Run(Action::DeleteWorktrees {
            paths: all.iter().map(|w| w.path.clone()).collect(),
            branches: false,
        });
        let label = format!("Delete {} worktrees", all.len());
        all_item(ui, label, &names, delete, blocked, busy, &mut request);
        return request;
    }
    let deletions: Vec<Target> = deletable(commit)
        .into_iter()
        .map(|w| {
            (
                w.name(),
                Request::Run(Action::DeleteWorktrees {
                    paths: vec![w.path.clone()],
                    branches: false,
                }),
                locked(w),
            )
        })
        .collect();
    target_menu(ui, "Delete worktree", &deletions, busy, &mut request);
    request
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
        .on_hover_text(names.join(", "));
    let response = match blocked {
        Some(reason) => response.on_disabled_hover_text(reason),
        None => response.on_disabled_hover_text(loading_reason(true)),
    };
    if response.clicked() {
        *request = Some(value);
        ui.close();
    }
}

/// *Reset `<branch>` to here…*: only the open worktree's branch, and only where there's a
/// reset to offer. In the node menu, so the graph and the log's rows both have it.
pub fn reset_item(
    ui: &mut Ui,
    commit: Oid,
    catalog: Option<&Catalog>,
    busy: bool,
) -> Option<Request> {
    // Greyed out, not hidden, while the worktree is stuck: it's only for now.
    if let Some(stuck) = catalog.and_then(Catalog::stuck) {
        let label = format!("Reset {} to here…", stuck_branch(catalog?));
        ui.add_enabled(false, egui::Button::new(label))
            .on_disabled_hover_text(stuck.reason());
        return None;
    }
    let branch = parterre_core::reset::branch(catalog?, commit).ok()?;
    let clicked = ui
        .add_enabled(!busy, egui::Button::new(format!("Reset {branch} to here…")))
        .on_disabled_hover_text(loading_reason(true))
        .clicked();
    if clicked {
        ui.close();
    }
    clicked.then_some(Request::Reset {
        target: commit,
        mode: None,
    })
}

/// *Revert in `<branch>`…*: in the log's rows only, where commits are acted on one by one, for
/// a commit the open worktree's HEAD reaches. Greyed out while the worktree is stuck.
pub fn revert_item(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    catalog: &Catalog,
    busy: bool,
) -> Option<Request> {
    let name = revert::offered(repo, catalog, commit)?;
    if let Some(stuck) = catalog.stuck() {
        let label = format!("Revert in {}…", stuck_branch(catalog));
        ui.add_enabled(false, egui::Button::new(label))
            .on_disabled_hover_text(stuck.reason());
        return None;
    }
    let clicked = ui
        .add_enabled(!busy, egui::Button::new(format!("Revert in {name}…")))
        .on_disabled_hover_text(loading_reason(true))
        .clicked();
    if clicked {
        ui.close();
    }
    clicked.then_some(Request::Revert { commit })
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
            (_, Some(reason)) => response.on_disabled_hover_text(capitalized(reason)),
            (true, None) => response.on_disabled_hover_text(loading_reason(true)),
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
                .on_disabled_hover_text(capitalized(&reason));
        }
        many => menu::plain_submenu(ui, verb, |ui| {
            for (name, target, blocked) in many {
                item(ui, name.clone(), target, blocked);
            }
        }),
    }
}

pub(super) fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[derive(Debug)]
struct Form {
    repo: Arc<Repo>,
    catalog: Arc<Catalog>,
    start: Oid,
    draft: CreateDraft,
    switch: bool,
    fresh: bool,
    /// The window it was asked from.
    opener: ViewportId,
    /// The worktree section, when this form adds a worktree.
    worktree: Option<WorktreeFields>,
}

/// Where the new worktree goes, and whether to go to it afterwards.
#[derive(Debug)]
struct WorktreeFields {
    /// Always ends in the platform's separator.
    root: String,
    /// The folder's name, following the branch until it's typed in.
    name: String,
    name_edited: bool,
    go_to: bool,
    /// The folder last asked about, and the working tree it's unignored in.
    inside: Option<(PathBuf, Option<worktree_folder::Inside>)>,
    browse: Option<crate::file_dialog::Pending<()>>,
}

/// What the branch name of a new worktree amounts to.
enum Pick<'a> {
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
            name: String::new(),
            name_edited: false,
            go_to: false,
            inside: None,
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

    fn pick(&self) -> Pick<'_> {
        let name = self.draft.name().trim();
        if name.is_empty() {
            Pick::Detached
        } else if self.catalog.locals.iter().any(|b| b.name == name) {
            Pick::Existing(name)
        } else {
            Pick::New
        }
    }

    fn short(&self) -> String {
        let hex = self.start.to_hex();
        hex[..self.repo.abbrev_len.clamp(4, hex.len())].to_owned()
    }

    fn registered(&self) -> Vec<PathBuf> {
        self.catalog
            .worktrees
            .iter()
            .map(|w| w.path.clone())
            .collect()
    }

    /// The folder name follows the branch until it's typed in: `/` becomes `-`, the short hash
    /// when detached, and a number when the folder is taken.
    fn follow_branch(&mut self) {
        let base = match self.draft.name().trim() {
            "" => self.short(),
            name => worktree_folder::folder_name(name),
        };
        let registered = self.registered();
        let Some(wt) = &mut self.worktree else { return };
        if !wt.name_edited {
            wt.name = worktree_folder::free_name(Path::new(&wt.root), &base, &registered);
        }
    }

    fn folder(&self) -> Option<PathBuf> {
        self.worktree
            .as_ref()
            .map(|wt| Path::new(&wt.root).join(wt.name.trim()))
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
        let checkout = match self.pick() {
            Pick::Detached => Checkout::Detached,
            Pick::Existing(name) => Checkout::Existing(name.to_owned()),
            Pick::New => Checkout::New {
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

    /// What's wrong with the worktree section, if anything.
    fn worktree_error(&self) -> Option<String> {
        let wt = self.worktree.as_ref()?;
        if !Path::new(&wt.root).is_absolute() {
            return Some("Enter the full path of a folder.".into());
        }
        if let Some(error) = worktree_folder::name_error(&wt.name) {
            return Some(error.into());
        }
        let folder = self.folder()?;
        if wt.name_edited && worktree_folder::taken(&folder, &self.registered()) {
            return Some(format!("The folder '{}' already exists.", wt.name.trim()));
        }
        None
    }

    fn branch_error(&self) -> Option<String> {
        match self.pick() {
            Pick::Detached if self.worktree.is_some() => None,
            Pick::Existing(name) if self.worktree.is_some() => {
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

    /// `busy` while another Git operation runs: it can't start until that one is done.
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
                    if let Some(ix) = self.repo.lookup(&self.start) {
                        log = dialogs::commit_line(ui, self.repo.commit(ix), self.repo.abbrev_len);
                    }
                    // The branch controls, then the worktree's, then the one checkbox that
                    // applies: switching here, or going to the new worktree.
                    if adding {
                        self.worktree_branch_field(ui);
                    } else {
                        self.name_field(ui);
                    }
                    ui.add_space(8.0);
                    if adding && !matches!(self.pick(), Pick::New) {
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
                let shown = commands
                    .as_ref()
                    .map(|cmds| cmds.iter().map(|a| command_text(a)).collect::<Vec<_>>())
                    .unwrap_or_default();
                dialogs::command_box(ui, &shown);
                let label = match &self.worktree {
                    Some(wt) if wt.go_to => "Add and go to",
                    Some(_) => "Add",
                    None if self.switch => "Create and switch",
                    None => "Create",
                };
                dialogs::actions(ui, label, commands.is_ok() && !busy, false, false)
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
        let error = match self.pick() {
            Pick::New => self
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
        let upstream = match self.pick() {
            Pick::Existing(name) => self
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
        let registered = self.registered();
        let base = match self.draft.name().trim() {
            "" => self.short(),
            name => worktree_folder::folder_name(name),
        };
        let folder = self.folder();
        let error = self.worktree_error();
        let catalog = self.catalog.clone();
        let Some(wt) = &mut self.worktree else { return };
        ui.label(RichText::new("Worktree root").strong());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let browse = ui.ctx().fonts_mut(|f| {
                f.layout_no_wrap("Browse…".into(), egui::FontId::default(), Color32::WHITE)
                    .size()
                    .x
            }) + 24.0;
            let width = ui.available_width() - browse - 4.0;
            if widgets::text_field(ui, &mut wt.root, "Folder", width).changed() {
                wt.root = with_separator(std::mem::take(&mut wt.root));
                if !wt.name_edited {
                    wt.name = worktree_folder::free_name(Path::new(&wt.root), &base, &registered);
                }
            }
            if ui
                .add_enabled(wt.browse.is_none(), egui::Button::new("Browse…"))
                .clicked()
                && wt.browse.is_none()
            {
                let mut dialog = rfd::AsyncFileDialog::new().set_title("Worktree root");
                let start = Path::new(&wt.root);
                if let Some(dir) = start.ancestors().find(|a| a.is_dir()) {
                    dialog = dialog.set_directory(dir);
                }
                wt.browse = Some(crate::file_dialog::Pending::start(
                    (),
                    dialog.pick_folder(),
                    ui.ctx(),
                ));
            }
        });
        // A root inside a working tree, where git would see the worktree as untracked.
        if let Some(folder) = &folder {
            if wt.inside.as_ref().is_none_or(|(f, _)| f != folder) {
                wt.inside = Some((folder.clone(), worktree_folder::inside_repository(folder)));
            }
            if let Some((_, Some(inside))) = &wt.inside {
                let place = match catalog
                    .worktrees
                    .iter()
                    .find(|w| worktree_folder::same_path(&w.path, &inside.top))
                {
                    Some(w) => format!("this repository's worktree {}", w.name()),
                    None => format!("another repository, {}", inside.top.display()),
                };
                let mut exclude = false;
                caution(ui, |ui| {
                    ui.label(format!(
                        "This folder is inside {place}. It will show there as untracked, \
                         git add . there would record it as an embedded repository, and \
                         git clean -ffdx there would delete it."
                    ));
                    exclude = ui.button("Exclude it").clicked();
                });
                if exclude {
                    match worktree_folder::exclude(inside) {
                        Ok(()) => wt.inside = None,
                        Err(e) => {
                            ui.colored_label(ui.visuals().error_fg_color, e.to_string());
                        }
                    }
                }
            }
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Worktree name").strong());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let width = ui.available_width() - widgets::BUTTON - 4.0;
            let mut name = wt.name.clone();
            if widgets::text_field(ui, &mut name, "Folder name", width).changed() {
                // Separators and what Windows forbids can't be typed.
                wt.name = name
                    .chars()
                    .filter(|c| !worktree_folder::forbidden(*c))
                    .collect();
                wt.name_edited = true;
            }
            if ui
                .add_enabled_ui(wt.name_edited, |ui| {
                    widgets::icon_button(ui, parterre_core::glyphs::RESET, false)
                })
                .inner
                .on_hover_text("Follow the branch name")
                .clicked()
            {
                wt.name_edited = false;
                wt.name = worktree_folder::free_name(Path::new(&wt.root), &base, &registered);
            }
        });
        if let Some(error) = error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    /// Takes the folder picked with *Browse…*.
    fn browsed(&mut self) {
        let base = match self.draft.name().trim() {
            "" => self.short(),
            name => worktree_folder::folder_name(name),
        };
        let registered = self.registered();
        let Some(wt) = &mut self.worktree else { return };
        let Some(answer) = wt.browse.as_ref().and_then(|p| p.answer()) else {
            return;
        };
        wt.browse = None;
        if let Some(dir) = answer {
            wt.root = with_separator(dir.to_string_lossy().into_owned());
            if !wt.name_edited {
                wt.name = worktree_folder::free_name(Path::new(&wt.root), &base, &registered);
            }
        }
    }

    fn name_field(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Local branch name").strong());
        let suggestion = self.draft.suggested_name(&self.catalog);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let width = ui.available_width() - widgets::BUTTON - 4.0;
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
                    |ui| widgets::icon_button(ui, parterre_core::glyphs::RESET, false),
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

    /// The tracking label, remote, branch and reset.
    fn track_row(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Track branch").strong());
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), widgets::BUTTON),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let mut selected = self.draft.remote().map(str::to_owned);
                let mut remotes = self.catalog.remote_names.clone();
                remotes.sort();
                let chosen = ui
                    .allocate_ui_with_layout(
                        vec2(110.0, widgets::BUTTON),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| dialogs::choice(ui, "track-remote", &mut selected, "None", &remotes),
                    )
                    .inner;
                if chosen {
                    self.draft.set_remote(selected);
                }
                let width = ui.available_width() - widgets::BUTTON - 4.0;
                ui.allocate_ui_with_layout(
                    vec2(width, widgets::BUTTON),
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
                        widgets::icon_button(ui, parterre_core::glyphs::RESET, false)
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
            caution(ui, |ui| {
                ui.label(format!(
                    "Already tracked by {}. This creates another local branch tracking {}.",
                    trackers.join(", "),
                    upstream.as_deref().unwrap_or_default()
                ));
            });
        }
    }
}

/// An amber box for something to be aware of that isn't lost work.
pub(super) fn caution(ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
    let color = if ui.visuals().dark_mode {
        Color32::from_rgb(240, 191, 95)
    } else {
        Color32::from_rgb(139, 86, 0)
    };
    egui::Frame::new()
        .fill(color.gamma_multiply(0.10))
        .corner_radius(6)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.visuals_mut().override_text_color = Some(color);
            content(ui);
        });
}

#[derive(Debug)]
struct Job {
    path: PathBuf,
    label: String,
    opener: ViewportId,
    cancel: CancelTree,
    rx: mpsc::Receiver<Outcome>,
    /// The worktree to go to once it's done.
    go_to: Option<PathBuf>,
    /// A revert's: where the branch was, for the log to follow it to the new commit.
    reverting: Option<Oid>,
    /// A fetch's, pull's or push's: git's output as it comes.
    live: Option<Live>,
}

/// A revert done: the log follows the branch from `from` to the new commit `to`, which is
/// all it says. A log that can't follow it gets the notification instead.
#[derive(Debug)]
pub struct Reverted {
    pub path: PathBuf,
    pub from: Oid,
    pub to: Oid,
    pub label: String,
}

/// The most rows of its error a notice shows.
const NOTICE_ROWS: usize = 6;

#[derive(Debug)]
struct Notice {
    id: u64,
    title: String,
    path: PathBuf,
    report: Report,
    error: Option<String>,
    at: f64,
    /// Done, but needing the user's attention: orange, and it stays until closed. `error`
    /// holds its message.
    attention: bool,
}

impl Notice {
    /// Nothing more to say than its title: no details to open.
    fn is_plain(&self) -> bool {
        self.error.is_none() && self.report.steps.is_empty()
    }
}

/// What the usage statistics call `action` (#264): its kind, nothing of what it names.
fn operation(action: &Action) -> usage::Action {
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
    }
}

/// A warning before losing work, waiting for an answer.
#[derive(Debug)]
struct Loss {
    path: PathBuf,
    warning: Warning,
    fresh: bool,
    opener: ViewportId,
    /// The deletions whose changed files are listed, by index.
    show_files: std::collections::HashSet<usize>,
}

/// A reset's preview, being read for the dialog.
#[derive(Debug)]
struct Previewing {
    target: Oid,
    opener: ViewportId,
    /// Read again for the open dialog, after the repository changed.
    refresh: bool,
    mode: Option<Mode>,
    rx: mpsc::Receiver<Result<Preview, String>>,
}

/// A rebase's preview, being read for its confirmation.
#[derive(Debug)]
struct RebaseLoading {
    target: String,
    opener: ViewportId,
    rx: mpsc::Receiver<Result<rebase::Preview, String>>,
}

/// A cherry-pick's preview, being read for its confirmation.
#[derive(Debug)]
struct CherryPickLoading {
    opener: ViewportId,
    rx: mpsc::Receiver<Result<cherry_pick::Preview, String>>,
}

/// A revert's preview, being read for its dialog.
#[derive(Debug)]
struct RevertLoading {
    commit: Oid,
    opener: ViewportId,
    rx: mpsc::Receiver<Result<revert::Preview, String>>,
}

/// A merge's preview, being read for its dialog.
#[derive(Debug)]
struct MergeLoading {
    target: String,
    opener: ViewportId,
    rx: mpsc::Receiver<Result<merge::Preview, String>>,
}

#[derive(Debug, Default)]
pub struct Tool {
    repo: Option<Arc<Repo>>,
    pub catalog: Option<Arc<Catalog>>,
    loading: Option<mpsc::Receiver<Result<Catalog, String>>>,
    form: Option<Form>,
    warning: Option<Loss>,
    reset: Option<ResetDialog>,
    previewing: Option<Previewing>,
    rebase: Option<RebaseDialog>,
    rebase_loading: Option<RebaseLoading>,
    merge: Option<MergeDialog>,
    merge_loading: Option<MergeLoading>,
    cherry_pick: Option<CherryPickDialog>,
    cherry_pick_loading: Option<CherryPickLoading>,
    revert: Option<RevertDialog>,
    revert_loading: Option<RevertLoading>,
    /// Putting back the changes stashed for a revert.
    restore: Option<RestoreDialog>,
    /// How to pull a diverged branch.
    pull: Option<PullDialog>,
    set_upstream: Option<SetUpstreamDialog>,
    /// A revert done, for the log to follow.
    pub reverted: Option<Reverted>,
    /// Diff windows asked for from a dialog.
    pub diff_requests: Vec<(Arc<Repo>, FileDiffSpec)>,
    job: Option<Job>,
    notices: Vec<Notice>,
    next_notice: u64,
    details: Option<u64>,
    pub log_request: Option<(Arc<Repo>, Vec<Oid>, bool)>,
    pub reload: Option<PathBuf>,
    /// A worktree to make the open one.
    pub go_to: Option<PathBuf>,
    /// When the stuck banner shows.
    banner: BannerTimer,
    /// The next look at the worktree shows the banner at once: it follows an operation of
    /// parterre's own, F5 or opening the repository.
    look_at_once: bool,
    /// An operation ended and the worktree hasn't been looked at since.
    unlooked: bool,
    /// When the catalogue was last asked for, to ask again while the worktree is stuck.
    looked_at: f64,
    /// The open worktree's conflicted files changed with the catalogue last loaded.
    pub conflicts_changed: bool,
}

impl Tool {
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// The next look at the worktree shows the banner at once, if it is stuck: after F5.
    pub fn look_at_once(&mut self) {
        self.look_at_once = true;
    }

    /// Whether the stuck banner shows (see [`BannerTimer`]).
    pub fn banner_shown(&mut self) -> bool {
        self.banner.update(self.busy() || self.unlooked)
    }

    /// Asks for the catalogue of `repo` on a worker thread.
    fn load_catalog(&mut self, ctx: &egui::Context, repo: &Arc<Repo>) {
        let path = repo.path.clone();
        let worker_ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = Catalog::load(&path).map_err(|e| e.to_string());
            let _ = tx.send(result);
            worker_ctx.request_repaint();
        });
        self.loading = Some(rx);
        self.looked_at = ctx.input(|i| i.time);
    }

    pub fn update(&mut self, ctx: &egui::Context, repo: Option<&Arc<Repo>>) {
        let changed = match (&self.repo, repo) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        if changed {
            let different_path = self.repo.as_ref().map(|r| &r.path) != repo.map(|r| &r.path);
            if different_path {
                self.form = None;
                self.warning = None;
                self.reset = None;
                self.previewing = None;
                self.rebase = None;
                self.rebase_loading = None;
                self.merge = None;
                self.merge_loading = None;
                self.cherry_pick = None;
                self.cherry_pick_loading = None;
                self.revert = None;
                self.revert_loading = None;
                self.restore = None;
                self.pull = None;
                self.set_upstream = None;
                self.catalog = None;
                self.banner = BannerTimer::default();
                self.look_at_once = true;
            } else if let Some(dialog) = &self.reset
                && self.previewing.is_none()
            {
                // What the reset does may have changed with the repository.
                let (target, opener) = (dialog.preview.target, dialog.opener);
                self.preview(ctx, target, None, opener, true);
            }
            self.repo = repo.cloned();
            self.loading = None;
            if let Some(repo) = repo {
                self.load_catalog(ctx, repo);
            }
        }
        // While the worktree is stuck, look again every second: for the banner to show once
        // it lasts, and to go, and for the conflicted files, which change no refs.
        let now = ctx.input(|i| i.time);
        if self.banner.watching()
            && self.loading.is_none()
            && let Some(repo) = self.repo.clone()
        {
            if now - self.looked_at >= 1.0 {
                self.load_catalog(ctx, &repo);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                    1.0 - (now - self.looked_at),
                ));
            }
        }
        let loaded = self.loading.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("Branch information worker stopped unexpectedly.".into()))
            }
        });
        if let Some(result) = loaded {
            self.loading = None;
            match result {
                Ok(catalog) => {
                    crate::startup::mark("branch catalogue loaded");
                    let catalog = Arc::new(catalog);
                    if let Some(form) = &mut self.form {
                        form.catalog = catalog.clone();
                    }
                    let at_once = std::mem::take(&mut self.look_at_once);
                    self.banner.look(catalog.stuck().is_some(), now, at_once);
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
                        self.notice(
                            ctx,
                            repo.path.clone(),
                            "Could not load branch information".into(),
                            Report::default(),
                            Some(e),
                        );
                    }
                }
            }
        }
        self.previewed(ctx);
        self.rebase_previewed(ctx);
        self.merge_previewed(ctx);
        self.cherry_pick_previewed(ctx);
        self.revert_previewed(ctx);
        let completed = self.job.as_ref().and_then(|job| match job.rx.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Outcome::Failed {
                error: parterre_core::branches::Error::Failed("Git operation worker stopped unexpectedly. Review the reloaded repository before retrying.".into()),
                report: Report::default(),
            }),
        });
        if let Some(result) = completed {
            let job = self.job.take().unwrap();
            self.reload = Some(job.path.clone());
            let here = self.repo.as_ref().is_some_and(|r| r.path == job.path);
            // What was loaded meanwhile is from before it ended: look again, at once.
            if let Some(repo) = self.repo.clone().filter(|_| here) {
                self.unlooked = true;
                self.look_at_once = true;
                self.load_catalog(ctx, &repo);
            }
            // Changes stashed for a revert that didn't stop: the user decides about them.
            if let Outcome::Done(report) | Outcome::Failed { report, .. } = &result
                && let Some(stash) = report.stash.clone()
                && here
            {
                self.restore = Some(RestoreDialog::new(stash, job.opener));
            }
            if let (Outcome::Done(report), Some(from)) = (&result, job.reverting)
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
            match result {
                Outcome::Warning(warning)
                    if self.repo.as_ref().is_some_and(|r| r.path == job.path) =>
                {
                    self.warning = Some(Loss {
                        path: job.path,
                        warning,
                        fresh: true,
                        opener: job.opener,
                        show_files: Default::default(),
                    });
                }
                Outcome::Warning(_) => self.notice(
                    ctx,
                    job.path,
                    job.label,
                    Report::default(),
                    Some(
                        "Open this repository again to review the commits at risk before retrying."
                            .into(),
                    ),
                ),
                Outcome::Done(report) => {
                    self.go_to = job.go_to;
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
                    self.pull = Some(PullDialog::new(*diverged, job.opener));
                }
                Outcome::Diverged(_) => self.notice(
                    ctx,
                    job.path,
                    job.label,
                    Report::default(),
                    Some("Open this repository again to pull.".into()),
                ),
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
        match request {
            Request::Create {
                start,
                track,
                switch,
            } => {
                if let Some(catalog) = self.catalog.clone() {
                    self.form = Some(Form::new(repo, catalog, start, track, switch, opener));
                }
            }
            Request::AddWorktree { start } => {
                if let Some(catalog) = self.catalog.clone() {
                    self.form = Some(Form::new_worktree(repo, catalog, start, opener));
                }
            }
            Request::GoTo(_) => unreachable!("handled above"),
            Request::SetUpstream { branch } => {
                if let Some(catalog) = &self.catalog {
                    self.set_upstream = Some(SetUpstreamDialog::new(catalog, branch, opener));
                }
            }
            Request::Run(action) => self.run(ctx, repo.path.clone(), action, None, opener),
            Request::Reset { target, mode } => self.preview(ctx, target, mode, opener, false),
            Request::Rebase { onto, target } => {
                let path = repo.path.clone();
                let (tx, rx) = mpsc::channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(rebase::Preview::load(&path, onto).map_err(|e| e.to_string()));
                    ctx.request_repaint();
                });
                self.rebase_loading = Some(RebaseLoading { target, opener, rx });
            }
            Request::Merge { theirs, target } => {
                let path = repo.path.clone();
                let (tx, rx) = mpsc::channel();
                let ctx = ctx.clone();
                let name = target.clone();
                std::thread::spawn(move || {
                    let preview = merge::Preview::load(&path, theirs, &name);
                    let _ = tx.send(preview.map_err(|e| e.to_string()));
                    ctx.request_repaint();
                });
                self.merge_loading = Some(MergeLoading { target, opener, rx });
            }
            Request::Revert { commit } => {
                let path = repo.path.clone();
                let (tx, rx) = mpsc::channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let preview = revert::Preview::load(&path, commit);
                    let _ = tx.send(preview.map_err(|e| e.to_string()));
                    ctx.request_repaint();
                });
                self.revert_loading = Some(RevertLoading { commit, opener, rx });
            }
            Request::MergeInto { into } => {
                let path = repo.path.clone();
                let (tx, rx) = mpsc::channel();
                let ctx = ctx.clone();
                let name = into.clone();
                std::thread::spawn(move || {
                    let preview = merge::Preview::load_into(&path, &name);
                    let _ = tx.send(preview.map_err(|e| e.to_string()));
                    ctx.request_repaint();
                });
                let target = format!("into {into}");
                self.merge_loading = Some(MergeLoading { target, opener, rx });
            }
            Request::CherryPick { picks, name } => {
                let path = repo.path.clone();
                let (tx, rx) = mpsc::channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let preview = cherry_pick::Preview::load(&path, &picks, name);
                    let _ = tx.send(preview.map_err(|e| e.to_string()));
                    ctx.request_repaint();
                });
                self.cherry_pick_loading = Some(CherryPickLoading { opener, rx });
            }
        }
    }

    /// Opens the cherry-pick's confirmation once its preview is read.
    fn cherry_pick_previewed(&mut self, ctx: &egui::Context) {
        let Some(loading) = &self.cherry_pick_loading else {
            return;
        };
        let result = match loading.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("The cherry-pick preview stopped unexpectedly.".into())
            }
        };
        let loading = self.cherry_pick_loading.take().unwrap();
        let Some(repo) = self.repo.clone() else {
            return;
        };
        match result {
            Ok(preview) => {
                self.cherry_pick = Some(CherryPickDialog::new(preview, repo, ctx, loading.opener));
            }
            Err(e) => {
                let title = "Cherry-pick".to_owned();
                self.notice(ctx, repo.path.clone(), title, Report::default(), Some(e));
            }
        }
    }

    /// Opens the merge's dialog once its preview is read.
    fn merge_previewed(&mut self, ctx: &egui::Context) {
        let Some(loading) = &self.merge_loading else {
            return;
        };
        let result = match loading.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("The merge preview stopped unexpectedly.".into())
            }
        };
        let loading = self.merge_loading.take().unwrap();
        let Some(repo) = self.repo.clone() else {
            return;
        };
        match result {
            Ok(preview) => {
                // Merging the open worktree's branch, the command names it.
                let target = match &preview.outgoing {
                    Some(out) => out.source.clone(),
                    None => loading.target,
                };
                self.merge = Some(MergeDialog::new(preview, repo, target, loading.opener));
            }
            Err(e) => {
                let title = format!("Merge {}", loading.target);
                self.notice(ctx, repo.path.clone(), title, Report::default(), Some(e));
            }
        }
    }

    /// Opens the revert's dialog once its preview is read.
    fn revert_previewed(&mut self, ctx: &egui::Context) {
        let Some(loading) = &self.revert_loading else {
            return;
        };
        let result = match loading.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("The revert preview stopped unexpectedly.".into())
            }
        };
        let loading = self.revert_loading.take().unwrap();
        let Some(repo) = self.repo.clone() else {
            return;
        };
        match result {
            Ok(preview) => {
                self.revert = Some(RevertDialog::new(preview, repo, loading.opener));
            }
            Err(e) => {
                let short = loading.commit.short(repo.abbrev_len.max(7));
                let title = format!("Revert {short}");
                self.notice(ctx, repo.path.clone(), title, Report::default(), Some(e));
            }
        }
    }

    /// The notification for a revert the log couldn't follow, so nothing shows it's done.
    pub fn reverted_unseen(&mut self, ctx: &egui::Context, reverted: Reverted) {
        self.notice(ctx, reverted.path, reverted.label, Report::default(), None);
    }

    /// Opens the rebase's confirmation once its preview is read.
    fn rebase_previewed(&mut self, ctx: &egui::Context) {
        let Some(loading) = &self.rebase_loading else {
            return;
        };
        let result = match loading.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("The rebase preview stopped unexpectedly.".into())
            }
        };
        let loading = self.rebase_loading.take().unwrap();
        let Some(repo) = self.repo.clone() else {
            return;
        };
        match result {
            Ok(preview) => {
                self.rebase = Some(RebaseDialog::new(
                    preview,
                    repo,
                    loading.target,
                    loading.opener,
                ));
            }
            Err(e) => {
                let title = format!("Rebase onto {}", loading.target);
                self.notice(ctx, repo.path.clone(), title, Report::default(), Some(e));
            }
        }
    }

    /// Reads what a reset to `target` would do, for its dialog.
    fn preview(
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
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Preview::load(&path, target).map_err(|e| e.to_string()));
            ctx.request_repaint();
        });
        self.previewing = Some(Previewing {
            target,
            opener,
            refresh,
            mode,
            rx,
        });
    }

    /// Opens the reset dialog once its preview is read, or brings it up to date.
    fn previewed(&mut self, ctx: &egui::Context) {
        let Some(p) = &self.previewing else { return };
        let result = match p.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("The reset preview stopped unexpectedly.".into())
            }
        };
        let p = self.previewing.take().unwrap();
        let Some(repo) = self.repo.clone() else {
            return;
        };
        match (result, &mut self.reset) {
            (Ok(preview), Some(dialog)) if p.refresh => dialog.refresh(preview, repo),
            // Closed meanwhile.
            (Ok(_), None) if p.refresh => {}
            (Ok(preview), dialog) => {
                let mut opened = ResetDialog::new(preview, repo, p.opener, dialog.is_some());
                opened.mode = p.mode.unwrap_or(opened.mode);
                *dialog = Some(opened);
            }
            // Gone since: reset elsewhere, or the branch is no longer checked out.
            (Err(_), _) if p.refresh => self.reset = None,
            (Err(e), _) => {
                let title = format!("Reset to {}", p.target.short(repo.abbrev_len.max(7)));
                self.notice(ctx, repo.path.clone(), title, Report::default(), Some(e));
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
    ) {
        // Counted when the user starts it, not again when they agree to lose work.
        if approval.is_none() {
            usage::action(operation(&action));
        }
        let (tx, rx) = mpsc::channel();
        let cancel = CancelTree::default();
        let worker_cancel = cancel.clone();
        let worker_path = path.clone();
        let ctx = ctx.clone();
        let label = action.label();
        let reverting = match &action {
            Action::Revert(r) => Some(r.head),
            _ => None,
        };
        let live = action.is_network().then(Live::default);
        let mut branches = Branches::new(worker_path);
        if let Some(live) = &live {
            branches = branches.with_live(live.clone());
        }
        std::thread::spawn(move || {
            let outcome = branches.execute(action, approval.as_ref(), &worker_cancel);
            let _ = tx.send(outcome);
            ctx.request_repaint();
        });
        self.job = Some(Job {
            path,
            label,
            opener,
            cancel,
            rx,
            go_to: None,
            reverting,
            live,
        });
    }

    fn notice(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        title: String,
        report: Report,
        error: Option<String>,
    ) {
        self.next_notice += 1;
        self.notices.push(Notice {
            id: self.next_notice,
            title,
            path,
            report,
            error,
            at: ctx.input(|i| i.time),
            attention: false,
        });
    }

    /// A green notification of something done outside the branch tool, such as a reload. The
    /// same one again shows it anew rather than adding another.
    pub fn inform(&mut self, ctx: &egui::Context, path: PathBuf, title: &str) {
        let at = ctx.input(|i| i.time);
        if let Some(n) = self
            .notices
            .iter_mut()
            .find(|n| n.title == title && n.path == path && n.is_plain())
        {
            n.at = at;
            return;
        }
        self.notice(ctx, path, title.to_owned(), Report::default(), None);
    }

    /// The graph's `palette` and `options`, for the rebase's commit list.
    pub fn show(&mut self, ctx: &egui::Context, palette: &Palette, options: &GraphOptions) {
        if let Some(mut form) = self.form.take() {
            let (answer, log) = form.show(ctx, self.busy());
            if log {
                self.log_request = Some((form.repo.clone(), vec![form.start], false));
            }
            match answer {
                dialogs::Answer::Primary => {
                    self.run(
                        ctx,
                        form.repo.path.clone(),
                        form.action(),
                        None,
                        form.opener,
                    );
                    let go_to = form
                        .worktree
                        .as_ref()
                        .filter(|w| w.go_to)
                        .and(form.folder());
                    if let Some(job) = &mut self.job {
                        job.go_to = go_to;
                    }
                }
                dialogs::Answer::Cancel => {}
                dialogs::Answer::Open => self.form = Some(form),
            }
        }
        self.reset_dialog(ctx);
        self.rebase_dialog(ctx, palette, options);
        self.merge_dialog(ctx, palette, options);
        self.cherry_pick_dialog(ctx, palette, options);
        self.revert_dialog(ctx);
        self.restore_dialog(ctx);
        self.pull_dialog(ctx);
        self.set_upstream_dialog(ctx);
        self.loss_dialog(ctx);
        self.network_window(ctx);
        self.notifications(ctx);
    }

    /// Whether a fetch can start now, or why not.
    pub fn fetch_blocked(&self) -> Option<&'static str> {
        match &self.catalog {
            _ if self.busy() => Some(loading_reason(true)),
            None => Some(loading_reason(false)),
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

    fn pull_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.pull.take() else {
            return;
        };
        let Some(path) = self.repo.as_ref().map(|r| r.path.clone()) else {
            return;
        };
        match dialog.show(ctx, self.busy()) {
            dialogs::Answer::Primary => {
                let action = Action::Pull(Box::new(dialog.pull()));
                self.run(ctx, path, action, None, dialog.opener);
            }
            dialogs::Answer::Cancel => {}
            dialogs::Answer::Open => self.pull = Some(dialog),
        }
    }

    fn set_upstream_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.set_upstream.take() else {
            return;
        };
        let Some(path) = self.repo.as_ref().map(|r| r.path.clone()) else {
            return;
        };
        match dialog.show(ctx, self.busy()) {
            dialogs::Answer::Primary => {
                if let Some(action) = dialog.action() {
                    self.run(ctx, path, action, None, dialog.opener);
                }
            }
            dialogs::Answer::Cancel => {}
            dialogs::Answer::Open => self.set_upstream = Some(dialog),
        }
    }

    fn reset_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.reset.take() else {
            return;
        };
        let asked = dialog.show(ctx, self.busy());
        let repo = self.repo.clone();
        if let (Some((commits, exact)), Some(repo)) = (asked.log, &repo) {
            self.log_request = Some((repo.clone(), commits, exact));
        }
        if let Some(repo) = &repo {
            self.diff_requests
                .extend(asked.diffs.into_iter().map(|spec| (repo.clone(), spec)));
        }
        match asked.answer {
            Some(dialogs::Answer::Primary) if !self.busy() => {
                if let Some(repo) = &repo {
                    let action = Action::Reset(Box::new(dialog.preview.reset(dialog.mode)));
                    self.previewing = None;
                    self.run(ctx, repo.path.clone(), action, None, dialog.opener);
                }
            }
            Some(dialogs::Answer::Cancel) => self.previewing = None,
            _ => self.reset = Some(dialog),
        }
    }

    fn rebase_dialog(&mut self, ctx: &egui::Context, palette: &Palette, options: &GraphOptions) {
        let Some(mut dialog) = self.rebase.take() else {
            return;
        };
        let asked = dialog.show(ctx, self.busy(), palette, options);
        let repo = self.repo.clone();
        if let (Some(oid), Some(repo)) = (asked.log, &repo) {
            self.log_request = Some((repo.clone(), vec![oid], false));
        }
        match asked.answer {
            Some(dialogs::Answer::Primary) if !self.busy() => {
                if let Some(repo) = &repo {
                    let action = Action::Rebase(Box::new(dialog.rebase()));
                    self.run(ctx, repo.path.clone(), action, None, dialog.opener);
                }
            }
            Some(dialogs::Answer::Cancel) => {}
            _ => self.rebase = Some(dialog),
        }
    }

    fn merge_dialog(&mut self, ctx: &egui::Context, palette: &Palette, options: &GraphOptions) {
        let Some(mut dialog) = self.merge.take() else {
            return;
        };
        let asked = dialog.show(ctx, self.busy(), palette, options);
        let repo = self.repo.clone();
        if let (Some(oid), Some(repo)) = (asked.log, &repo) {
            self.log_request = Some((repo.clone(), vec![oid], false));
        }
        match asked.answer {
            Some(dialogs::Answer::Primary) if !self.busy() => {
                if let Some(repo) = &repo {
                    let action = Action::Merge(Box::new(dialog.merge()));
                    self.run(ctx, repo.path.clone(), action, None, dialog.opener);
                }
            }
            Some(dialogs::Answer::Cancel) => {}
            _ => self.merge = Some(dialog),
        }
    }

    fn cherry_pick_dialog(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        options: &GraphOptions,
    ) {
        let Some(mut dialog) = self.cherry_pick.take() else {
            return;
        };
        let asked = dialog.show(ctx, self.busy(), palette, options);
        let repo = self.repo.clone();
        if let (Some(oid), Some(repo)) = (asked.log, &repo) {
            self.log_request = Some((repo.clone(), vec![oid], false));
        }
        match asked.answer {
            Some(dialogs::Answer::Primary) if !self.busy() => {
                if let Some(repo) = &repo {
                    let action = Action::CherryPick(Box::new(dialog.cherry_pick()));
                    self.run(ctx, repo.path.clone(), action, None, dialog.opener);
                }
            }
            Some(dialogs::Answer::Cancel) => {}
            _ => self.cherry_pick = Some(dialog),
        }
    }

    fn revert_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.revert.take() else {
            return;
        };
        let asked = dialog.show(ctx, self.busy());
        let repo = self.repo.clone();
        if let (Some(oid), Some(repo)) = (asked.log, &repo) {
            self.log_request = Some((repo.clone(), vec![oid], false));
        }
        match asked.answer {
            Some(dialogs::Answer::Primary) if !self.busy() => {
                if let Some(repo) = &repo {
                    let action = Action::Revert(Box::new(dialog.revert()));
                    self.run(ctx, repo.path.clone(), action, None, dialog.opener);
                }
            }
            Some(dialogs::Answer::Cancel) => {}
            _ => self.revert = Some(dialog),
        }
    }

    fn restore_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.restore.take() else {
            return;
        };
        match dialog.show(ctx, self.busy()) {
            dialogs::Answer::Primary if !self.busy() => {
                if let Some(repo) = self.repo.clone() {
                    let action = Action::RestoreStash(dialog.stash.oid);
                    self.run(ctx, repo.path.clone(), action, None, dialog.opener);
                }
            }
            dialogs::Answer::Cancel => {}
            _ => self.restore = Some(dialog),
        }
    }

    /// The confirmation or warning an operation came back with.
    fn loss_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut loss) = self.warning.take() else {
            return;
        };
        let warning = &loss.warning;
        let plural = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
        let commits = plural(warning.commits.len(), "commit");
        let changed: usize = warning.deletions.iter().map(|d| d.files.len()).sum();
        let files = plural(changed, "changed file");
        let confirmation = warning.is_confirmation();
        let refused = warning.deletions.iter().any(|d| d.refusal.is_some());
        let several = warning.deletions.len() > 1;
        let mut deletes_branches = warning.deletes_branches();
        let (title, button) = match &warning.action {
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
            Action::Push(push) if confirmation => (
                format!("Force push {} to {}?", push.branch, push.remote),
                "Force push",
            ),
            Action::Push(push) => (
                format!(
                    "Force push {} to {} and lose {commits}?",
                    push.branch, push.remote
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
        };
        const TRIANGLE: parterre_core::glyphs::Glyph = &[parterre_core::glyphs::Part::Path(
            "M12 3 2 21h20ZM12 9v5m0 3v1",
        )];
        let mut show_log = None;
        let busy = self.busy();
        let force_push = match &warning.action {
            Action::Push(push) => Some(push),
            _ => None,
        };
        let screen = match force_push {
            Some(_) => crate::usage::Screen::ForcePush,
            None => crate::usage::Screen::LostWork,
        };
        let mut dialog = dialogs::Dialog::new("branch-loss", &title)
            .screen(screen)
            .opener(loss.opener)
            .raise(loss.fresh)
            .resizable();
        if !confirmation {
            dialog = dialog.icon(TRIANGLE, true);
        }
        let show_files = &mut loss.show_files;
        let shown = dialog.show(ctx, |ui| {
            dialogs::fields(ui, |ui| {
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
                    ui.checkbox(&mut deletes_branches, label);
                }
                let unreachable = match force_push {
                    Some(push) => format!(
                        "These commits on {}/{} are not on {}.",
                        push.remote, push.branch, push.branch
                    ),
                    None => "These commits are not reachable from any surviving branch, tag or \
                             worktree."
                        .to_owned(),
                };
                let row = |ui: &mut Ui, content: &mut dyn FnMut(&mut Ui)| {
                    egui::Frame::new()
                        .fill(widgets::tones(ui).seg_bg)
                        .corner_radius(8)
                        .inner_margin(egui::Margin::symmetric(12, 8))
                        .show(ui, |ui| ui.horizontal(|ui| content(ui)));
                };
                // What one deletion loses: one row each for its files and its commits.
                let mut losses = |ui: &mut Ui, i: usize, d: &parterre_core::branches::Deletion| {
                    if let Some(refusal) = &d.refusal {
                        ui.label("Git refused to delete it:");
                        ui.label(RichText::new(refusal).monospace());
                    }
                    if !d.files.is_empty() {
                        let shown = show_files.contains(&i);
                        row(ui, &mut |ui| {
                            ui.label(plural(d.files.len(), "changed file"));
                            if ui.link(if shown { "Hide" } else { "Show" }).clicked()
                                && !show_files.remove(&i)
                            {
                                show_files.insert(i);
                            }
                        });
                        if shown {
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
                    }
                    if let Some(b) = branch {
                        row(ui, &mut |ui| {
                            ui.label(format!("{} on {}", plural(b.commits.len(), "commit"), b.name));
                            if ui.link("Show in log").clicked() {
                                show_log = Some(b.commits.clone());
                            }
                        });
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
                        ui.label(&commits);
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
            });
            let commands = warning.commands.iter().map(|a| command_text(a)).collect::<Vec<_>>();
            dialogs::command_box(ui, &commands);
            dialogs::actions(ui, button, !busy, !confirmation, loss.fresh && !confirmation)
        });
        if let Some(commits) = show_log {
            self.log_request = Some((warning.repo.clone(), commits, true));
        }
        if deletes_branches != loss.warning.deletes_branches() {
            loss.warning.set_deletes_branches(deletes_branches);
            ctx.request_repaint();
        }
        match shown.inner {
            dialogs::Answer::Primary => {
                let action = loss.warning.action.clone();
                self.run(ctx, loss.path, action, Some(loss.warning), loss.opener)
            }
            dialogs::Answer::Cancel => {}
            dialogs::Answer::Open if !shown.should_close() => {
                self.warning = Some(Loss {
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
            .retain(|n| n.error.is_some() || self.details == Some(n.id) || now - n.at < 5.0);
        if !self.notices.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
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
                                if n.is_plain() {
                                    ui.label(title);
                                } else if ui.link(title).clicked() {
                                    self.details = Some(n.id);
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
                                job.wrap.max_rows = NOTICE_ROWS;
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
                                    self.details = Some(n.id);
                                }
                            }
                        });
                }
                if let Some(id) = remove {
                    self.notices.retain(|n| n.id != id);
                }
            });
        if let Some(n) = self.notices.iter().find(|n| Some(n.id) == self.details) {
            let shown = dialogs::Dialog::new("git-operation-details", &n.title)
                .screen(crate::usage::Screen::OperationDetails)
                .width(600.0)
                .resizable()
                .show(ctx, |ui| {
                    dialogs::fields(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.weak(n.path.display().to_string());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    copy_button(ui, n.id, details_text(n));
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
                    dialogs::actions(ui, "", false, false, false)
                });
            if shown.inner == dialogs::Answer::Cancel || shown.should_close() {
                self.details = None;
            }
        }
    }
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

/// Copies `text`, with a check mark for a moment after the click.
fn copy_button(ui: &mut Ui, id: u64, text: String) {
    let copied_id = Id::new("copied-operation-details").with(id);
    let now = ui.input(|i| i.time);
    let at: Option<f64> = ui.data(|d| d.get_temp(copied_id));
    let copied = at.is_some_and(|at| now - at < 1.5);
    if copied {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(300));
    }
    let done = Color32::from_rgb(0x2e, 0xa0, 0x43);
    if widgets::copy_button(ui, copied, done)
        .on_hover_text("Copy")
        .clicked()
    {
        ui.ctx().copy_text(text);
        ui.data_mut(|d| d.insert_temp(copied_id, now));
    }
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
                |ui| super::node_menu(ui, &repo, commit, group, Some(&catalog), false, true),
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
            |ui| super::node_menu(ui, &repo, tip, &[tip, base], Some(&catalog), false, true),
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
                    .find(|w| parterre_core::worktree_folder::same_path(&w.path, p))
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
            .find(|w| parterre_core::worktree_folder::same_path(&w.path, &others.join(name)))
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
        // Unticked, the branch keeps its commit: a confirmation.
        assert!(h.shows("Delete") && !h.shows("Delete anyway"));
        h.click("Also delete local branch b");
        h.until("the branch's commit is listed", |h| {
            h.shows("Delete worktree b and lose 1 commit?")
                && h.shows("1 commit on b")
                && h.shows("Delete anyway")
        });
        h.click("Show in log");
        let (_, commits, _) = h.tool.log_request.take().expect("the log is asked for");
        assert_eq!(commits, [only]);
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
                |ui| super::node_menu(ui, &repo, commit, group, Some(&catalog), false, false),
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
