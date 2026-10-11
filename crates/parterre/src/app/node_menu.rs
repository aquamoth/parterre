//! A node's menu is the Git menu without what doesn't apply, then *Actions ›* and *Layout*.
//! Fetch isn't the node's, and Pull is offered on the current branch's node and its upstream's
//! only. Items held up by a running git operation stay, greyed out.

use parterre_core::RefKind;
use parterre_core::Worktree;

use super::compare_window::CompareRequest;
use super::git_menu::is_pull;
use super::menu_bar::{Command, Entry, Item, Submenu, Why, case, case_with};
use super::{Opener, ParterreApp};
use crate::keys::{self, Platform};

impl ParterreApp {
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

    pub(super) fn node_menu(&self, node: usize) -> Vec<Entry> {
        let p = Platform::CURRENT;
        let Some(scene) = &self.scene else {
            return Vec::new();
        };
        let group = self.selection.group_of(node);
        let oid = scene.node_oid(node);
        let pull_here = scene.graph.nodes[node].refs.iter().any(|&r| {
            let r = &scene.repo.refs[r];
            r.kind == RefKind::LocalBranch && r.is_head
        }) || self
            .branches
            .catalog
            .as_deref()
            .is_some_and(|c| parterre_core::remote::pull_offered(c, oid).is_some());
        let mut sections: Vec<(String, Vec<Entry>)> = Vec::new();
        for entry in self.git_menu() {
            match entry {
                Entry::Heading(heading) => sections.push((heading, Vec::new())),
                Entry::Separator => {}
                Entry::Item(i) if matches!(i.command, Some(Command::Fetch)) => {}
                Entry::Item(i) if is_pull(&i) && (!pull_here || group.len() > 1) => {}
                entry => {
                    if let Some((_, items)) = sections.last_mut() {
                        items.push(entry);
                    }
                }
            }
        }
        if group.len() == 1
            && let Some((_, history)) = sections.first_mut()
        {
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

    fn marks(&self, node: usize) -> Vec<Entry> {
        let p = Platform::CURRENT;
        let Some(scene) = &self.scene else {
            return Vec::new();
        };
        let oid = scene.node_oid(node);
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

    fn actions(&self, node: usize, selected_count: usize) -> Submenu {
        let p = Platform::CURRENT;
        let mut entries = Vec::new();
        let Some(scene) = &self.scene else {
            return Submenu::new(case(p, "Actions"), entries);
        };
        let n = &scene.graph.nodes[node];
        // With worktrees hidden, the open one is still at HEAD.
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
        // Alone selected, the node's hash is what Ctrl+C copies.
        entries.push(
            if selected_count > 1 {
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
        if self.pull_requests_active() && self.pull_requests.list().is_some() {
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

/// A greyed-out item goes, unless only a running git operation holds it up; a submenu keeps
/// what applies, and goes when nothing does.
fn applying(entry: Entry) -> Option<Entry> {
    let waiting = |why: &Option<Why>| why.as_ref().is_some_and(Why::is_waiting);
    match entry {
        Entry::Item(i) if !i.enabled && !waiting(&i.why_disabled) => None,
        Entry::Submenu(s) if !s.enabled && !waiting(&s.why_disabled) => None,
        Entry::Submenu(mut s) => {
            s.entries = s.entries.into_iter().filter_map(applying).collect();
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

    fn label(entry: &Entry) -> String {
        match entry {
            Entry::Item(i) => i.label.clone(),
            Entry::Submenu(s) => s.label.clone(),
            _ => String::new(),
        }
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
        let waiting = Item::new("e", Command::ShowLog).blocked(Some(Why::GitBusy));
        assert!(applying(waiting.into()).is_some());
    }
}
