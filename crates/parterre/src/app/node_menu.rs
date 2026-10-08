//! The graph's right-click menus (#323). A node's is the Git menu (`git_menu`) with what
//! doesn't apply left out: its sections, headings and order, then *Actions ›* and *Layout*.
//! Two departures: Fetch isn't the node's, so it stays in the menu bar, and Pull is offered on
//! the current branch's node only. Items held up by a running git operation stay, greyed out.
//! With several nodes selected, only what acts on all of them is offered.

use parterre_core::branches::{Action, Catalog};
use parterre_core::{Oid, RefKind, Repo, Worktree};

use super::Opener;
use super::ParterreApp;
use super::branches::{self, Request, loading_reason};
use super::compare_window::CompareRequest;
use super::menu_bar::{Command, Entry, Item, Submenu, case, case_with};
use crate::keys::{self, Platform};

impl ParterreApp {
    /// The menu of the canvas, away from the nodes.
    pub(super) fn canvas_menu(&self) -> Vec<Entry> {
        let p = Platform::CURRENT;
        let mut entries = vec![
            Item::new(case(p, "Zoom to fit"), Command::ZoomToFit)
                .key(keys::ZOOM_TO_FIT)
                .into(),
        ];
        if self.scene.as_ref().is_some_and(|s| s.net.any_displaced()) {
            entries.push(
                Item::new(
                    case(p, "Return all nodes to layout"),
                    Command::ReturnAllToLayout,
                )
                .into(),
            );
        }
        entries
    }

    /// The menu of `node`, which the selection holds.
    pub(super) fn node_menu(&self, node: usize) -> Vec<Entry> {
        let p = Platform::CURRENT;
        let Some(scene) = &self.scene else {
            return Vec::new();
        };
        let group: Vec<usize> = if self.selection.contains(node) {
            self.selection.nodes.clone()
        } else {
            vec![node]
        };
        let has_current = scene.graph.nodes[node].refs.iter().any(|&r| {
            let r = &scene.repo.refs[r];
            r.kind == RefKind::LocalBranch && r.is_head
        });
        let mut sections: Vec<(String, Vec<Entry>)> = Vec::new();
        for entry in self.git_menu() {
            match entry {
                Entry::Heading(heading) => sections.push((heading, Vec::new())),
                Entry::Separator => {}
                Entry::Item(i) if matches!(i.command, Command::Fetch) => {}
                Entry::Item(i) if is_pull(&i) && !has_current => {}
                entry => {
                    if let Some((_, items)) = sections.last_mut() {
                        items.push(entry);
                    }
                }
            }
        }
        if group.len() > 1 {
            // History has what several nodes share; the rest acts on one node.
            let (worktree, branch) = match (scene, self.branches.catalog.as_deref()) {
                (scene, Some(catalog)) => {
                    let oids: Vec<Oid> = group
                        .iter()
                        .map(|&n| scene.repo.commit(scene.graph.nodes[n].commit).oid)
                        .collect();
                    group_deletions(&scene.repo, catalog, &oids, self.branches.busy())
                }
                (_, None) => (Vec::new(), Vec::new()),
            };
            for (heading, items) in &mut sections {
                if *heading == case(p, "Worktree") {
                    *items = worktree.clone();
                } else if *heading == case(p, "Branch") {
                    *items = branch.clone();
                } else if *heading != case(p, "History") {
                    items.clear();
                }
            }
        } else if let Some((_, history)) = sections.first_mut() {
            history.extend(self.marks(node));
        }
        sections.push((String::new(), vec![self.actions(node, group.len()).into()]));
        sections.push((case(p, "Layout"), self.layout(node, group)));
        let mut entries = Vec::new();
        for (heading, items) in sections {
            let items: Vec<Entry> = items.into_iter().filter_map(applying).collect();
            if items.is_empty() {
                continue;
            }
            if !entries.is_empty() {
                entries.push(Entry::Separator);
            }
            if !heading.is_empty() {
                entries.push(Entry::Heading(heading));
            }
            entries.extend(items);
        }
        entries
    }

