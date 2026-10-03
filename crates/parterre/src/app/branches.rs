//! The local-branch and worktree tools. Graph nodes and log rows share the same menu and
//! controller. Slow Git queries and all mutations run on workers; forms retain their selected
//! commit. Its dialogs are modeless windows, opened over the window they were asked from.

use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use eframe::egui::{self, Color32, Id, RichText, Ui, ViewportId, vec2};
use parterre_core::branches::{
    Action, AddWorktree, Branches, Cancel, Catalog, Checkout, Create, CreateDraft, Outcome, Report,
    Warning, command_text,
};
use parterre_core::file_diff::FileDiffSpec;
use parterre_core::reset::{Mode, Preview};
use parterre_core::worktree_folder;
use parterre_core::{Oid, RefKind, Repo};

use super::cherry_pick::CherryPickDialog;
use super::merge::MergeDialog;
use super::rebase::{RebaseDialog, stuck_color};
use super::reset::ResetDialog;
use super::revert::{RestoreDialog, RevertDialog};
use crate::theme::Palette;
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
}

/// One target gets a direct named item; several get the existing app's submenu treatment.
/// The worktree section follows the branch section while `worktrees` are shown.
pub fn node_menu(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    catalog: Option<&Catalog>,
    busy: bool,
    worktrees: bool,
) -> Option<Request> {
    menu_for(ui, repo, commit, None, catalog, busy, worktrees)
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
    menu_for(ui, repo, commit, Some(selection), catalog, busy, worktrees)
}

fn menu_for(
    ui: &mut Ui,
    repo: &Repo,
    commit: Oid,
    selection: Option<&[Oid]>,
    catalog: Option<&Catalog>,
    busy: bool,
    worktrees: bool,
) -> Option<Request> {
    let branch = branch_section(ui, repo, commit, selection, catalog, busy);
    let worktree = worktrees
        .then(|| worktree_section(ui, commit, catalog, busy))
        .flatten();
    branch.or(worktree)
}

fn loading_reason(busy: bool) -> &'static str {
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
    let deletions: Vec<_> = refs
        .iter()
        .filter(|r| {
            r.kind == RefKind::LocalBranch
                && catalog.current.as_ref() != Some(&r.name)
                && !catalog.occupied.contains_key(&r.name)
        })
        .map(|r| {
            (
                r.name.clone(),
                Request::Run(Action::Delete {
                    name: r.name.clone(),
                    tip: commit,
                }),
                None,
            )
        })
        .collect();
    target_menu(ui, "Delete branch", &deletions, busy, &mut request);
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

