//! PROTOTYPE (#323) – throwaway, never for main. The graph's right-click menu as the Git menu
//! with what doesn't apply left out (variant A, chosen in round one), switched with the
//! floating bar at the bottom of the window (or `[` and `]`), or chosen at start with
//! `PARTERRE_PROTO_MENU=a1|a2|today`:
//!
//! - A1: Open in › and Copy › under an *Actions* heading;
//! - A2: one *Actions ›* submenu with all their choices;
//! - A+C: A1, narrowed to the branch whose label was right-clicked (#144's variant C); the
//!   node's body or a tag gets A1 as it is. *Set upstream…* and deleting `origin/X` go under
//!   *Advanced ›* after the Git sections;
//! - Today: the menu as on main.
//!
//! The Git menu's own sections, in its order and with its headings, then Actions, then
//! Layout. Two departures from the Git menu: Fetch is the canvas's, and Pull is offered on the
//! current branch's node only.

use std::path::PathBuf;

use eframe::egui::{self, Align2, Color32, Key, RichText, vec2};
use parterre_core::RefKind;

use super::menu_bar::{Command, Entry, Item, Submenu};
use super::{Opener, ParterreApp};
use crate::keys;
use crate::scene::RowKind;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Variant {
    A1,
    A2,
    AC,
    Today,
}

impl Variant {
    const ALL: [Variant; 4] = [Variant::A1, Variant::A2, Variant::AC, Variant::Today];

    pub fn from_env() -> Variant {
        let v = std::env::var("PARTERRE_PROTO_MENU").unwrap_or_default();
        match v.to_lowercase().as_str() {
            "a2" => Variant::A2,
            "ac" => Variant::AC,
            "today" => Variant::Today,
            _ => Variant::A1,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Variant::A1 => "A1 – Actions section",
            Variant::A2 => "A2 – Actions submenu",
            Variant::AC => "A+C – The label narrows A1",
            Variant::Today => "Today – as on main",
        }
    }

    fn step(self, by: isize) -> Variant {
        let i = Variant::ALL.iter().position(|&v| v == self).unwrap_or(0) as isize;
        Variant::ALL[(i + by).rem_euclid(Variant::ALL.len() as isize) as usize]
    }
}

/// What the menu's own items do, beyond the menu bar's commands.
#[derive(Clone, Debug)]
pub enum Action {
    OpenPullRequest(String),
    Open(Opener, PathBuf),
    Copy(String),
    SelectSubtree(Vec<usize>),
    ReturnToLayout(Vec<usize>),
    Centre(usize),
}

/// The floating switcher at the bottom centre.
pub fn bar(ctx: &egui::Context, variant: &mut Variant) {
    if !ctx.egui_wants_keyboard_input() {
        if ctx.input(|i| i.key_pressed(Key::OpenBracket)) {
            *variant = variant.step(-1);
        }
        if ctx.input(|i| i.key_pressed(Key::CloseBracket)) {
            *variant = variant.step(1);
        }
    }
    egui::Area::new(egui::Id::new("prototype-context-menu-bar"))
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -40.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgb(30, 30, 30))
                .corner_radius(18.0)
                .inner_margin(vec2(10.0, 6.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let arrow = |t: &str| {
                            egui::Button::new(RichText::new(t).color(Color32::WHITE).strong())
                                .fill(Color32::from_rgb(60, 60, 60))
                        };
                        if ui.add(arrow(" < ")).on_hover_text("[").clicked() {
                            *variant = variant.step(-1);
                        }
                        ui.label(
                            RichText::new(format!(
                                "PROTOTYPE right-click menu: {}",
                                variant.name()
                            ))
                            .color(Color32::WHITE),
                        );
                        if ui.add(arrow(" > ")).on_hover_text("]").clicked() {
                            *variant = variant.step(1);
                        }
                    });
                });
        });
}

fn act(label: impl Into<String>, action: Action) -> Item {
    Item::new(label, Command::Proto(action))
}

impl ParterreApp {
    /// The menu for `node`, or the canvas's when `None`.
    pub(super) fn proto_menu(&self, variant: Variant, node: Option<usize>) -> Vec<Entry> {
        match node {
            None => self.proto_canvas(),
            Some(node) => self.proto_node(variant, node),
        }
    }