    /// *Mark for comparison* (or *Clear the mark*) and *Compare with marked*.
    fn marks(&self, node: usize) -> Vec<Entry> {
        let p = Platform::CURRENT;
        let Some(scene) = &self.scene else {
            return Vec::new();
        };
        let oid = scene.repo.commit(scene.graph.nodes[node].commit).oid;
        let marked = self.marked.as_ref();
        let mark = match marked {
            Some((m, _)) if *m == oid => Item::new(
                case(p, "Clear the mark"),
                Command::Compare(CompareRequest::Mark(None)),
            ),
            _ => Item::new(
                case(p, "Mark for comparison"),
                Command::Compare(CompareRequest::Mark(Some(oid))),
            ),
        };
        let mut entries = vec![mark.into()];
        if let Some((m, name)) = marked.filter(|(m, _)| *m != oid) {
            entries.push(
                Item::new(
                    case_with(p, "Compare with marked ({})", &[name]),
                    Command::Compare(CompareRequest::Compare(*m, oid)),
                )
                .into(),
            );
        }
        entries
    }

    /// *Actions ›*: copying, then opening, what the node right-clicked has.
    fn actions(&self, node: usize, selected: usize) -> Submenu {
        let p = Platform::CURRENT;
        let mut entries = Vec::new();
        let Some(scene) = &self.scene else {
            return Submenu::new(case(p, "Actions"), entries);
        };
        let n = &scene.graph.nodes[node];
        // The worktrees shown on the node, the open one at HEAD among them. Hidden, the open
        // one is still at HEAD.
        let worktrees: Vec<&Worktree> = if self.settings.graph.show_worktrees {
            scene
                .worktrees_on(node)
                .into_iter()
                .map(|k| &scene.repo.worktrees[k])
                .collect()
        } else {
            scene
                .repo
                .worktrees
                .iter()
                .filter(|w| w.open && n.is_head)
                .collect()
        };
        let worktrees: Vec<&Worktree> = worktrees.into_iter().filter(|w| !w.missing).collect();
        // One worktree: the item. Several: a submenu naming them.
        let each = |label: &str, command: &dyn Fn(&Worktree) -> Command| -> Option<Entry> {
            match worktrees.as_slice() {
                [] => None,
                [w] => Some(
                    Item::new(case(p, label), command(w))
                        .tip(w.path.display().to_string())
                        .into(),
                ),
                several => Some(
                    Submenu::new(
                        case(p, label),
                        several
                            .iter()
                            .map(|w| {
                                Item::new(w.label(), command(w))
                                    .tip(w.path.display().to_string())
                                    .into()
                            })
                            .collect(),
                    )
                    .into(),
                ),
            }
        };
        let oid = scene.repo.commit(n.commit).oid;
        let hash = Item::new(case(p, "Copy commit hash"), Command::CopyText(oid.to_hex()));
        // Right-clicking selects the node alone, and then Ctrl+C copies the same hash.
        entries.push(
            if selected > 1 {
                hash
            } else {
                hash.key(keys::COPY)
            }
            .into(),
        );
        if !n.refs.is_empty() {
            let names: Vec<&str> = n
                .refs
                .iter()
                .map(|&r| scene.repo.refs[r].full_name.as_str())
                .collect();
            entries.push(
                Item::new(
                    case(p, "Copy ref names"),
                    Command::CopyText(names.join("\n")),
                )
                .into(),
            );
        }
        entries.extend(each("Copy folder path", &|w| {
            Command::CopyText(w.path.display().to_string())
        }));
        let mut open = Vec::new();
        let pull_requests_shown = self.settings.graph.show_pull_requests
            && self.pull_requests.origin().is_some()
            && !self.pull_requests.needs_sign_in()
            && self.pull_requests.list().is_some();
        if pull_requests_shown {
            let item = |label: String, i: usize| -> Entry {
                let pr = &scene.pull_requests[i];
                Item::new(label, Command::OpenPullRequest(pr.url.clone()))
                    .tip(pr.title.clone())
                    .into()
            };
            let number = |i: usize| scene.pull_requests[i].number.to_string();
            match n.pull_requests.as_slice() {
                [] => {}
                &[i] => open.push(item(
                    case_with(p, "Open pull request #{}", &[&number(i)]),
                    i,
                )),
                several => open.push(
                    Submenu::new(
                        case(p, "Open pull request"),
                        several
                            .iter()
                            .map(|&i| item(format!("#{}", number(i)), i))
                            .collect(),
                    )
                    .into(),
                ),
            }
        }
        open.extend(each("Open in file manager", &|w| {
            Command::OpenIn(Opener::FileManager, w.path.clone())
        }));
        open.extend(each("Open in terminal", &|w| {
            Command::OpenIn(Opener::Terminal, w.path.clone())
        }));
        if !open.is_empty() {
            entries.push(Entry::Separator);
            entries.extend(open);
        }
        Submenu::new(case(p, "Actions"), entries)
    }

