//! PROTOTYPE (throwaway, branch `prototype/upstreams`): how the graph shows which remote
//! branch a local branch tracks.
//!
//! Round 6: a rebased branch (diverged, with upstream commits it had before it was
//! rewritten) always has a dashed edge to its upstream, routed like the graph's own, in the
//! edge colour; hovered, it turns red if a force push would lose commits. The commit it was
//! rebased from is an ordinary node while the upstream has moved on past it. Hovering or
//! selecting a branch or its upstream colours the commits between them: green ahead, blue
//! behind, red lost to a force push (never on the branch, no copy on it), grey dashed
//! replaced by the rebase. The status bar shows `branch 3|2` for the hovered, selected or
//! HEAD's branch. *Compare → Upstream* compares the two. The log window's branch badges get
//! ahead/behind counts. `PARTERRE_UPSTREAMS_HOVER=<ref>` fakes a hover for screenshots.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};
use parterre_core::glyphs::{Glyph, Part};
use parterre_core::{CommitIx, Oid, Repo};

use crate::render::{edge_path, link_path};
use crate::scene::Scene;
use crate::settings::Settings;
use crate::theme::Palette;
use crate::view::View;

/// A local branch and the upstream git has for it.
#[derive(Clone, Debug)]
struct Link {
    /// Index into `Repo::refs` of the local branch.
    local: usize,
    /// Index into `Repo::refs` of the upstream, if it exists (and is loaded).
    upstream: Option<usize>,
    /// `origin/feature/x`.
    upstream_short: String,
    ahead: u32,
    behind: u32,
    gone: bool,
}

/// The commits between a branch and its upstream, as full hashes. Split as git's
/// `--force-if-includes` sees them: whether the branch ever had them (its reflog).
#[derive(Clone, Debug, Default)]
struct Range {
    /// On the branch only (`upstream..branch`).
    ahead: HashSet<String>,
    /// On the upstream only, and never on the branch: a force push would drop them, so git's
    /// guard refuses it.
    behind: HashSet<String>,
    /// On the upstream only, but on the branch before it was rewritten (rebased).
    replaced: HashSet<String>,
    /// The newest of `replaced`: the commit the branch was rebased from.
    rebased_from: Option<String>,
}

type Cached = Option<(usize, Arc<Vec<Link>>)>;

thread_local! {
    static LINKS: RefCell<Cached> = const { RefCell::new(None) };
    static RANGES: RefCell<HashMap<(usize, usize), Arc<Range>>> = RefCell::new(HashMap::new());
}

/// Ahead/behind counts by full ref name, for the log window (which has no scene).
static COUNTS: Mutex<Option<HashMap<String, (u32, u32)>>> = Mutex::new(None);

fn git(repo: &Repo, args: &[&str]) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(&repo.path)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn links(repo: &Arc<Repo>) -> Arc<Vec<Link>> {
    let key = Arc::as_ptr(repo) as usize;
    LINKS.with(|c| {
        if let Some((k, l)) = &*c.borrow()
            && *k == key
        {
            return Arc::clone(l);
        }
        let l = Arc::new(load(repo));
        *COUNTS.lock().unwrap() = Some(
            l.iter()
                .map(|l| (repo.refs[l.local].full_name.clone(), (l.ahead, l.behind)))
                .collect(),
        );
        RANGES.with(|r| r.borrow_mut().clear());
        *c.borrow_mut() = Some((key, Arc::clone(&l)));
        l
    })
}

fn load(repo: &Repo) -> Vec<Link> {
    let out = git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(upstream)%00%(upstream:short)%00%(upstream:track)",
            "refs/heads",
        ],
    );
    let by_name: HashMap<&str, usize> = repo
        .refs
        .iter()
        .enumerate()
        .map(|(i, r)| (r.full_name.as_str(), i))
        .collect();
    let count = |track: &str, word: &str| -> u32 {
        track
            .split(word)
            .nth(1)
            .and_then(|s| {
                s.trim_start()
                    .split(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse()
                    .ok()
            })
            .unwrap_or(0)
    };
    out.lines()
        .filter_map(|line| {
            let mut f = line.split('\0');
            let (name, up, short, track) = (f.next()?, f.next()?, f.next()?, f.next()?);
            if up.is_empty() {
                return None;
            }
            Some(Link {
                local: *by_name.get(name)?,
                upstream: by_name.get(up).copied(),
                upstream_short: short.to_owned(),
                ahead: count(track, "ahead"),
                behind: count(track, "behind"),
                gone: track.contains("gone"),
            })
        })
        .collect()
}

