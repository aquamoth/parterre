//! The Git menu (#339, #344): fixed items acting on the selected node, named after their
//! targets, and greyed out, not left out, where they don't apply. In each section what only
//! looks comes first, then what changes, then what destroys. A target that has several (Switch
//! to with two branches on the node) is a submenu. With several nodes selected, what acts on
//! one is greyed out, and deleting what they all have is one item. The graph's right-click
//! menu is this one, without what doesn't apply (`node_menu`).

use parterre_core::branches::{Action, Catalog};
use parterre_core::{Oid, RefKind, Repo};

use super::ParterreApp;
use super::branches::{self, Request, Target, loading_reason};
use super::compare_window::CompareRequest;
use super::menu_bar::{Command, Entry, Item, Submenu, case, case_with};
use crate::keys::{self, Platform};

/// What a section's items need to know.
struct Ctx<'a> {
    p: Platform,
    /// The selected node's name: its first branch (a local one first), or its short hash.
    x: Option<String>,
    /// The current branch, or HEAD.
    current: String,
    catalog: Option<&'a Catalog>,
    busy: bool,
}

impl Ctx<'_> {
    /// Why git can't be asked now, if it can't.
    fn blocked(&self) -> Option<&'static str> {
        (self.busy || self.catalog.is_none()).then(|| loading_reason(self.busy))
    }

    /// `template` in the menus' case, naming the node where it says `{x}` and the current
    /// branch where it says `{current}`; with no node selected, `without`.
    fn label(&self, template: &str, without: &str) -> String {
        self.label_on(template, without, &self.current)
    }

    /// [`Ctx::label`] with `branch` as `{current}`: the branch an operation would change.
    fn label_on(&self, template: &str, without: &str, branch: &str) -> String {
        let (text, x) = match &self.x {
            Some(x) => (case(self.p, template), x.as_str()),
            None => (case(self.p, without), ""),
        };
        fill(&text, x, branch)
    }
}

