//! PROTOTYPE (#323) – throwaway, never for main. Layouts of the graph's right-click menu,
//! switched with the floating bar at the bottom of the window (or `[` and `]`), or chosen at
//! start with `PARTERRE_PROTO_MENU=a|b|c|today`:
//!
//! - A: the Git menu's sections in its order, headed, with only what applies;
//! - B: short and always the same shape: the Git menu's sections as submenus;
//! - C: the label right-clicked decides (#144's variant C), *Advanced ›* at the bottom;
//! - Today: the menu as on main.
//!
//! Every variant is cut from the menu bar's Git menu (`git_menu`), so its items open their real
//! dialogs. With several nodes selected, A–C offer only what acts on the whole selection.

use eframe::egui::{self, Align2, Color32, Key, RichText, vec2};
use parterre_core::RefKind;

use super::ParterreApp;
use super::branches::loading_reason;
use super::compare_window::CompareRequest;
use super::menu_bar::{Command, Entry, Item, Submenu};
use crate::keys;
use crate::scene::{Row, RowKind};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Variant {
    A,
    B,
    C,
    Today,
}

impl Variant {
    const ALL: [Variant; 4] = [Variant::A, Variant::B, Variant::C, Variant::Today];

    pub fn from_env() -> Variant {
        let v = std::env::var("PARTERRE_PROTO_MENU").unwrap_or_default();
        match v.to_lowercase().as_str() {
            "b" => Variant::B,
            "c" => Variant::C,
            "today" => Variant::Today,
            _ => Variant::A,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Variant::A => "A – Mirror the Git menu",
            Variant::B => "B – Short, same shape",
            Variant::C => "C – The label decides",
            Variant::Today => "Today – as on main",
        }
    }

    fn step(self, by: isize) -> Variant {
        let i = Variant::ALL.iter().position(|&v| v == self).unwrap_or(0) as isize;
        Variant::ALL[(i + by).rem_euclid(Variant::ALL.len() as isize) as usize]
    }
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
                .shadow(egui::Shadow {
                    offset: [0, 2],
                    blur: 10,
                    spread: 0,
                    color: Color32::from_black_alpha(80),
                })
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

/// A, B or C's menu for a node, or the canvas when `node` is `None`.
pub struct ProtoMenu {
    pub variant: Variant,
    pub entries: Vec<Entry>,
}

impl ParterreApp {
    pub(super) fn proto_menu(&self, variant: Variant, node: Option<usize>) -> ProtoMenu {
        let entries = match node {
            None => self.proto_canvas(),
            Some(_) if self.selection.len() > 1 => self.proto_several(),
            Some(node) => match variant {
                Variant::A => self.proto_a(node),
                Variant::B => self.proto_b(node),
                _ => self.proto_c(node),
            },
        };
        ProtoMenu { variant, entries }
    }

    fn proto_canvas(&self) -> Vec<Entry> {
        vec![
            Item::new("Zoom to fit", Command::ZoomToFit)
                .key(keys::ZOOM_TO_FIT)
                .into(),
            Item::new("Go to HEAD", Command::GoToHead)
                .key(keys::GO_TO_HEAD)
                .into(),
            Entry::Separator,
            Item::new("Fetch", Command::Fetch)
                .key(keys::fetch())
                .blocked(self.branches.fetch_blocked())
                .into(),
            Entry::Separator,
            Item::new("Return all nodes to layout", Command::ReturnAllToLayout).into(),
        ]
    }

    /// The Git menu's sections, without their headings: History, Worktree, Branch, Integrate
    /// and Remote, with Fetch left to the canvas and Pull to the current branch's node.
    fn sections(&self, node: usize) -> Vec<(String, Vec<Entry>)> {
        let has_current = self.scene.as_ref().is_some_and(|s| {
            s.graph.nodes[node].refs.iter().any(|&r| {
                let r = &s.repo.refs[r];
                r.kind == RefKind::LocalBranch && r.is_head
            })
        });
        let mut out: Vec<(String, Vec<Entry>)> = Vec::new();
        for e in self.git_menu() {
            match e {
                Entry::Heading(h) => out.push((h, Vec::new())),
                Entry::Separator => {}
                Entry::Item(i) if matches!(i.command, Command::Fetch) => {}
                Entry::Item(i) if i.label.starts_with("Pull ") && !has_current => {}
                e => {
                    if let Some(last) = out.last_mut() {
                        last.1.push(e)
                    }
                }
            }
        }
        out
    }