fn range(repo: &Arc<Repo>, l: &Link) -> Arc<Range> {
    let key = (Arc::as_ptr(repo) as usize, l.local);
    RANGES.with(|c| {
        if let Some(r) = c.borrow().get(&key) {
            return Arc::clone(r);
        }
        let mut r = Range::default();
        if let Some(u) = l.upstream {
            let upstream = repo.refs[u].full_name.as_str();
            let local = repo.refs[l.local].full_name.as_str();
            let set = |args: &[&str]| -> Vec<String> {
                git(repo, args).lines().map(str::to_owned).collect()
            };
            r.ahead = set(&["rev-list", &format!("{upstream}..{local}")])
                .into_iter()
                .collect();
            let behind = set(&["rev-list", &format!("{local}..{upstream}")]);
            // Everything the branch has pointed at, as its reflog remembers.
            let mut args = vec!["rev-list".to_owned(), format!("{local}..{upstream}")];
            let reflog = set(&["reflog", "show", "--format=%H", local]);
            args.extend(reflog.iter().map(|h| format!("^{h}")));
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let never: HashSet<String> = set(&args).into_iter().collect();
            // Upstream commits with a copy on the branch (cherry-picked): nothing is lost,
            // even though git's --force-if-includes still refuses.
            let spec = format!("{local}...{upstream}");
            let copied: HashSet<String> =
                set(&["rev-list", "--right-only", "--cherry-mark", &spec])
                    .into_iter()
                    .filter_map(|l| l.strip_prefix('=').map(str::to_owned))
                    .collect();
            for oid in behind {
                if never.contains(&oid) && !copied.contains(&oid) {
                    r.behind.insert(oid);
                } else {
                    r.rebased_from.get_or_insert_with(|| oid.clone());
                    r.replaced.insert(oid);
                }
            }
        }
        let r = Arc::new(r);
        c.borrow_mut().insert(key, Arc::clone(&r));
        r
    })
}

fn node_of_ref(scene: &Scene, r: usize) -> Option<usize> {
    scene.graph.nodes.iter().position(|n| n.refs.contains(&r))
}

struct Colors {
    ahead: Color32,
    behind: Color32,
    lost: Color32,
    replaced: Color32,
    line: Color32,
}

fn colors(dark: bool) -> Colors {
    if dark {
        Colors {
            ahead: Color32::from_rgb(90, 200, 110),
            behind: Color32::from_rgb(100, 170, 255),
            lost: Color32::from_rgb(255, 95, 85),
            replaced: Color32::from_gray(140),
            line: Color32::from_rgb(100, 170, 255),
        }
    } else {
        Colors {
            ahead: Color32::from_rgb(20, 150, 50),
            behind: Color32::from_rgb(20, 110, 230),
            lost: Color32::from_rgb(215, 40, 30),
            replaced: Color32::from_gray(150),
            line: Color32::from_rgb(30, 130, 230),
        }
    }
}

/// The part of a polyline from `t0` to `t1` (fractions of its length).
fn sub_path(path: &[Pos2], t0: f32, t1: f32) -> Vec<Pos2> {
    let total: f32 = path.windows(2).map(|w| w[0].distance(w[1])).sum();
    let (a, b) = (total * t0, total * t1);
    let mut out = Vec::new();
    let mut at = 0.0;
    for w in path.windows(2) {
        let d = w[0].distance(w[1]);
        let (s, e) = (at, at + d);
        if e >= a && s <= b && d > 0.0 {
            let p0 = w[0].lerp(w[1], ((a - s) / d).clamp(0.0, 1.0));
            let p1 = w[0].lerp(w[1], ((b - s) / d).clamp(0.0, 1.0));
            if out.is_empty() {
                out.push(p0);
            }
            out.push(p1);
        }
        at = e;
    }
    out
}