    /// Selecting the subtree, returning the selection to the layout, centring on the node.
    fn layout(&self, node: usize, group: Vec<usize>) -> Vec<Entry> {
        let p = Platform::CURRENT;
        let Some(scene) = &self.scene else {
            return Vec::new();
        };
        let displaced: Vec<usize> = group
            .iter()
            .copied()
            .filter(|&n| scene.net.is_displaced(n))
            .collect();
        let back = if group.len() > 1 {
            "Return selection to layout"
        } else {
            "Return node to layout"
        };
        let mut entries = vec![
            Item::new(case(p, "Select subtree"), Command::SelectSubtree(group))
                .tip("Select everything that grows out of this (first-parent descendants)")
                .into(),
        ];
        if !displaced.is_empty() {
            entries.push(Item::new(case(p, back), Command::ReturnToLayout(displaced)).into());
        }
        entries.push(Item::new(case(p, "Centre view here"), Command::CentreOn(node)).into());
        entries
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

/// The Git menu's *Pull ‹current branch›*.
fn is_pull(item: &Item) -> bool {
    matches!(&item.command, Command::Git(Request::Run(Action::Pull(_))))
        || item.label.starts_with(&case(Platform::CURRENT, "Pull "))
}

/// The names of what each node has, all in one list per node.
fn names<T>(each: &[Vec<T>], name: impl Fn(&T) -> String) -> Vec<Vec<String>> {
    each.iter()
        .map(|found| found.iter().map(&name).collect())
        .collect()
}

/// `entry` without what doesn't apply: a greyed-out item goes, unless only a running git
/// operation holds it up; a submenu keeps what applies, and goes when nothing does.
fn applying(entry: Entry) -> Option<Entry> {
    let waiting = |why: &Option<String>| {
        why.as_deref()
            .is_some_and(|w| w == loading_reason(true) || w == loading_reason(false))
    };
    match entry {
        Entry::Item(i) if !i.enabled && !waiting(&i.why) => None,
        Entry::Submenu(s) if !s.enabled && !waiting(&s.why) => None,
        Entry::Submenu(mut s) => {
            s.entries = s.entries.into_iter().filter_map(applying).collect();
            // A separator left first or last by what went.
            while matches!(s.entries.last(), Some(Entry::Separator)) {
                s.entries.pop();
            }
            while matches!(s.entries.first(), Some(Entry::Separator)) {
                s.entries.remove(0);
            }
            (!s.entries.is_empty()).then_some(Entry::Submenu(s))
        }
        entry => Some(entry),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tool_harness::{git, init, load};

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

    #[test]
    fn what_does_not_apply_goes_unless_git_is_busy() {
        let item = |label: &str, why: Option<&str>| -> Entry {
            Item::new(label, Command::ShowLog).blocked(why).into()
        };
        let actions = Submenu::new(
            "Actions",
            vec![item("a", Some("No")), Entry::Separator, item("b", None)],
        );
        let Some(Entry::Submenu(kept)) = applying(actions.into()) else {
            panic!("b applies")
        };
        assert_eq!(kept.entries.iter().map(label).collect::<Vec<_>>(), ["b"]);
        let empty = Submenu::new("Open", vec![item("c", Some("No"))]);
        assert!(applying(empty.into()).is_none());
        assert!(applying(item("d", Some("Up to date"))).is_none());
        assert!(applying(item("e", Some(loading_reason(true)))).is_some());
    }
}