    fn marks(&self, node: usize) -> Vec<Entry> {
        let Some(scene) = &self.scene else {
            return Vec::new();
        };
        let oid = scene.repo.commit(scene.graph.nodes[node].commit).oid;
        let is_marked = self.marked.as_ref().is_some_and(|(m, _)| *m == oid);
        let mark = if is_marked {
            Item::new(
                "Clear the mark",
                Command::Compare(CompareRequest::Mark(None)),
            )
        } else {
            Item::new(
                "Mark for comparison",
                Command::Compare(CompareRequest::Mark(Some(oid))),
            )
        };
        let with = match self.marked.as_ref().filter(|(m, _)| *m != oid) {
            Some((m, name)) => Item::new(
                format!("Compare with marked ({name})"),
                Command::Compare(CompareRequest::Compare(*m, oid)),
            ),
            None => {
                Item::new("Compare with marked", Command::ShowLog).blocked(Some(if is_marked {
                    "This is the marked commit"
                } else {
                    "Mark a commit for comparison first"
                }))
            }
        };
        vec![mark.into(), with.into()]
    }

    /// Several nodes: only what acts on all of them.
    fn proto_several(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        for e in self.git_menu() {
            if let Entry::Item(i) = &e {
                let all = i.label.starts_with("Show log")
                    || (i.label.starts_with("Compare ") && i.label.contains(" with "))
                    || (i.label.starts_with("Delete ") && i.label.ends_with(" branches"));
                if all && i.enabled {
                    out.push(e);
                }
            }
        }
        out
    }

    fn proto_a(&self, node: usize) -> Vec<Entry> {
        let mut out = Vec::new();
        for (i, (heading, items)) in self.sections(node).into_iter().enumerate() {
            let mut items: Vec<Entry> = items.into_iter().filter(applies).collect();
            if i == 0 {
                items.extend(self.marks(node).into_iter().filter(applies));
            }
            if items.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push(Entry::Separator);
            }
            out.push(Entry::Heading(heading));
            out.extend(items);
        }
        out
    }

    /// Show log and the Compare submenu, as B and C start.
    fn proto_history(&self, node: usize, history: Vec<Entry>) -> Vec<Entry> {
        let mut history = history.into_iter();
        let mut out: Vec<Entry> = history.next().into_iter().collect();
        let mut compare: Vec<Entry> = history.collect();
        compare.push(Entry::Separator);
        compare.extend(self.marks(node));
        out.push(Submenu::new("Compare", compare).into());
        out
    }

    fn proto_b(&self, node: usize) -> Vec<Entry> {
        let mut s = self.sections(node).into_iter().map(|(_, items)| items);
        let (history, worktree, branch, integrate, remote) = (
            s.next().unwrap_or_default(),
            s.next().unwrap_or_default(),
            s.next().unwrap_or_default(),
            s.next().unwrap_or_default(),
            s.next().unwrap_or_default(),
        );
        let mut out = self.proto_history(node, history);
        out.push(Entry::Separator);
        // Branch: Switch to, Set upstream, Create branch, Reset, Delete.
        let mut branch = branch.into_iter();
        let switch = branch.next();
        let upstream = branch.next();
        let create = branch.next();
        out.extend(switch);
        out.extend(create);
        out.push(Entry::Separator);
        let rest: Vec<Entry> = upstream.into_iter().chain(branch).collect();
        out.push(sub("Branch", rest));
        out.push(sub("Integrate", integrate));
        out.push(sub("Remote", remote));
        out.push(sub("Worktree", worktree));
        out
    }