/// The commit a rebased branch was rebased from, when the upstream has moved on past it (so
/// no ref shows it): the graph makes it a node while that lasts.
fn rebased_from(repo: &Arc<Repo>, l: &Link) -> Option<CommitIx> {
    let r = range(repo, l);
    let c = repo.lookup(&Oid::from_hex(r.rebased_from.as_deref()?)?)?;
    (Some(c) != l.upstream.map(|u| repo.refs[u].target)).then_some(c)
}

/// The extra nodes the graph needs: see [`rebased_from`].
pub fn extra_nodes(repo: &Arc<Repo>) -> Vec<CommitIx> {
    links(repo)
        .iter()
        .filter_map(|l| rebased_from(repo, l))
        .collect()
}

/// Paints the upstreams over the graph: always for rebased branches whose upstream moved on,
/// otherwise for the hovered or selected nodes.
#[allow(clippy::too_many_arguments)]
pub fn paint(
    painter: &Painter,
    canvas: Rect,
    view: &View,
    scene: &Scene,
    palette: &Palette,
    settings: &Settings,
    hovered: Option<usize>,
    selected: &[bool],
) {
    let links = links(&scene.repo);
    let to_screen = |p: Pos2| view.to_screen(canvas, p);
    // Screen pixels per world unit, as `edge_path` works it out.
    let scale = to_screen(pos2(1.0, 0.0)).x - to_screen(Pos2::ZERO).x;
    let zoom = view.fixed(view.zoom);
    let width = (2.0 * zoom).max(1.0);
    let c = colors(palette.dark);
    let active = |n: usize| hovered == Some(n) || selected.get(n).copied().unwrap_or(false);
    let visible = canvas.expand(40.0);
    let node_box = |n: usize| view.rect_to_screen(canvas, scene.node_rect(n));
    let time = |n: usize| scene.repo.commit(scene.graph.nodes[n].commit).commit_time;

    for l in links.iter() {
        let Some(ln) = node_of_ref(scene, l.local) else {
            continue;
        };
        let Some(un) = l.upstream.and_then(|u| node_of_ref(scene, u)) else {
            continue;
        };
        if un == ln {
            continue;
        }
        let from = rebased_from(&scene.repo, l)
            .and_then(|c| scene.graph.node_of(c))
            .map(|n| n as usize);
        let on = active(ln) || active(un) || from.is_some_and(active);
        let r = range(&scene.repo, l);
        // Rebased: diverged, and the upstream has commits the branch had before it was
        // rewritten. Shown always; anything else only while hovered or selected.
        let rebased = !r.ahead.is_empty() && !r.replaced.is_empty();
        if !on && !rebased {
            continue;
        }
        // Rebased branches only get the dashed edge; otherwise the coloured edges show it.
        // Neutral at rest; while hovered, red if a force push would drop commits.
        let lost = !r.ahead.is_empty() && !r.behind.is_empty();
        let line = match (on, lost) {
            (false, _) => palette.edge,
            (true, true) => c.lost,
            (true, false) => c.line,
        };

        // Colour the commits between them, each on its own stretch of the edge that holds it.
        if on {
            let color_of = |oid: &str| {
                if r.ahead.contains(oid) {
                    Some((c.ahead, false))
                } else if r.behind.contains(oid) {
                    Some((if r.ahead.is_empty() { c.behind } else { c.lost }, false))
                } else if r.replaced.contains(oid) {
                    Some((c.replaced, true))
                } else {
                    None
                }
            };
            for (e, &edge) in scene.graph.edges.iter().enumerate() {
                let child = scene.graph.nodes[edge.child as usize].commit;
                let mut on_edge = vec![child];
                on_edge.extend(scene.graph.collapsed_commits(&scene.repo, edge, usize::MAX));
                let colours: Vec<_> = on_edge
                    .iter()
                    .map(|&k| color_of(&scene.repo.commit(k).oid.to_hex()))
                    .collect();
                if colours.iter().all(Option::is_none) {
                    continue;
                }
                let Some(path) = edge_path(scene, e, settings.edge_style, to_screen, Some(visible))
                else {
                    continue;
                };
                let k = colours.len() as f32;
                for (j, colour) in colours.iter().enumerate() {
                    let Some((color, dashed)) = *colour else {
                        continue;
                    };
                    let part = sub_path(&path, j as f32 / k, (j + 1) as f32 / k);
                    let stroke = Stroke::new(width * 2.2, color);
                    if dashed {
                        let dash = 5.0 * zoom.max(0.5);
                        painter.extend(Shape::dashed_line(&part, stroke, dash, dash * 0.8));
                    } else {
                        painter.add(Shape::line(part, stroke));
                    }
                }
            }
        }

        // The dashed edge to the upstream, routed like an edge from the newer box to the
        // older one.
        if !rebased {
            continue;
        }
        let (child, parent) = if time(ln) >= time(un) {
            (node_box(ln), node_box(un))
        } else {
            (node_box(un), node_box(ln))
        };
        let path = dashed_route(scene, child, parent, settings, scale);
        let dash = 6.0 * zoom.max(0.5);
        painter.extend(Shape::dashed_line(
            &path,
            Stroke::new(width, line),
            dash,
            dash * 0.7,
        ));
    }
}