/// `text` with `{x}` and `{current}` filled in, in one pass: a name with braces stays as it is.
fn fill(text: &str, x: &str, current: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('{') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        if let Some(after) = rest.strip_prefix("{x}") {
            out.push_str(x);
            rest = after;
        } else if let Some(after) = rest.strip_prefix("{current}") {
            out.push_str(current);
            rest = after;
        } else {
            out.push('{');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

/// An item greyed out with `why`.
fn grey(label: String, why: &str) -> Entry {
    Item::new(label, Command::ShowLog)
        .blocked(Some(why.to_owned()))
        .into()
}

/// One item for a single target (`single` names it), a submenu (`verb`) for several, greyed
/// out (`none`, `why`) for none. Targets blocked for the same reason grey the submenu.
fn targets(
    c: &Ctx,
    verb: String,
    single: impl Fn(&str) -> String,
    none: String,
    why: &str,
    targets: Vec<Target>,
) -> Entry {
    let blocked = c.blocked();
    let item = |label: String, request: Request, reason: Option<String>| -> Entry {
        Item::new(label, Command::Git(request))
            .blocked(reason.or(blocked.map(str::to_owned)))
            .into()
    };
    let same = targets
        .first()
        .and_then(|t| t.2.clone())
        .filter(|r| targets.iter().all(|t| t.2.as_ref() == Some(r)));
    match targets.len() {
        0 => grey(none, blocked.unwrap_or(why)),
        1 => {
            let (name, request, reason) = targets.into_iter().next().expect("one");
            item(
                single(&name),
                request,
                reason.map(|r| branches::capitalized(&r)),
            )
        }
        _ => {
            let mut submenu = Submenu::new(
                verb,
                targets
                    .into_iter()
                    .map(|(name, request, reason)| {
                        item(name, request, reason.map(|r| branches::capitalized(&r)))
                    })
                    .collect(),
            );
            if let Some(reason) = same.or(blocked.map(str::to_owned)) {
                submenu.enabled = false;
                submenu.why = Some(branches::capitalized(&reason));
            }
            submenu.into()
        }
    }
}

impl ParterreApp {
    /// The Git menu's sections, for the selected node.
    pub(super) fn git_menu(&self) -> Vec<Entry> {
        let p = Platform::CURRENT;
        let catalog = self.branches.catalog.as_deref();
        let scene = self.scene.as_ref();
        let group: Vec<usize> = self.selection.nodes.clone();
        let node = self.selection.current();
        let commit = node.and_then(|n| {
            let scene = scene?;
            Some(scene.repo.commit(scene.graph.nodes[n].commit).oid)
        });
        let x = node.and_then(|n| scene.map(|s| node_name(s, n)));
        let current = catalog
            .and_then(|c| c.current.clone())
            .unwrap_or_else(|| "HEAD".to_owned());
        let c = Ctx {
            p,
            x,
            current,
            catalog,
            busy: self.branches.busy(),
        };
        let mut worktree = worktree_section(&c, commit);
        let mut branch = branch_section(&c, self, commit);
        let mut integrate = integrate_section(&c, self, commit);
        let mut remote = self.remote_section(&c, commit);
        if group.len() > 1 {
            // Several nodes: what acts on one is greyed out, and deleting what every node has
            // is one item, in place of the section's Delete (its last).
            for section in [&mut worktree, &mut branch, &mut integrate, &mut remote] {
                one_node(p, section);
            }
            if let (Some(scene), Some(catalog)) = (scene, catalog) {
                let oids: Vec<Oid> = group
                    .iter()
                    .map(|&n| scene.repo.commit(scene.graph.nodes[n].commit).oid)
                    .collect();
                let (worktrees, branches) = group_deletions(&scene.repo, catalog, &oids, c.busy);
                for (section, all) in [(&mut worktree, worktrees), (&mut branch, branches)] {
                    if !all.is_empty() {
                        section.pop();
                        section.extend(all);
                    }
                }
            }
        }
        let mut entries = Vec::new();
        entries.extend(self.history(&c, &group));
        for section in [worktree, branch, integrate, remote] {
            entries.push(Entry::Separator);
            entries.extend(section);
        }
        entries
    }

    /// Show Log, and comparing with HEAD, the working tree and the upstream.
    fn history(&self, c: &Ctx, group: &[usize]) -> Vec<Entry> {
        let p = c.p;
        let mut entries = vec![Entry::Heading(case(p, "History"))];
        let log = Item::new(case(p, "Show log"), Command::ShowLog).key(keys::SHOW_LOG);
        let log = match group.len() {
            1 | 2 => log,
            _ => log.blocked(Some("Select one or two nodes")),
        };
        entries.push(log.into());
        let Some(scene) = &self.scene else {
            entries.push(grey(
                case(p, "Compare with HEAD"),
                "Open a repository first",
            ));
            entries.push(grey(
                case(p, "Compare with working tree"),
                "Open a repository first",
            ));
            return entries;
        };
        let oid = |n: usize| scene.repo.commit(scene.graph.nodes[n].commit).oid;
        let head = scene.repo.head_commit().map(|h| scene.repo.commit(h).oid);
        let compare = match *group {
            [a, b] => Item::new(
                case_with(
                    p,
                    "Compare {} with {}",
                    &[&node_name(scene, a), &node_name(scene, b)],
                ),
                Command::Compare(CompareRequest::Compare(oid(a), oid(b))),
            ),
            [a] => {
                let item = Item::new(
                    case(p, "Compare with HEAD"),
                    Command::Compare(CompareRequest::Mark(None)),
                );
                match head {
                    Some(h) if h != oid(a) => Item {
                        command: Command::Compare(CompareRequest::Compare(oid(a), h)),
                        ..item
                    },
                    Some(_) => item.blocked(Some("This is HEAD")),
                    None => item.blocked(Some("HEAD has no commit")),
                }
            }
            _ => Item::new(case(p, "Compare with HEAD"), Command::ShowLog)
                .blocked(Some("Select one or two nodes")),
        };
        entries.push(compare.into());
        let working_tree = Item::new(case(p, "Compare with working tree"), Command::ShowLog);
        let working_tree = match *group {
            _ if !scene.repo.has_working_tree => {
                working_tree.blocked(Some("A bare repository has no working tree"))
            }
            [a] => Item {
                command: Command::Compare(CompareRequest::WorkingTree(oid(a))),
                ..working_tree
            },
            _ => working_tree.blocked(Some("Select one node")),
        };
        entries.push(working_tree.into());
        // Each branch on the node against its upstream, from the upstream.
        let upstreams = match *group {
            [n] if self.settings.graph.show_upstreams => crate::upstreams::compare_items(scene, n),
            _ => Vec::new(),
        };
        let compare_upstream = |label: String, pair: crate::upstreams::ComparePair| -> Entry {
            match pair {
                Ok((up, branch)) => {
                    Item::new(label, Command::Compare(CompareRequest::Compare(up, branch))).into()
                }
                Err(why) => grey(label, why),
            }
        };
        let entry = match upstreams.as_slice() {
            [] => grey(
                case(p, "Compare with upstream"),
                if self.settings.graph.show_upstreams {
                    "No branch here has an upstream"
                } else {
                    "Upstreams are off (Settings, Advanced)"
                },
            ),
            [(u, pair)] => {
                compare_upstream(case_with(p, "Compare with {}", &[u.short_name()]), *pair)
            }
            several => Submenu::new(
                case(p, "Compare with upstream"),
                several
                    .iter()
                    .map(|(u, pair)| compare_upstream(u.short_name().to_owned(), *pair))
                    .collect(),
            )
            .into(),
        };
        entries.push(entry);
        entries
    }

    /// Fetch, pull the current branch, push the node's branches.
    fn remote_section(&self, c: &Ctx, commit: Option<Oid>) -> Vec<Entry> {
        let p = c.p;
        let mut entries = vec![Entry::Heading(case(p, "Remote"))];
        entries.push(
            Item::new(case(p, "Fetch"), Command::Fetch)
                .key(keys::fetch())
                .blocked(self.fetch_blocked())
                .into(),
        );
        // Pull is the current branch's, wherever the selection is.
        let pull_label = case_with(p, "Pull {}", &[&c.current]);
        let pull = c.catalog.and_then(|catalog| {
            let head = catalog.head?;
            let branch = parterre_core::remote::pull_offered(catalog, head)?;
            let stuck = catalog.stuck().map(|s| branches::capitalized(&s.reason()));
            let request = Request::Run(Action::Pull(Box::new(parterre_core::remote::Pull {
                branch: branch.name.clone(),
                head,
                how: None,
            })));
            let upstream = branch.upstream.clone().unwrap_or_default();
            Some(
                Item::new(pull_label.clone(), Command::Git(request))
                    .tip(format!("From {upstream}"))
                    .blocked(stuck.or(c.blocked().map(str::to_owned)))
                    .into(),
            )
        });
        entries.push(pull.unwrap_or_else(|| {
            grey(
                pull_label,
                c.blocked().unwrap_or("The current branch has no upstream"),
            )
        }));
        // Every branch on the node to every remote: one asking for a force push ends in "…".
        let pushes: Vec<Target> = match (c.catalog, commit, &self.repo) {
            (Some(catalog), Some(commit), Some(repo)) => catalog
                .locals
                .iter()
                .filter(|b| b.tip == commit)
                .flat_map(|b| {
                    parterre_core::remote::push_targets(repo, catalog, &b.name)
                        .into_iter()
                        .map(|(push, state)| {
                            use parterre_core::remote::PushState;
                            let name = format!("{} to {}", b.name, push.target());
                            let push = Request::Run(Action::Push(Box::new(push)));
                            match state {
                                PushState::Force => (format!("{name}…"), push, None),
                                PushState::UpToDate => (name, push, Some("Up to date".to_owned())),
                                PushState::New | PushState::Ahead => (name, push, None),
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .collect(),
            _ => Vec::new(),
        };
        let why = match c.catalog {
            Some(catalog) if catalog.remote_names.is_empty() => "The repository has no remote",
            _ => "No local branch here",
        };
        entries.push(targets(
            c,
            case(p, "Push"),
            |name| case_with(p, "Push {}", &[name]),
            c.label("Push {x}", "Push"),
            why,
            pushes,
        ));
        entries
    }
}

fn worktree_section(c: &Ctx, commit: Option<Oid>) -> Vec<Entry> {
    let p = c.p;
    let mut entries = vec![Entry::Heading(case(p, "Worktree"))];
    let (Some(catalog), Some(commit)) = (c.catalog, commit) else {
        let why = c.blocked().unwrap_or("Select a node");
        entries.push(grey(case(p, "Go to worktree"), why));
        entries.push(grey(c.label("Add worktree at {x}…", "Add worktree…"), why));
        entries.push(grey(case(p, "Delete worktree"), why));
        return entries;
    };
    entries.push(targets(
        c,
        case(p, "Go to worktree"),
        |name| case_with(p, "Go to worktree {}", &[name]),
        case(p, "Go to worktree"),
        "No other worktree is checked out here",
        branches::go_to_targets(catalog, commit),
    ));
    entries.push(
        Item::new(
            c.label("Add worktree at {x}…", "Add worktree…"),
            Command::Git(Request::AddWorktree { start: commit }),
        )
        .blocked(c.blocked())
        .into(),
    );
    entries.push(targets(
        c,
        case(p, "Delete worktree"),
        |name| case_with(p, "Delete worktree {}", &[name]),
        case(p, "Delete worktree"),
        "No other worktree is checked out here",
        branches::worktree_deletions(catalog, commit),
    ));
    entries
}

fn branch_section(c: &Ctx, app: &ParterreApp, commit: Option<Oid>) -> Vec<Entry> {
    let p = c.p;
    let mut entries = vec![Entry::Heading(case(p, "Branch"))];
    let (Some(catalog), Some(commit), Some(repo)) = (c.catalog, commit, &app.repo) else {
        let why = c.blocked().unwrap_or("Select a node");
        for label in [
            c.label("Switch to {x}", "Switch to branch"),
            c.label("Set upstream of {x}…", "Set upstream…"),
            c.label("Create branch at {x}…", "Create branch…"),
            c.label("Reset {current} to {x}…", "Reset {current}…"),
            c.label("Delete {x}", "Delete"),
        ] {
            entries.push(grey(label, why));
        }
        return entries;
    };
    let refs = branches::refs_at(repo, commit, catalog);
    entries.push(targets(
        c,
        case(p, "Switch to"),
        |name| case_with(p, "Switch to {}", &[name]),
        c.label("Switch to {x}", "Switch to branch"),
        if catalog.has_working_tree {
            "Nothing to switch to here"
        } else {
            "This repository has no working tree"
        },
        branches::switch_targets(repo, commit, &refs, catalog),
    ));
    let upstreams: Vec<Target> = if catalog.remote_names.is_empty() {
        Vec::new()
    } else {
        catalog
            .locals
            .iter()
            .filter(|b| b.tip == commit)
            .map(|b| {
                let request = Request::SetUpstream {
                    branch: b.name.clone(),
                };
                (b.name.clone(), request, None)
            })
            .collect()
    };
    entries.push(targets(
        c,
        case(p, "Set upstream of"),
        |name| case_with(p, "Set upstream of {}…", &[name]),
        c.label("Set upstream of {x}…", "Set upstream…"),
        if catalog.remote_names.is_empty() {
            "The repository has no remote"
        } else {
            "No local branch here"
        },
        upstreams,
    ));
    entries.push(
        Item::new(
            c.label("Create branch at {x}…", "Create branch…"),
            Command::Git(Request::Create {
                start: commit,
                track: None,
                switch: false,
            }),
        )
        .blocked(c.blocked())
        .into(),
    );
    let reset = match (
        catalog.stuck(),
        parterre_core::reset::branch(catalog, commit),
    ) {
        (Some(stuck), _) => {
            let branch = branches::stuck_branch(catalog);
            let label = c.label_on("Reset {current} to {x}…", "Reset {current}…", &branch);
            grey(label, &stuck.reason())
        }
        (None, Ok(branch)) => Item::new(
            c.label_on("Reset {current} to {x}…", "Reset {current}…", branch),
            Command::Git(Request::Reset {
                target: commit,
                mode: None,
            }),
        )
        .blocked(c.blocked())
        .into(),
        (None, Err(why)) => grey(c.label("Reset {current} to {x}…", "Reset {current}…"), &why),
    };
    entries.push(reset);
    entries.push(delete_entry(c, app, commit));
    entries
}

/// *Delete*: the node's local and remote-tracking branches, one item for one (*Delete
/// feature/x*), a submenu (Local x, Remote origin/x) for several.
fn delete_entry(c: &Ctx, app: &ParterreApp, commit: Oid) -> Entry {
    let p = c.p;
    let (Some(catalog), Some(repo)) = (c.catalog, &app.repo) else {
        return grey(
            c.label("Delete {x}", "Delete"),
            c.blocked().unwrap_or("Select a node"),
        );
    };
    let mut found: Vec<Target> = branches::deletable_locals(repo, catalog, commit)
        .into_iter()
        .map(|b| {
            let name = b.name.clone();
            (name, Request::Run(Action::DeleteBranches(vec![b])), None)
        })
        .collect();
    let locals = found.len();
    found.extend(
        branches::deletable_remotes(repo, catalog, commit)
            .into_iter()
            .map(|b| {
                let name = b.name();
                (
                    name,
                    Request::Run(Action::DeleteRemoteBranches(vec![b])),
                    None,
                )
            }),
    );
    if found.len() > 1 {
        // Local x, Remote origin/x.
        for (i, (name, _, _)) in found.iter_mut().enumerate() {
            let kind = if i < locals { "Local {}" } else { "Remote {}" };
            *name = case_with(p, kind, &[name]);
        }
    }
    let any_branch = repo.refs.iter().any(|r| {
        matches!(r.kind, RefKind::LocalBranch | RefKind::RemoteBranch)
            && repo.commit(r.target).oid == commit
    });
    targets(
        c,
        case(p, "Delete"),
        |name| case_with(p, "Delete {}", &[name]),
        c.label("Delete {x}", "Delete"),
        if any_branch {
            "The branch here is checked out"
        } else {
            "No branch here"
        },
        found,
    )
}

fn integrate_section(c: &Ctx, app: &ParterreApp, commit: Option<Oid>) -> Vec<Entry> {
    let p = c.p;
    let mut entries = vec![Entry::Heading(case(p, "Integrate"))];
    let merge_label = c.label("Merge {x} into {current}…", "Merge into {current}…");
    let merge_into_label = c.label("Merge {current} into {x}…", "Merge {current} into…");
    let pick_label = c.label(
        "Cherry-pick {x} onto {current}…",
        "Cherry-pick onto {current}…",
    );
    let rebase_label = c.label("Rebase {current} onto {x}", "Rebase {current}");
    let (Some(catalog), Some(commit), Some(repo)) = (c.catalog, commit, &app.repo) else {
        let why = c.blocked().unwrap_or("Select a node");
        for label in [merge_label, merge_into_label, pick_label, rebase_label] {
            entries.push(grey(label, why));
        }
        return entries;
    };
    let refs = branches::refs_at(repo, commit, catalog);
    let nothing = "Nothing to do here";
    entries.push(match branches::merge_offer(repo, commit, &refs, catalog) {
        Some((branch, found)) => targets(
            c,
            case_with(p, "Merge into {}", &[&branch]),
            |name| case_with(p, "Merge {} into {}…", &[name, &branch]),
            merge_label,
            nothing,
            found,
        ),
        None => grey(merge_label, "Nothing here to merge"),
    });
    entries.push(
        match branches::merge_into_offer(repo, commit, &refs, catalog) {
            Some((source, found)) => targets(
                c,
                case_with(p, "Merge {} into", &[&source]),
                |name| case_with(p, "Merge {} into {}…", &[&source, name]),
                merge_into_label,
                "No local branch here lacks its commits",
                found,
            ),
            None => grey(merge_into_label, "No local branch here lacks its commits"),
        },
    );
    entries.push(
        match branches::cherry_pick_offer(repo, commit, &refs, catalog) {
            Some((name, branch, request)) => {
                let stuck = catalog.stuck().map(|s| branches::capitalized(&s.reason()));
                Item::new(
                    case_with(p, "Cherry-pick {} onto {}…", &[&name, &branch]),
                    Command::Git(request),
                )
                .blocked(stuck.or(c.blocked().map(str::to_owned)))
                .into()
            }
            None => grey(pick_label, "No commit here to pick"),
        },
    );
    entries.push(match branches::rebase_offer(repo, commit, &refs, catalog) {
        Some((branch, found)) => targets(
            c,
            case_with(p, "Rebase {} onto", &[&branch]),
            |name| case_with(p, "Rebase {} onto {}", &[&branch, name]),
            rebase_label,
            nothing,
            found,
        ),
        None => grey(rebase_label, "Nothing to rebase onto here"),
    });
    entries
}

/// For a selection of several nodes: greys what acts on one. Fetch, and Pull of the current
/// branch, don't, and stay.
fn one_node(p: Platform, section: &mut [Entry]) {
    let why = || Some("Select one node".to_owned());
    for entry in section {
        match entry {
            Entry::Item(i)
                if matches!(i.command, Command::Fetch)
                    || i.label.starts_with(&case(p, "Pull ")) => {}
            Entry::Item(i) => {
                i.enabled = false;
                i.why = why();
            }
            Entry::Submenu(s) => {
                s.enabled = false;
                s.why = why();
            }
            _ => {}
        }
    }
}

/// What every one of the commits `oids` has to delete, as one item each: worktrees, then local
/// and remote-tracking branches.
fn group_deletions(
    repo: &Repo,
    catalog: &Catalog,
    oids: &[Oid],
    busy: bool,
) -> (Vec<Entry>, Vec<Entry>) {
    let p = Platform::CURRENT;
    let blocked = busy.then(|| loading_reason(true));
    // One item for them all when every node has some.
    let all = |each: Vec<Vec<String>>, many: &str, action: Action| -> Option<Entry> {
        if each.iter().any(Vec::is_empty) {
            return None;
        }
        let names: Vec<String> = each.into_iter().flatten().collect();
        let label = case_with(
            p,
            &format!("Delete {{}} {many}"),
            &[&names.len().to_string()],
        );
        Some(
            Item::new(label, Command::Git(Request::Run(action)))
                .tip(names.join(", "))
                .blocked(blocked)
                .into(),
        )
    };
    let worktrees: Vec<Vec<&parterre_core::branches::Worktree>> = oids
        .iter()
        .map(|&o| branches::deletable_worktrees(catalog, o))
        .collect();
    let unlocked = worktrees.iter().flatten().all(|w| w.locked.is_none());
    let worktree = unlocked
        .then(|| {
            let paths = worktrees.iter().flatten().map(|w| w.path.clone()).collect();
            all(
                names(&worktrees, |w| w.label()),
                "worktrees",
                Action::DeleteWorktrees {
                    paths,
                    branches: false,
                },
            )
        })
        .flatten();
    let locals: Vec<_> = oids
        .iter()
        .map(|&o| branches::deletable_locals(repo, catalog, o))
        .collect();
    let remotes: Vec<_> = oids
        .iter()
        .map(|&o| branches::deletable_remotes(repo, catalog, o))
        .collect();
    let branch = [
        all(
            names(&locals, |b| b.name.clone()),
            "local branches",
            Action::DeleteBranches(locals.iter().flatten().cloned().collect()),
        ),
        all(
            names(&remotes, |b| b.name()),
            "remote branches",
            Action::DeleteRemoteBranches(remotes.iter().flatten().cloned().collect()),
        ),
    ];
    (
        worktree.into_iter().collect(),
        branch.into_iter().flatten().collect(),
    )
}

/// The names of what each node has, all in one list per node.
fn names<T>(each: &[Vec<T>], name: impl Fn(&T) -> String) -> Vec<Vec<String>> {
    each.iter()
        .map(|found| found.iter().map(&name).collect())
        .collect()
}

/// A node's name in labels: its first branch, a local one first, else its first ref, else its
/// short hash.
fn node_name(scene: &crate::scene::Scene, node: usize) -> String {
    let n = &scene.graph.nodes[node];
    let repo = &scene.repo;
    n.refs
        .iter()
        .map(|&r| &repo.refs[r])
        .min_by_key(|r| match r.kind {
            RefKind::LocalBranch => 0,
            RefKind::RemoteBranch => 1,
            RefKind::Tag => 2,
            _ => 3,
        })
        .map(|r| r.name.clone())
        .unwrap_or_else(|| repo.commit(n.commit).oid.short(repo.abbrev_len))
}

#[cfg(test)]
mod tests {
    use super::{Command, Ctx, Entry, Item, case, fill, group_deletions, one_node};
    use crate::app::tool_harness::{git, init, load};
    use crate::keys::Platform;
    use parterre_core::Oid;

    fn label(entry: &Entry) -> String {
        match entry {
            Entry::Item(i) => i.label.clone(),
            Entry::Submenu(s) => s.label.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn several_nodes_offer_to_delete_what_they_all_have() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        init(p);
        git(p, &["commit", "-q", "--allow-empty", "-m", "base"]);
        git(p, &["branch", "a"]);
        git(p, &["commit", "-q", "--allow-empty", "-m", "next"]);
        git(p, &["branch", "b"]);
        git(p, &["commit", "-q", "--allow-empty", "-m", "tip"]);
        git(
            p,
            &["remote", "add", "origin", "https://example.com/demo.git"],
        );
        git(p, &["update-ref", "refs/remotes/origin/a", "a"]);
        git(p, &["update-ref", "refs/remotes/origin/b", "b"]);
        let (repo, catalog) = load(p);
        let oid = |name: &str| repo.commit(repo.resolve(name).unwrap()).oid;
        let labels = |oids: &[Oid]| {
            let (worktrees, branches) = group_deletions(&repo, &catalog, oids, false);
            worktrees
                .iter()
                .chain(&branches)
                .map(label)
                .collect::<Vec<_>>()
        };
        let p = Platform::CURRENT;
        assert_eq!(
            labels(&[oid("a"), oid("b")]),
            [
                case(p, "Delete 2 local branches"),
                case(p, "Delete 2 remote branches")
            ]
        );
        // The current branch can't go, and its commit has no remote-tracking branch.
        assert!(labels(&[oid("a"), oid("HEAD")]).is_empty());
    }

    fn ctx(p: Platform, x: Option<&str>) -> Ctx<'static> {
        Ctx {
            p,
            x: x.map(str::to_owned),
            current: "main".to_owned(),
            catalog: None,
            busy: false,
        }
    }

    #[test]
    fn several_nodes_grey_what_acts_on_one_but_fetch_and_pull() {
        let p = Platform::CURRENT;
        let mut section: Vec<Entry> = vec![
            Entry::Heading(case(p, "Remote")),
            Item::new(case(p, "Fetch"), Command::Fetch).into(),
            Item::new(case(p, "Pull main"), Command::ShowLog).into(),
            Item::new(case(p, "Push x to origin"), Command::ShowLog).into(),
        ];
        one_node(p, &mut section);
        let enabled: Vec<bool> = section
            .iter()
            .filter_map(|e| match e {
                Entry::Item(i) => Some(i.enabled),
                _ => None,
            })
            .collect();
        assert_eq!(enabled, [true, true, false]);
    }

    #[test]
    fn names_go_in_after_the_case() {
        let mac = ctx(Platform::Mac, Some("feature/x"));
        assert_eq!(
            mac.label("Merge {x} into {current}…", "Merge into {current}…"),
            "Merge feature/x into main…"
        );
        assert_eq!(
            mac.label("Create branch at {x}…", "Create branch…"),
            "Create Branch at feature/x…"
        );
        let none = ctx(Platform::Mac, None);
        assert_eq!(
            none.label("Add worktree at {x}…", "Add worktree…"),
            "Add Worktree…"
        );
        let linux = ctx(Platform::Linux, Some("v1"));
        assert_eq!(
            linux.label_on("Reset {current} to {x}…", "Reset {current}…", "dev"),
            "Reset dev to v1…"
        );
    }

    #[test]
    fn a_name_with_braces_stays() {
        assert_eq!(fill("Push {x}", "a{current}b", "main"), "Push a{current}b");
        assert_eq!(fill("{y} {x}", "a", "b"), "{y} a");
    }
}