/// *Add worktree here…*, and going to or deleting the other worktrees at the commit.
fn worktree_section(
    ui: &mut Ui,
    commit: Oid,
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
    let deletions: Vec<Target> = others
        .iter()
        .filter(|w| !w.main)
        .map(|w| {
            // An operation in progress there doesn't keep it: deleting it ends that too.
            let blocked = match &w.locked {
                Some(reason) if reason.is_empty() => Some("Locked".to_owned()),
                Some(reason) => Some(format!("Locked: {reason}")),
                None => None,
            };
            (
                w.name(),
                Request::Run(Action::DeleteWorktree {
                    path: w.path.clone(),
                }),
                blocked,
            )
        })
        .collect();
    target_menu(ui, "Delete worktree", &deletions, busy, &mut request);
    request
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
type Target = (String, Request, Option<String>);

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
fn target_menu_named(
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

fn capitalized(s: &str) -> String {
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
        let (id, title) = if adding {
            ("add-worktree", "Add a worktree")
        } else {
            ("create-branch", "Create branch")
        };
        let shown = dialogs::Dialog::new(id, title)
            .opener(self.opener)
            .raise(self.fresh)
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
    cancel: Cancel,
    rx: mpsc::Receiver<Outcome>,
    /// The worktree to go to once it's done.
    go_to: Option<PathBuf>,
    /// A revert's: where the branch was, for the log to follow it to the new commit.
    reverting: Option<Oid>,
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

/// A warning before losing work, waiting for an answer.
#[derive(Debug)]
struct Loss {
    path: PathBuf,
    warning: Warning,
    fresh: bool,
    opener: ViewportId,
    /// The changed files are listed.
    show_files: bool,
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
}

impl Tool {
    pub fn busy(&self) -> bool {
        self.job.is_some()
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
                self.catalog = None;
            } else if let Some(dialog) = &self.reset
                && self.previewing.is_none()
            {
                // What the reset does may have changed with the repository.
                let (target, opener) = (dialog.preview.target, dialog.opener);
                self.preview(ctx, target, None, opener, true);
            }
            self.repo = repo.cloned();
            self.loading = repo.map(|r| {
                let path = r.path.clone();
                let ctx = ctx.clone();
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = Catalog::load(&path).map_err(|e| e.to_string());
                    let _ = tx.send(result);
                    ctx.request_repaint();
                });
                rx
            });
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
                    let catalog = Arc::new(catalog);
                    if let Some(form) = &mut self.form {
                        form.catalog = catalog.clone();
                    }
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
                        show_files: false,
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
        let (tx, rx) = mpsc::channel();
        let cancel = Cancel::default();
        let worker_cancel = cancel.clone();
        let worker_path = path.clone();
        let ctx = ctx.clone();
        let label = action.label();
        let reverting = match &action {
            Action::Revert(r) => Some(r.head),
            _ => None,
        };
        std::thread::spawn(move || {
            let outcome =
                Branches::new(worker_path).execute(action, approval.as_ref(), &worker_cancel);
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
        self.loss_dialog(ctx);
        self.notifications(ctx);
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
        let files = plural(warning.files.len(), "changed file");
        let confirmation = warning.is_confirmation();
        let (title, button) = match &warning.action {
            Action::Delete { name, .. } => (
                format!("Delete branch {name} and lose {commits}?"),
                "Delete anyway",
            ),
            Action::DeleteWorktree { path } => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let cost = match (warning.files.is_empty(), warning.commits.is_empty()) {
                    (false, false) => format!(" and lose {files} and {commits}"),
                    (false, true) => format!(" and lose {files}"),
                    (true, false) => format!(" and lose {commits}"),
                    (true, true) if warning.refusal.is_some() => " anyway".to_owned(),
                    (true, true) => String::new(),
                };
                let button = if confirmation {
                    "Delete"
                } else {
                    "Delete anyway"
                };
                (format!("Delete worktree {name}{cost}?"), button)
            }
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
        let mut show_log = false;
        let busy = self.busy();
        let mut dialog = dialogs::Dialog::new("branch-loss", &title)
            .opener(loss.opener)
            .raise(loss.fresh);
        if !confirmation {
            dialog = dialog.icon(TRIANGLE, true);
        }
        let show_files = &mut loss.show_files;
        let shown = dialog.show(ctx, |ui| {
            dialogs::fields(ui, |ui| {
                if let Action::DeleteWorktree { path } = &warning.action {
                    ui.label(format!(
                        "The folder {} and everything in it will be deleted.",
                        path.display()
                    ));
                }
                if let Some(refusal) = &warning.refusal {
                    ui.label("Git refused to delete it:");
                    ui.label(RichText::new(refusal).monospace());
                }
                let row = |ui: &mut Ui, content: &mut dyn FnMut(&mut Ui)| {
                    egui::Frame::new()
                        .fill(widgets::tones(ui).seg_bg)
                        .corner_radius(8)
                        .inner_margin(egui::Margin::symmetric(12, 8))
                        .show(ui, |ui| ui.horizontal(|ui| content(ui)));
                };
                if !warning.files.is_empty() {
                    row(ui, &mut |ui| {
                        ui.label(&files);
                        let label = if *show_files { "Hide" } else { "Show" };
                        if ui.link(label).clicked() {
                            *show_files = !*show_files;
                        }
                    });
                    if *show_files {
                        for file in &warning.files {
                            ui.label(RichText::new(file).monospace());
                        }
                    }
                }
                if !warning.commits.is_empty() {
                    ui.label(
                        "These commits are not reachable from any surviving branch, tag or worktree.",
                    );
                    row(ui, &mut |ui| {
                        ui.label(&commits);
                        show_log = ui.link("Show in log").clicked();
                    });
                }
            });
            let commands = warning.commands.iter().map(|a| command_text(a)).collect::<Vec<_>>();
            dialogs::command_box(ui, &commands);
            dialogs::actions(ui, button, !busy, !confirmation, loss.fresh && !confirmation)
        });
        if show_log {
            self.log_request = Some((warning.repo.clone(), warning.commits.clone(), true));
        }
        match shown.inner {
            dialogs::Answer::Primary => {
                let action = warning.action.clone();
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
                if let Some(job) = &self.job {
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
                                if ui.link(RichText::new(&n.title).color(color)).clicked() {
                                    self.details = Some(n.id);
                                }
                                if ui.small_button("×").clicked() {
                                    remove = Some(n.id);
                                }
                            });
                            if let Some(error) = &n.error {
                                ui.add(egui::Label::new(error).wrap());
                            }
                        });
                }
                if let Some(id) = remove {
                    self.notices.retain(|n| n.id != id);
                }
            });
        if let Some(n) = self.notices.iter().find(|n| Some(n.id) == self.details) {
            let shown = dialogs::Dialog::new("git-operation-details", &n.title)
                .width(600.0)
                .show(ctx, |ui| {
                    dialogs::fields(ui, |ui| {
                        ui.weak(n.path.display().to_string());
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

impl Drop for Tool {
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.cancel();
        }
    }
}