/// The dashed edge's route: as an edge would go when one box is further along the history
/// than the other; side by side, from facing side to facing side, the same way turned across.
fn dashed_route(
    scene: &Scene,
    child: Rect,
    parent: Rect,
    settings: &Settings,
    scale: f32,
) -> Vec<Pos2> {
    let f = scene.layout.direction.flow();
    let flow = vec2(f.x, f.y);
    let across = vec2(flow.y.abs(), flow.x.abs());
    let depth = |r: Rect| (flow.x.abs() * r.width() + flow.y.abs() * r.height()) / 2.0;
    let breadth = |r: Rect| (across.x * r.width() + across.y * r.height()) / 2.0;
    let along = |p: Pos2| p.to_vec2().dot(flow);
    let side = |p: Pos2| p.to_vec2().dot(across);
    let level = along(parent.center()) - depth(parent) < along(child.center()) + depth(child);
    let apart =
        (side(parent.center()) - side(child.center())).abs() > breadth(parent) + breadth(child);
    if !(level && apart) {
        return link_path(scene, child, parent, settings.edge_style, scale);
    }
    let sign = (side(parent.center()) - side(child.center())).signum();
    let p0 = child.center() + across * (sign * breadth(child));
    let p3 = parent.center() - across * (sign * breadth(parent));
    let reach = ((side(p3) - side(p0)).abs() / 2.0).max(20.0 * scale);
    let (c1, c2) = (p0 + across * (sign * reach), p3 - across * (sign * reach));
    (0..=32)
        .map(|i| {
            let t = i as f32 / 32.0;
            let u = 1.0 - t;
            (p0.to_vec2() * (u * u * u)
                + c1.to_vec2() * (3.0 * u * u * t)
                + c2.to_vec2() * (3.0 * u * t * t)
                + p3.to_vec2() * (t * t * t))
                .to_pos2()
        })
        .collect()
}

