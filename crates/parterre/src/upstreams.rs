//! Local branches against their upstreams, as the graph, the status bar and the log window
//! show them (see [`parterre_core::upstream`]).
//!
//! Hovering or selecting a node colours the commits between the branch the status bar names
//! for it and that branch's upstream, each on its own stretch of the edge that holds it: green
//! ahead, blue behind, red lost to a force push, grey dashed replaced by a rebase. A rebased
//! branch always has a dashed edge to its upstream, red while hovered if a force push would
//! lose commits.

use std::collections::HashSet;

use eframe::egui::{
    self, Color32, FontId, Painter, Pos2, Rect, RichText, Shape, Stroke, pos2, vec2,
};
use parterre_core::glyphs;
use parterre_core::upstream::{Side, Upstream};
use parterre_core::{Repo, revgraph::RevGraph};

use crate::render::{Marks, edge_path};
use crate::scene::Scene;
use crate::settings::{EdgeStyle, Settings};
use crate::theme::Palette;
use crate::view::View;

/// The colour of a commit's stretch of edge, and whether it is dashed.
fn side_stroke(palette: &Palette, side: Side) -> (Color32, bool) {
    match side {
        Side::Ahead => (palette.ahead, false),
        Side::Behind => (palette.behind, false),
        Side::Lost => (palette.lost, false),
        Side::Replaced => (palette.replaced, true),
    }
}

/// The node that shows the ref `r` (an index into [`Repo::refs`]), if it is shown.
fn node_of_ref(repo: &Repo, graph: &RevGraph, r: usize) -> Option<usize> {
    let n = graph.node_of(repo.refs[r].target)? as usize;
    graph.nodes[n].refs.contains(&r).then_some(n)
}