    fn proto_c(&self, node: usize) -> Vec<Entry> {
        let Some(scene) = &self.scene else {
            return Vec::new();
        };
        let mut s = self.sections(node).into_iter().map(|(_, items)| items);
        let (history, worktrees, branch, integrate, remote) = (
            s.next().unwrap_or_default(),
            s.next().unwrap_or_default(),
            s.next().unwrap_or_default(),
            s.next().unwrap_or_default(),
            s.next().unwrap_or_default(),
        );
        let mut out = self.proto_history(node, history);
        out.push(Entry::Separator);
        // The node's body: what starts from the commit.
        let body = |worktree: &[Entry], branch: &[Entry]| -> Vec<Entry> {
            let mut out = Vec::new();
            if let Some(Entry::Item(i)) = branch.get(2) {
                out.push(relabel(i.clone(), "Create branch here…"));
            }
            if let Some(Entry::Item(i)) = worktree.get(1) {
                out.push(relabel(i.clone(), "Add worktree here…"));
            }
            out.extend(branch.get(3).cloned().filter(applies));
            out
        };
        let worktree_name = |c: crate::scene::Checkout| {
            let w = &scene.repo.worktrees[c.index];
            (!w.open).then(|| w.name())
        };
        let flat = |entries: &[Entry]| -> Vec<Item> {
            entries.iter().cloned().flat_map(flatten).collect()
        };
        let row = self.proto_row.clone();
        let section: Vec<Entry> = match row.as_ref().map(|r| (r, &r.kind)) {
            Some((
                Row { label, .. },
                RowKind::Ref {
                    kind: kind @ (RefKind::LocalBranch | RefKind::RemoteBranch),
                    head,
                    worktree,
                },
            )) => {
                let local = *kind == RefKind::LocalBranch;
                let current = local && *head;
                // Branch, Integrate and Remote, without Create branch and Reset (the body's).
                let mut items: Vec<Item> = flat(&branch)
                    .into_iter()
                    .chain(flat(&integrate))
                    .chain(flat(&remote))
                    .filter(|i| {
                        !i.label.starts_with("Create branch") && !i.label.starts_with("Reset ")
                    })
                    .collect();
                // The remote-tracking branches of a local one on this node, to delete.
                let remotes: Vec<String> = if local {
                    scene.graph.nodes[node]
                        .refs
                        .iter()
                        .map(|&r| &scene.repo.refs[r])
                        .filter(|r| {
                            r.kind == RefKind::RemoteBranch
                                && r.name.ends_with(&format!("/{label}"))
                        })
                        .map(|r| r.name.clone())
                        .collect()
                } else {
                    Vec::new()
                };
                let advanced_item = |i: &Item| {
                    i.label.starts_with("Set upstream")
                        || (i.label.starts_with("Delete ")
                            && (!local || remotes.iter().any(|r| mentions(&i.label, r))))
                };
                // #144: greyed only for what could apply but doesn't now; hidden otherwise.
                items.retain(|i| {
                    i.enabled || i.why.as_deref() == Some("Up to date") || busy(&i.why)
                });
                items.retain(|i| {
                    mentions(&i.label, label)
                        || (advanced_item(i) && remotes.iter().any(|r| mentions(&i.label, r)))
                });
                if current {
                    items.retain(|i| {
                        i.label.starts_with("Push")
                            || i.label.starts_with("Pull")
                            || advanced_item(i)
                    });
                }
                let (advanced, mut main): (Vec<Item>, Vec<Item>) =
                    items.into_iter().partition(|i| advanced_item(i));
                // A branch another worktree has: its worktree's items too.
                if let Some(name) = worktree.and_then(worktree_name) {
                    main.extend(
                        flat(&worktrees)
                            .into_iter()
                            .filter(|i| mentions(&i.label, &name)),
                    );
                }
                let mut out: Vec<Entry> = main.into_iter().map(Entry::Item).collect();
                if !advanced.is_empty() {
                    out.push(Entry::Separator);
                    out.push(
                        Submenu::new("Advanced", advanced.into_iter().map(Entry::Item).collect())
                            .into(),
                    );
                }
                out
            }
            Some((_, RowKind::Worktree(c)))
            | Some((
                _,
                RowKind::Ref {
                    worktree: Some(c), ..
                },
            )) if worktree_name(*c).is_some() => {
                let name = worktree_name(*c).unwrap_or_default();
                flat(&worktrees)
                    .into_iter()
                    .filter(|i| mentions(&i.label, &name))
                    .map(Entry::Item)
                    .collect()
            }
            _ => body(&worktrees, &branch),
        };
        out.extend(section);
        out
    }
}

fn busy(why: &Option<String>) -> bool {
    why.as_deref()
        .is_some_and(|w| w == loading_reason(true) || w == loading_reason(false))
}

/// What the menu shows in A: what can be done, and what only a running git operation holds up.
fn applies(e: &Entry) -> bool {
    match e {
        Entry::Item(i) => i.enabled || busy(&i.why),
        Entry::Submenu(s) => s.enabled || busy(&s.why),
        _ => true,
    }
}

/// A submenu of `items`, greyed when none of them can be done.
fn sub(label: &str, items: Vec<Entry>) -> Entry {
    let mut s = Submenu::new(label, items);
    if !s.entries.iter().any(|e| match e {
        Entry::Item(i) => i.enabled,
        Entry::Submenu(s) => s.enabled,
        _ => false,
    }) {
        s.enabled = false;
        s.why = Some("Nothing to do here".to_owned());
    }
    s.into()
}

fn relabel(mut item: Item, label: &str) -> Entry {
    item.label = label.to_owned();
    item.into()
}

/// `label` names `name` as a word of its own.
fn mentions(label: &str, name: &str) -> bool {
    label
        .split_whitespace()
        .any(|w| w.trim_end_matches('…') == name)
}

/// An item, or a submenu's items named in full: *Merge into main › x* is *Merge x into main…*.
fn flatten(e: Entry) -> Vec<Item> {
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