/// The ahead|behind marker in the status bar, for the branch on the hovered node (or the
/// selected one, or HEAD's): `feature/x 3|2`, ahead in green, behind in red.
pub fn status_ui(ui: &mut egui::Ui, scene: &Scene, node: Option<usize>) {
    let links = links(&scene.repo);
    let on = |n: usize| {
        let refs = &scene.graph.nodes[n].refs;
        links.iter().find(|l| refs.contains(&l.local)).or_else(|| {
            links
                .iter()
                .find(|l| l.upstream.is_some_and(|u| refs.contains(&u)))
        })
    };
    let head = || links.iter().find(|l| scene.repo.refs[l.local].is_head);
    let Some(l) = node.and_then(on).or_else(head) else {
        return;
    };
    let c = colors(ui.visuals().dark_mode);
    let weak = ui.visuals().weak_text_color();
    ui.label(egui::RichText::new(&scene.repo.refs[l.local].name).color(weak))
        .on_hover_text(format!("Upstream: {}", l.upstream_short));
    let spacing = ui.spacing().item_spacing.x;
    ui.spacing_mut().item_spacing.x = 0.0;
    if l.gone {
        ui.label(egui::RichText::new("gone").monospace().color(weak));
    } else {
        let n = |v: u32, color: Color32| {
            egui::RichText::new(v.to_string())
                .monospace()
                .strong()
                .color(if v == 0 { weak } else { color })
        };
        ui.label(n(l.ahead, c.ahead)).on_hover_text("ahead");
        ui.label(egui::RichText::new("|").monospace().color(weak));
        ui.label(n(l.behind, c.lost)).on_hover_text("behind");
    }
    ui.spacing_mut().item_spacing.x = spacing;
    ui.separator();
}

/// For *Compare → Upstream*: the upstream of the (first) branch on `node` that has one on
/// another commit, as (its name, its commit, the branch's commit).
pub fn upstream_of(scene: &Scene, node: usize) -> Option<(String, Oid, Oid)> {
    let n = &scene.graph.nodes[node];
    links(&scene.repo).iter().find_map(|l| {
        if !n.refs.contains(&l.local) {
            return None;
        }
        let up = scene.repo.refs[l.upstream?].target;
        let local = scene.repo.refs[l.local].target;
        let oid = |c| scene.repo.commit(c).oid;
        (up != local).then(|| (l.upstream_short.clone(), oid(up), oid(local)))
    })
}

/// For screenshots: the node of `PARTERRE_UPSTREAMS_HOVER` (a ref's short name), as if hovered.
pub fn fake_hover(scene: &Scene) -> Option<usize> {
    let name = std::env::var("PARTERRE_UPSTREAMS_HOVER").ok()?;
    let r = scene.repo.refs.iter().position(|r| r.name == name)?;
    node_of_ref(scene, r)
}

/// The log window's indicator: ahead and behind counts of a local branch, if any.
pub fn counts(full_name: &str) -> Option<(u32, u32)> {
    COUNTS
        .lock()
        .unwrap()
        .as_ref()?
        .get(full_name)
        .copied()
        .filter(|&(a, b)| a > 0 || b > 0)
}

const ARROW_UP: Glyph = &[Part::Path("M12 20v-16M5 11l7-7 7 7")];
const ARROW_DOWN: Glyph = &[Part::Path("M12 4v16M5 13l7 7 7-7")];
const ARROW: f32 = 10.0;
const GAP: f32 = 1.0;

/// Width of the indicator [`paint_counts`] draws.
pub fn counts_width(painter: &Painter, (ahead, behind): (u32, u32)) -> f32 {
    let font = FontId::proportional(11.5);
    let mut w = 0.0;
    for n in [ahead, behind].into_iter().filter(|&n| n > 0) {
        let g = painter.layout_no_wrap(n.to_string(), font.clone(), Color32::WHITE);
        w += 5.0 + ARROW + GAP + g.size().x;
    }
    w
}

/// Draws `↑2 ↓1` left-aligned at `at`, with the app's glyph style.
pub fn paint_counts(painter: &Painter, at: Pos2, (ahead, behind): (u32, u32), color: Color32) {
    let font = FontId::proportional(11.5);
    let mut x = at.x;
    for (n, glyph) in [(ahead, ARROW_UP), (behind, ARROW_DOWN)] {
        if n == 0 {
            continue;
        }
        x += 5.0;
        let icon = Rect::from_min_size(pos2(x, at.y - ARROW / 2.0), vec2(ARROW, ARROW));
        crate::widgets::paint_glyph(painter, icon, glyph, color);
        x += ARROW + GAP;
        let g = painter.layout_no_wrap(n.to_string(), font.clone(), color);
        let w = g.size().x;
        painter.galley(pos2(x, at.y - g.size().y / 2.0), g, color);
        x += w;
    }
}