/// Paints the upstreams over the edges: the dashed edges of rebased branches, and the
/// commits between the hovered or selected branches and their upstreams.
pub fn paint(
    painter: &Painter,
    canvas: Rect,
    view: &View,
    scene: &Scene,
    palette: &Palette,
    settings: &Settings,
    marks: &Marks,
) {
    let (repo, graph) = (&*scene.repo, &scene.graph);
    let to_screen = |p: Pos2| view.to_screen(canvas, p);
    let zoom = view.fixed(view.zoom);
    let width = view.fixed((2.0 * view.zoom).max(1.0));
    let visible = canvas.expand(40.0);
    // The upstreams the hovered and selected nodes stand for, as the status bar names them.
    let selected = marks.selected.iter().enumerate().filter(|&(_, &s)| s);
    let active: HashSet<usize> = marks
        .hovered
        .into_iter()
        .chain(selected.map(|(n, _)| n))
        .filter_map(|n| upstream_on(scene, n))
        .collect();
    for (i, u) in repo.upstreams.iter().enumerate() {
        let branch = node_of_ref(repo, graph, u.branch);
        let upstream = u.target.and_then(|t| node_of_ref(repo, graph, t));
        let on = active.contains(&i);

        if let (Some(b), Some(t)) = (branch, upstream)
            && b != t
            && u.is_rebased()
        {
            // Neutral at rest; while hovered, red if a force push would lose commits.
            let color = match (on, u.loses_commits()) {
                (false, _) => palette.edge,
                (true, true) => palette.lost,
                (true, false) => palette.behind,
            };
            let node_box = |n: usize| view.rect_to_screen(canvas, scene.node_rect(n));
            let scale = to_screen(pos2(1.0, 0.0)).x - to_screen(Pos2::ZERO).x;
            let style = settings.edge_style;
            // PROTOTYPE: rebasing (#184). Routed around other boxes, as the rebasing edge is.
            let others: Vec<Rect> = (0..graph.nodes.len())
                .filter(|&n| n != b && n != t)
                .map(node_box)
                .filter(|r| visible.expand(160.0).intersects(*r))
                .collect();
            let path = route_around(node_box(b), node_box(t), &others, 4.0, scene, style, scale);
            let dash = 6.0 * zoom.max(0.5);
            painter.extend(Shape::dashed_line(
                &path,
                Stroke::new(width, color),
                dash,
                dash * 0.7,
            ));
        }

        if !on {
            continue;
        }
        for (e, sides) in u.edge_sides(graph, repo) {
            let Some(path) = edge_path(scene, e, settings.edge_style, to_screen, Some(visible))
            else {
                continue;
            };
            let k = sides.len() as f32;
            // Runs of one side are drawn as one stretch.
            let mut start = 0;
            while start < sides.len() {
                let end = start
                    + sides[start..]
                        .iter()
                        .take_while(|&&s| s == sides[start])
                        .count();
                if let Some(side) = sides[start] {
                    let part = sub_path(&path, start as f32 / k, end as f32 / k);
                    let (color, dashed) = side_stroke(palette, side);
                    let stroke = Stroke::new(width * 2.2, color);
                    if dashed {
                        let dash = 5.0 * zoom.max(0.5);
                        painter.extend(Shape::dashed_line(&part, stroke, dash, dash * 0.8));
                    } else {
                        painter.add(Shape::line(part, stroke));
                    }
                }
                start = end;
            }
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

/// The upstream a node stands for, as an index into [`Repo::upstreams`]: of a branch on it,
/// else of the branch whose upstream is on it, else of the branch it is the rebased-from
/// commit of. The status bar names it, and the graph colours it.
fn upstream_on(scene: &Scene, node: usize) -> Option<usize> {
    let n = scene.graph.nodes.get(node)?;
    let upstreams = &scene.repo.upstreams;
    let find = |f: &dyn Fn(&Upstream) -> bool| upstreams.iter().position(f);
    find(&|u| n.refs.contains(&u.branch))
        .or_else(|| find(&|u| u.target.is_some_and(|t| n.refs.contains(&t))))
        .or_else(|| find(&|u| u.rebased_from == Some(n.commit)))
}

/// The ahead|behind marker: `feature/x 3|2` for the hovered node's branch, else the selected
/// node's, else HEAD's. Ahead in green, behind in red, git's counts.
pub fn status_ui(
    ui: &mut egui::Ui,
    scene: &Scene,
    hovered: Option<usize>,
    selected: Option<usize>,
) {
    let repo = &scene.repo;
    let head = || repo.upstreams.iter().find(|u| repo.refs[u.branch].is_head);
    let Some(u) = hovered
        .and_then(|n| upstream_on(scene, n))
        .or_else(|| selected.and_then(|n| upstream_on(scene, n)))
        .map(|i| &repo.upstreams[i])
        .or_else(head)
    else {
        return;
    };
    let palette = Palette::new(ui.visuals().dark_mode, &[]);
    let weak = ui.visuals().weak_text_color();
    ui.label(RichText::new(&repo.refs[u.branch].name).color(weak))
        .on_hover_text(u.short_name());
    let spacing = ui.spacing().item_spacing.x;
    ui.spacing_mut().item_spacing.x = 0.0;
    if u.is_gone() {
        ui.label(RichText::new("gone").monospace().color(weak))
            .on_hover_text(u.short_name());
    } else {
        let count = |n: usize, color: Color32| {
            RichText::new(n.to_string())
                .monospace()
                .strong()
                .color(if n == 0 { weak } else { color })
        };
        ui.label(count(u.ahead(), palette.ahead))
            .on_hover_text("ahead");
        ui.label(RichText::new("|").monospace().color(weak));
        ui.label(count(u.behind(), palette.lost))
            .on_hover_text("behind");
    }
    ui.spacing_mut().item_spacing.x = spacing;
    ui.separator();
}

/// The commits *Compare → Upstream* compares, from the upstream to the branch, or why there
/// are none.
pub type ComparePair = Result<(parterre_core::Oid, parterre_core::Oid), &'static str>;

/// For *Compare → Upstream*: each branch on `node` with an upstream, as (the branch's
/// upstream, the commits to compare from the upstream to the branch, or why not).
pub fn compare_items(scene: &Scene, node: usize) -> Vec<(&Upstream, ComparePair)> {
    let repo = &scene.repo;
    let oid = |r: usize| repo.commit(repo.refs[r].target).oid;
    scene.graph.nodes[node]
        .refs
        .iter()
        .filter_map(|&r| repo.upstream_of(r))
        .map(|u| {
            let pair = match u.target {
                None => Err("gone"),
                Some(t) if repo.refs[t].target == repo.refs[u.branch].target => Err("Up to date"),
                Some(t) => Ok((oid(t), oid(u.branch))),
            };
            (u, pair)
        })
        .collect()
}

const ARROW: f32 = 10.0;
const GAP: f32 = 1.0;

/// Width of the counts [`paint_counts`] draws.
pub fn counts_width(painter: &Painter, (ahead, behind): (usize, usize)) -> f32 {
    let font = FontId::proportional(11.5);
    [ahead, behind]
        .into_iter()
        .filter(|&n| n > 0)
        .map(|n| {
            let g = painter.layout_no_wrap(n.to_string(), font.clone(), Color32::WHITE);
            5.0 + ARROW + GAP + g.size().x
        })
        .sum()
}

/// Draws `↑2 ↓1` left-aligned at `at`, with the app's glyph style.
pub fn paint_counts(painter: &Painter, at: Pos2, (ahead, behind): (usize, usize), color: Color32) {
    let font = FontId::proportional(11.5);
    let mut x = at.x;
    for (n, glyph) in [(ahead, glyphs::AHEAD), (behind, glyphs::BEHIND)] {
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

// PROTOTYPE: rebasing (#184). A worktree with a rebase in progress has a dashed orange edge
// from its HEAD (the commits replayed so far) to the branch being rebased, still at its old
// commit.
thread_local! {
    static REBASING: std::cell::RefCell<Option<(usize, Vec<(parterre_core::CommitIx, usize)>)>> =
        const { std::cell::RefCell::new(None) };
}

/// Every worktree's HEAD and the ref (an index into [`Repo::refs`]) it is rebasing, read once
/// per loaded repository.
fn rebasing(repo: &std::sync::Arc<Repo>) -> Vec<(parterre_core::CommitIx, usize)> {
    let key = std::sync::Arc::as_ptr(repo) as usize;
    REBASING.with_borrow_mut(|cache| {
        if cache.as_ref().is_none_or(|(k, _)| *k != key) {
            let found = repo
                .worktrees
                .iter()
                .filter_map(|w| {
                    let head = w.head?;
                    let dir = ["rebase-merge", "rebase-apply"]
                        .into_iter()
                        .find_map(|name| {
                            let out = std::process::Command::new("git")
                                .arg("-C")
                                .arg(&w.path)
                                .args(["rev-parse", "--git-path", name])
                                .output()
                                .ok()?;
                            let rel = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                            let path = w.path.join(rel);
                            path.is_dir().then_some(path)
                        })?;
                    let name = std::fs::read_to_string(dir.join("head-name")).ok()?;
                    let r = repo.refs.iter().position(|r| r.full_name == name.trim())?;
                    Some((head, r))
                })
                .collect();
            *cache = Some((key, found));
        }
        cache.as_ref().map(|(_, v)| v.clone()).unwrap_or_default()
    })
}

pub fn paint_rebasing(
    painter: &Painter,
    canvas: Rect,
    view: &View,
    scene: &Scene,
    settings: &Settings,
) {
    let (repo, graph) = (&*scene.repo, &scene.graph);
    let to_screen = |p: Pos2| view.to_screen(canvas, p);
    let zoom = view.fixed(view.zoom);
    let width = view.fixed((2.0 * view.zoom).max(1.0));
    let color = if painter.ctx().global_style().visuals.dark_mode {
        Color32::from_rgb(240, 160, 60)
    } else {
        Color32::from_rgb(220, 130, 30)
    };
    for (head, r) in rebasing(&scene.repo) {
        let (Some(h), Some(b)) = (
            graph.node_of(head).map(|n| n as usize),
            node_of_ref(repo, graph, r),
        ) else {
            continue;
        };
        if h == b {
            continue;
        }
        let node_box = |n: usize| view.rect_to_screen(canvas, scene.node_rect(n));
        let scale = to_screen(pos2(1.0, 0.0)).x - to_screen(Pos2::ZERO).x;
        // Every other box in view, to route around.
        let others: Vec<Rect> = (0..graph.nodes.len())
            .filter(|&n| n != h && n != b)
            .map(node_box)
            .filter(|r| canvas.expand(200.0).intersects(*r))
            .collect();
        let amp = 3.0 * zoom.max(0.5);
        let path = route_around(
            node_box(h),
            node_box(b),
            &others,
            amp + 3.0,
            scene,
            settings.edge_style,
            scale,
        );
        stuck_line(painter, &path, color, width, zoom.max(0.5));
    }
}

/// PROTOTYPE: rebasing (#184). A route from the centre of box `a` to the centre of box `b`,
/// cut where it leaves `a` and enters `b` (as TortoiseGit draws them), so it emerges wherever
/// it crosses the box's edge. Of curves bowed more or less to either side of the straight
/// line, it takes the one crossing the fewest of the `others` (each kept `margin` clear), then
/// the shortest.
fn route_around(
    a: Rect,
    b: Rect,
    others: &[Rect],
    margin: f32,
    _scene: &Scene,
    style: EdgeStyle,
    scale: f32,
) -> Vec<Pos2> {
    let (p0, p3) = (a.center(), b.center());
    let chord = p3 - p0;
    let normal = if chord.length() > 0.0 {
        chord.normalized().rot90()
    } else {
        vec2(0.0, 1.0)
    };
    let curve = |c1: Pos2, c2: Pos2| -> Vec<Pos2> {
        if style == EdgeStyle::Straight {
            return vec![p0, c1, c2, p3];
        }
        (0..=48)
            .map(|i| {
                let t = i as f32 / 48.0;
                let u = 1.0 - t;
                (p0.to_vec2() * (u * u * u)
                    + c1.to_vec2() * (3.0 * u * u * t)
                    + c2.to_vec2() * (3.0 * u * t * t)
                    + p3.to_vec2() * (t * t * t))
                    .to_pos2()
            })
            .collect()
    };
    let crossings = |path: &[Pos2]| {
        let dense: Vec<Pos2> = along(path, 4.0).into_iter().map(|(p, _)| p).collect();
        others
            .iter()
            .filter(|r| {
                let r = r.expand(margin);
                dense.iter().any(|p| r.contains(*p))
            })
            .count()
    };
    let length = |path: &[Pos2]| -> f32 { path.windows(2).map(|w| w[0].distance(w[1])).sum() };
    let mut best: Option<(usize, f32, Vec<Pos2>)> = None;
    for bow in [
        0.0f32, -1.0, 1.0, -2.0, 2.0, -3.0, 3.0, -4.0, 4.0, -6.0, 6.0,
    ] {
        let lift = normal * (bow * 30.0 * scale);
        let c1 = p0 + chord / 3.0 + lift;
        let c2 = p0 + chord * (2.0 / 3.0) + lift;
        let path = clip(&clip(&curve(c1, c2), a), b);
        if path.len() < 2 {
            continue;
        }
        let score = (crossings(&path), length(&path));
        if best
            .as_ref()
            .is_none_or(|(n, l, _)| score.0 < *n || score.0 == *n && score.1 < *l)
        {
            best = Some((score.0, score.1, path));
        }
    }
    best.map(|(_, _, path)| path).unwrap_or_default()
}

/// The part of a polyline outside `r`, at the end that starts or ends inside it: cut at the
/// point where it crosses `r`'s edge.
fn clip(path: &[Pos2], r: Rect) -> Vec<Pos2> {
    let mut path = path.to_vec();
    let starts_inside = path.first().is_some_and(|p| r.contains(*p));
    if !starts_inside {
        path.reverse();
    }
    if !path.first().is_some_and(|p| r.contains(*p)) {
        if !starts_inside {
            path.reverse();
        }
        return path;
    }
    let Some(k) = path.iter().position(|p| !r.contains(*p)) else {
        return Vec::new();
    };
    // The edge lies between the inside point k-1 and the outside point k.
    let (mut lo, mut hi) = (path[k - 1], path[k]);
    for _ in 0..20 {
        let mid = lo.lerp(hi, 0.5);
        if r.contains(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let mut out = vec![hi];
    out.extend_from_slice(&path[k..]);
    if !starts_inside {
        out.reverse();
    }
    out
}

/// PROTOTYPE: rebasing (#184). The looks to choose between for a rebasing worktree's edge.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StuckLine {
    Dotted,
    DashDot,
    Crosses,
    Zigzag,
}

impl StuckLine {
    pub const ALL: [StuckLine; 4] = [Self::Dotted, Self::DashDot, Self::Crosses, Self::Zigzag];
    pub fn name(self) -> &'static str {
        match self {
            Self::Dotted => "Dotted",
            Self::DashDot => "Dash-dot",
            Self::Crosses => "Crosses",
            Self::Zigzag => "Zigzag",
        }
    }
}

thread_local! {
    pub static STUCK_LINE: std::cell::Cell<StuckLine> = const { std::cell::Cell::new(StuckLine::Dotted) };
}

/// Points every `step` along a polyline, with the direction there.
fn along(path: &[Pos2], step: f32) -> Vec<(Pos2, egui::Vec2)> {
    let mut out = Vec::new();
    let mut next = 0.0;
    let mut at = 0.0;
    for w in path.windows(2) {
        let d = w[0].distance(w[1]);
        if d <= 0.0 {
            continue;
        }
        let dir = (w[1] - w[0]) / d;
        while next <= at + d {
            out.push((w[0] + dir * (next - at), dir));
            next += step;
        }
        at += d;
    }
    out
}

fn stuck_line(painter: &Painter, path: &[Pos2], color: Color32, width: f32, zoom: f32) {
    let stroke = Stroke::new(width, color);
    match STUCK_LINE.get() {
        StuckLine::Dotted => {
            for (p, _) in along(path, 6.0 * zoom) {
                painter.circle_filled(p, width * 1.1, color);
            }
        }
        StuckLine::DashDot => painter.extend(Shape::dashed_line_with_offset(
            path,
            stroke,
            &[10.0 * zoom, 1.5 * zoom],
            &[3.5 * zoom, 3.5 * zoom],
            0.0,
        )),
        StuckLine::Crosses => {
            let arm = 3.0 * zoom;
            for (p, dir) in along(path, 9.0 * zoom) {
                let (a, b) = (dir + dir.rot90(), dir - dir.rot90());
                let (a, b) = (a.normalized() * arm, b.normalized() * arm);
                painter.line_segment([p - a, p + a], stroke);
                painter.line_segment([p - b, p + b], stroke);
            }
        }
        StuckLine::Zigzag => {
            let amp = 3.0 * zoom;
            let points: Vec<Pos2> = along(path, 4.0 * zoom)
                .into_iter()
                .enumerate()
                .map(|(i, (p, dir))| {
                    let side = match i % 2 {
                        0 => 1.0,
                        _ => -1.0,
                    };
                    p + dir.rot90() * (amp * side)
                })
                .collect();
            painter.add(Shape::line(points, stroke));
        }
    }
}

/// PROTOTYPE: rebasing (#184). A switcher for the rebasing worktree's edge, bottom left, while
/// a worktree has a rebase in progress.
pub fn stuck_line_bar(ctx: &egui::Context, repo: Option<&std::sync::Arc<Repo>>) {
    // For screenshots: PARTERRE_PROTOTYPE_LINE=dotted|dash-dot|crosses|zigzag.
    if let Ok(name) = std::env::var("PARTERRE_PROTOTYPE_LINE")
        && let Some(line) = StuckLine::ALL
            .into_iter()
            .find(|l| l.name().eq_ignore_ascii_case(&name))
        && ctx.cumulative_frame_nr() < 3
    {
        STUCK_LINE.set(line);
    }
    let Some(repo) = repo else { return };
    if rebasing(repo).is_empty() {
        return;
    }
    egui::Area::new(egui::Id::new("prototype-stuck-line"))
        .anchor(egui::Align2::LEFT_BOTTOM, vec2(12.0, -40.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.weak("PROTOTYPE: rebasing edge");
                    let mut current = STUCK_LINE.get();
                    for line in StuckLine::ALL {
                        ui.selectable_value(&mut current, line, line.name());
                    }
                    if current != STUCK_LINE.get() {
                        STUCK_LINE.set(current);
                        ui.ctx().request_repaint();
                    }
                });
            });
        });
}
