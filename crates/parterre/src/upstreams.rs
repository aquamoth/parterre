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
            let path = dashed_route(node_box(b), node_box(t), scene, style, scale);
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

/// The dashed edge's route, from side to side across the flow (left and right when the
/// newest commits are on top): the sides that face each other when the boxes are apart, else
/// the same side of both, towards the upstream. Curved, or with straight segments, as the
/// edges are. `scale` is screen pixels per world unit.
fn dashed_route(a: Rect, b: Rect, scene: &Scene, style: EdgeStyle, scale: f32) -> Vec<Pos2> {
    let f = scene.layout.direction.flow();
    let across = vec2(f.y.abs(), f.x.abs());
    let breadth = |r: Rect| (across.x * r.width() + across.y * r.height()) / 2.0;
    let side = |p: Pos2| p.to_vec2().dot(across);
    let gap = side(b.center()) - side(a.center());
    let sign = if gap < 0.0 { -1.0 } else { 1.0 };
    let apart = gap.abs() > breadth(a) + breadth(b);
    let p0 = a.center() + across * (sign * breadth(a));
    let (p3, c1, c2) = if apart {
        let p3 = b.center() - across * (sign * breadth(b));
        // Out sideways only briefly, so it heads for the other box rather than along the
        // row of boxes it starts in.
        let reach = ((side(p3) - side(p0)).abs() / 2.0).clamp(15.0 * scale, 40.0 * scale);
        (
            p3,
            p0 + across * (sign * reach),
            p3 - across * (sign * reach),
        )
    } else {
        // Out of the same side of both, round the wider one.
        let p3 = b.center() + across * (sign * breadth(b));
        let far = (sign * side(p0)).max(sign * side(p3)) + 30.0 * scale;
        let beyond = |p: Pos2| p + across * (sign * far - side(p));
        (p3, beyond(p0), beyond(p3))
    };
    if style == EdgeStyle::Straight {
        return vec![p0, c1, c2, p3];
    }
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