    pub(super) fn proto_run(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::OpenPullRequest(url) => {
                if let Err(e) = crate::browser::open(&url) {
                    self.status = Some((e, true));
                }
            }
            Action::Open(opener, dir) => self.open_in(opener, &dir),
            Action::Copy(text) => ctx.copy_text(text),
            Action::SelectSubtree(roots) => self.select_subtree(&roots),
            Action::ReturnToLayout(nodes) => self.return_to_layout(&nodes),
            Action::Centre(node) => self.center_on(node),
        }
    }

    fn proto_canvas(&self) -> Vec<Entry> {
        let displaced = self
            .scene
            .as_ref()
            .is_some_and(|s| (0..s.node_count()).any(|n| s.net.is_displaced(n)));
        let mut out = vec![
            Item::new("Zoom to fit", Command::ZoomToFit)
                .key(keys::ZOOM_TO_FIT)
                .into(),
            Item::new("Go to HEAD", Command::GoToHead)
                .key(keys::GO_TO_HEAD)
                .into(),
        ];
        if self.branches.fetch_blocked().is_none() {
            out.push(Entry::Separator);
            out.push(Item::new("Fetch", Command::Fetch).key(keys::fetch()).into());
        }
        if displaced {
            out.push(Entry::Separator);
            out.push(Item::new("Return all nodes to layout", Command::ReturnAllToLayout).into());
        }
        out
    }

    fn proto_node(&self, variant: Variant, node: usize) -> Vec<Entry> {
        let Some(scene) = &self.scene else {
            return Vec::new();
        };
        let has_current = scene.graph.nodes[node].refs.iter().any(|&r| {
            let r = &scene.repo.refs[r];
            r.kind == RefKind::LocalBranch && r.is_head
        });
        // The Git menu's sections.
        let mut sections: Vec<(String, Vec<Entry>)> = Vec::new();
        for e in self.git_menu() {
            match e {
                Entry::Heading(h) => sections.push((h, Vec::new())),
                Entry::Separator => {}
                Entry::Item(i) if matches!(i.command, Command::Fetch) => {}
                Entry::Item(i) if i.label.starts_with("Pull ") && !has_current => {}
                e => {
                    if let Some(last) = sections.last_mut() {
                        last.1.push(e)
                    }
                }
            }
        }
        if variant == Variant::AC {
            self.narrow(node, &mut sections);
        }
        let actions = self.actions(node);
        match variant {
            Variant::A2 => {
                let flat = actions.into_iter().flat_map(flatten).collect();
                sections.push((String::new(), vec![Submenu::new("Actions", flat).into()]));
            }
            _ => sections.push(("Actions".to_owned(), actions)),
        }
        sections.push(("Layout".to_owned(), self.layout(node)));
        let mut out = Vec::new();
        for (heading, items) in sections {
            let items: Vec<Entry> = items.into_iter().filter_map(applying).collect();
            if items.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push(Entry::Separator);
            }
            if !heading.is_empty() {
                out.push(Entry::Heading(heading));
            }
            out.extend(items);
        }
        out
    }

    /// A+C: Branch, Integrate and Remote narrowed to the branch whose label was right-clicked;
    /// Set upstream and deleting remote-tracking branches moved to Advanced ›.
    fn narrow(&self, node: usize, sections: &mut Vec<(String, Vec<Entry>)>) {
        let Some(scene) = &self.scene else { return };
        let is_remote = |name: &str| {
            scene
                .repo
                .refs
                .iter()
                .any(|r| r.kind == RefKind::RemoteBranch && r.name == name)
        };
        let advanced = |i: &Item| {
            i.label.starts_with("Set upstream")
                || i.label
                    .strip_prefix("Delete ")
                    .is_some_and(|n| is_remote(n.trim_end_matches('…')))
        };
        // The branch right-clicked, and whether it's the current one.
        let label = match self.proto_row.as_ref().map(|r| (&r.label, &r.kind)) {
            Some((
                label,
                RowKind::Ref {
                    kind: kind @ (RefKind::LocalBranch | RefKind::RemoteBranch),
                    head,
                    ..
                },
            )) => Some((label.clone(), *kind == RefKind::LocalBranch && *head)),
            _ => None,
        };
        let x = super::git_menu::node_name(scene, node);
        let mut moved: Vec<Entry> = Vec::new();
        for (index, (_, items)) in sections.iter_mut().enumerate() {
            // Branch, Integrate, Remote.
            if !(2..=4).contains(&index) {
                continue;
            }
            let all = std::mem::take(items);
            let Some((l, _current)) = &label else {
                // The body: as A1, but Advanced's items moved there.
                for e in all {
                    match e {
                        Entry::Item(i) if advanced(&i) => moved.push(i.into()),
                        Entry::Submenu(s) => {
                            let (adv, rest): (Vec<Entry>, Vec<Entry>) =
                                s.entries.iter().cloned().partition(|c| match c {
                                    Entry::Item(i) => advanced(&Item {
                                        label: spell(&s.label, &i.label),
                                        ..i.clone()
                                    }),
                                    _ => false,
                                });
                            for c in adv {
                                if let Entry::Item(mut i) = c {
                                    i.label = spell(&s.label, &i.label);
                                    moved.push(i.into());
                                }
                            }
                            if !rest.is_empty() {
                                items.push(Submenu { entries: rest, ..s }.into());
                            }
                        }
                        e => items.push(e),
                    }
                }
                continue;
            };
            // The label's remote-tracking branches on this node, to delete under Advanced.
            let remotes: Vec<String> = scene.graph.nodes[node]
                .refs
                .iter()
                .map(|&r| &scene.repo.refs[r])
                .filter(|r| r.kind == RefKind::RemoteBranch && r.name.ends_with(&format!("/{l}")))
                .map(|r| r.name.clone())
                .collect();
            for mut i in all.into_iter().flat_map(flatten_named) {
                let commit_level =
                    i.label.starts_with("Create branch") || i.label.starts_with("Reset ");
                if commit_level {
                    // Named after the label right-clicked rather than the node's first branch.
                    i.label = i
                        .label
                        .split(' ')
                        .map(|w| match w.strip_suffix('…') {
                            Some(w) if w == x => format!("{l}…"),
                            _ if w == x => l.clone(),
                            _ => w.to_owned(),
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    items.push(i.into());
                } else if mentions(&i.label, l) {
                    if advanced(&i) {
                        moved.push(i.into());
                    } else {
                        items.push(i.into());
                    }
                } else if advanced(&i) && remotes.iter().any(|r| mentions(&i.label, r)) {
                    moved.push(i.into());
                }
            }
        }
        if !moved.is_empty() {
            sections.push((String::new(), vec![Submenu::new("Advanced", moved).into()]));
        }
    }

    /// Open in › and Copy ›, for the node right-clicked.
    fn actions(&self, node: usize) -> Vec<Entry> {
        let Some(scene) = &self.scene else {
            return Vec::new();
        };
        let n = &scene.graph.nodes[node];
        let worktrees_shown = self.settings.graph.show_worktrees;
        let open_worktree = scene.repo.worktrees.iter().find(|w| w.open);
        let worktrees: Vec<&parterre_core::Worktree> = if worktrees_shown {
            scene
                .worktrees_on(node)
                .into_iter()
                .map(|k| &scene.repo.worktrees[k])
                .filter(|w| !w.missing)
                .collect()
        } else {
            open_worktree.filter(|_| n.is_head).into_iter().collect()
        };
        let pull_requests_shown = self.settings.graph.show_pull_requests
            && self.pull_requests.origin().is_some()
            && !self.pull_requests.needs_sign_in()
            && self.pull_requests.list().is_some();
        // One worktree: the item. Several: a submenu naming them.
        let per_worktree =
            |label: &str, each: &dyn Fn(&parterre_core::Worktree) -> Action| -> Option<Entry> {
                match worktrees.as_slice() {
                    [] => None,
                    [w] => Some(act(label, each(w)).tip(w.path.display().to_string()).into()),
                    several => Some(
                        Submenu::new(
                            label,
                            several
                                .iter()
                                .map(|w| act(w.name(), each(w)).into())
                                .collect(),
                        )
                        .into(),
                    ),
                }
            };
        let mut open: Vec<Entry> = Vec::new();
        if pull_requests_shown {
            let item = |label: String, i: usize| -> Entry {
                let pr = &scene.pull_requests[i];
                act(label, Action::OpenPullRequest(pr.url.clone()))
                    .tip(pr.title.clone())
                    .into()
            };
            match n.pull_requests.as_slice() {
                [] => {}
                &[i] => open.push(item(
                    format!("Pull request #{}", scene.pull_requests[i].number),
                    i,
                )),
                several => open.push(
                    Submenu::new(
                        "Pull request",
                        several
                            .iter()
                            .map(|&i| item(format!("#{}", scene.pull_requests[i].number), i))
                            .collect(),
                    )
                    .into(),
                ),
            }
        }
        open.extend(per_worktree("File manager", &|w| {
            Action::Open(Opener::FileManager, w.path.clone())
        }));
        open.extend(per_worktree("Terminal", &|w| {
            Action::Open(Opener::Terminal, w.path.clone())
        }));
        let commit = scene.repo.commit(n.commit);
        let mut copy: Vec<Entry> = vec![
            act("Commit hash", Action::Copy(commit.oid.to_hex()))
                .key(keys::COPY)
                .into(),
        ];
        if !n.refs.is_empty() {
            let names: Vec<&str> = n
                .refs
                .iter()
                .map(|&r| scene.repo.refs[r].full_name.as_str())
                .collect();
            copy.push(act("Ref names", Action::Copy(names.join("\n"))).into());
        }
        copy.extend(per_worktree("Folder path", &|w| {
            Action::Copy(w.path.display().to_string())
        }));
        let mut out = Vec::new();
        if !open.is_empty() {
            out.push(Submenu::new("Open in", open).into());
        }
        out.push(Submenu::new("Copy", copy).into());
        out
    }

    fn layout(&self, node: usize) -> Vec<Entry> {
        let Some(scene) = &self.scene else {
            return Vec::new();
        };
        let group = if self.selection.contains(node) {
            self.selection.nodes.clone()
        } else {
            vec![node]
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
        let mut out: Vec<Entry> = vec![
            act("Select subtree", Action::SelectSubtree(group))
                .tip("Select everything that grows out of this (first-parent descendants)")
                .into(),
        ];
        if !displaced.is_empty() {
            out.push(act(back, Action::ReturnToLayout(displaced)).into());
        }
        out.push(act("Centre view here", Action::Centre(node)).into());
        out
    }
}

/// `e` without what doesn't apply: a greyed item is left out, a submenu keeps what applies
/// and goes when nothing does.
fn applying(e: Entry) -> Option<Entry> {
    match e {
        Entry::Item(i) if !i.enabled => None,
        Entry::Submenu(s) if !s.enabled => None,
        Entry::Submenu(mut s) => {
            s.entries = s.entries.into_iter().filter_map(applying).collect();
            (!s.entries.is_empty()).then_some(Entry::Submenu(s))
        }
        e => Some(e),
    }
}

/// A2: Open in › File manager is *Open in file manager*, Copy › Commit hash *Copy commit hash*.
fn flatten(e: Entry) -> Vec<Entry> {
    let Entry::Submenu(s) = e else {
        return vec![e];
    };
    let verb = s.label;
    let name = |label: &str| {
        let mut chars = label.chars();
        let lower: String = match chars.next() {
            Some(f) => f.to_lowercase().chain(chars).collect(),
            None => String::new(),
        };
        if verb == "Open in" && label.starts_with("Pull request") {
            format!("Open {lower}")
        } else {
            format!("{verb} {lower}")
        }
    };
    s.entries
        .into_iter()
        .map(|c| match c {
            Entry::Item(mut i) => {
                i.label = name(&i.label);
                Entry::Item(i)
            }
            Entry::Submenu(mut sub) => {
                sub.label = name(&sub.label);
                Entry::Submenu(sub)
            }
            c => c,
        })
        .collect()
}

/// `label` names `name` as a word of its own.
fn mentions(label: &str, name: &str) -> bool {
    label
        .split_whitespace()
        .any(|w| w.trim_end_matches('…') == name)
}

/// An item, or a submenu's items named in full: *Merge into main › x* is *Merge x into main…*.
fn flatten_named(e: Entry) -> Vec<Item> {
    match e {
        Entry::Item(i) => vec![i],
        Entry::Submenu(s) => s
            .entries
            .into_iter()
            .filter_map(|c| match c {
                Entry::Item(mut i) => {
                    i.label = spell(&s.label, &i.label);
                    if !s.enabled {
                        i.enabled = false;
                        i.why = s.why.clone();
                    }
                    Some(i)
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn spell(verb: &str, name: &str) -> String {
    if let Some(current) = verb.strip_prefix("Merge into ") {
        return format!("Merge {name} into {current}…");
    }
    if verb == "Delete" {
        let n = name
            .strip_prefix("Local ")
            .or_else(|| name.strip_prefix("Remote "))
            .unwrap_or(name);
        return format!("Delete {n}");
    }
    if verb == "Set upstream of" {
        return format!("Set upstream of {name}…");
    }
    if verb.starts_with("Merge ") && verb.ends_with(" into") {
        return format!("{verb} {name}…");
    }
    format!("{verb} {name}")
}
