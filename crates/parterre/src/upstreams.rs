use std::collections::HashSet;

use eframe::egui::{
    self, Color32, FontId, Painter, Pos2, Rect, RichText, Shape, Stroke, pos2, vec2,
};
use parterre_core::glyphs;
use parterre_core::upstream::{Side, Upstream};
use parterre_core::{CommitIx, Repo, revgraph::RevGraph};

use crate::render::{ARROW_LEN, Marks, arrowhead_points, edge_path};
use crate::scene::Scene;
use crate::settings::{Arrows, EdgeStyle, Settings};
use crate::theme::Palette;
use crate::view::View;

fn side_color(palette: &Palette, side: Side) -> Color32 {
    match side {
        Side::Ahead => palette.ahead,
        Side::Behind => palette.behind,
        Side::Lost => palette.lost,
        Side::Replaced => palette.replaced,
    }
}

/// The node showing the ref `ref_ix`, if it is shown.
fn node_of_ref(repo: &Repo, graph: &RevGraph, ref_ix: usize) -> Option<usize> {
    let n = graph.node_of(repo.refs[ref_ix].target)? as usize;
    graph.nodes[n].refs.contains(&ref_ix).then_some(n)
}

fn line_width(view: &View) -> f32 {
    view.fixed((2.0 * view.zoom).max(1.0))
}

/// Paints the dashed edges of rebased branches to their upstreams, and the commits between the
/// hovered or selected branches and their upstreams.
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
    let width = line_width(view);
    let visible = canvas.expand(40.0);
    let selected = marks.selected.iter().enumerate().filter(|&(_, &s)| s);
    let highlighted: HashSet<usize> = marks
        .hovered
        .into_iter()
        .chain(selected.map(|(n, _)| n))
        .filter_map(|n| upstream_on(scene, n))
        .collect();
    for (i, u) in repo.upstreams.iter().enumerate() {
        let branch = node_of_ref(repo, graph, u.branch);
        let upstream = u.target.and_then(|t| node_of_ref(repo, graph, t));
        let is_highlighted = highlighted.contains(&i);

        if let (Some(b), Some(t)) = (branch, upstream)
            && b != t
            && u.is_rebased()
        {
            let color = match (is_highlighted, u.loses_commits()) {
                (false, _) => palette.edge,
                (true, true) => palette.lost,
                (true, false) => palette.behind,
            };
            let path = special_route(scene, view, canvas, b, t, 4.0, settings.edge_style);
            let dash = 6.0 * zoom.max(0.5);
            painter.extend(Shape::dashed_line(
                &path,
                Stroke::new(width, color),
                dash,
                dash * 0.7,
            ));
        }

        if !is_highlighted {
            continue;
        }
        for (e, sides) in u.edge_sides(graph) {
            let Some(path) = edge_path(scene, e, settings.edge_style, to_screen, Some(visible))
            else {
                continue;
            };
            let count = sides.len() as f32;
            let mut start = 0;
            for run in sides.chunk_by(|a, b| a == b) {
                let end = start + run.len();
                if let Some(side) = run[0] {
                    let part = sub_path(&path, start as f32 / count, end as f32 / count);
                    let stroke = Stroke::new(width * 2.2, side_color(palette, side));
                    if side == Side::Replaced {
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

fn polyline_length(path: &[Pos2]) -> f32 {
    path.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// The part of a polyline between the fractions `from` and `to` of its length.
fn sub_path(path: &[Pos2], from: f32, to: f32) -> Vec<Pos2> {
    let total = polyline_length(path);
    let (start, end) = (total * from, total * to);
    let mut out = Vec::new();
    let mut at = 0.0;
    for w in path.windows(2) {
        let len = w[0].distance(w[1]);
        if at + len >= start && at <= end && len > 0.0 {
            let p0 = w[0].lerp(w[1], ((start - at) / len).clamp(0.0, 1.0));
            let p1 = w[0].lerp(w[1], ((end - at) / len).clamp(0.0, 1.0));
            if out.is_empty() {
                out.push(p0);
            }
            out.push(p1);
        }
        at += len;
    }
    out
}

/// The screen route of an edge parterre adds between the nodes `a` and `b`, around the other
/// boxes in view (see [`route_around`]).
pub(crate) fn special_route(
    scene: &Scene,
    view: &View,
    canvas: Rect,
    a: usize,
    b: usize,
    margin: f32,
    style: EdgeStyle,
) -> Vec<Pos2> {
    let node_box = |n: usize| view.rect_to_screen(canvas, scene.node_rect(n));
    let near = canvas.expand(160.0);
    let others: Vec<Rect> = (0..scene.graph.nodes.len())
        .filter(|&n| n != a && n != b)
        .map(node_box)
        .filter(|r| near.intersects(*r))
        .collect();
    route_around(
        node_box(a),
        node_box(b),
        &others,
        margin,
        style,
        view.points_per_world_unit(),
    )
}

/// From the centre of box `a` to the centre of box `b`, cut where it leaves `a` and enters `b`,
/// as TortoiseGit draws such edges. Of curves bowed more or less to either side of the straight
/// line, it takes the one crossing the fewest of the `others` (each kept `margin` clear), then
/// the shortest.
fn route_around(
    a: Rect,
    b: Rect,
    others: &[Rect],
    margin: f32,
    style: EdgeStyle,
    points_per_world_unit: f32,
) -> Vec<Pos2> {
    const BOWS: [f32; 11] = [0.0, -1.0, 1.0, -2.0, 2.0, -3.0, 3.0, -4.0, 4.0, -6.0, 6.0];
    const BOW_STEP: f32 = 30.0;
    const CURVE_SEGMENTS: u16 = 48;
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
        (0..=CURVE_SEGMENTS)
            .map(|i| {
                let t = f32::from(i) / f32::from(CURVE_SEGMENTS);
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
    BOWS.into_iter()
        .filter_map(|bow| {
            let lift = normal * (bow * BOW_STEP * points_per_world_unit);
            let c1 = p0 + chord / 3.0 + lift;
            let c2 = p0 + chord * (2.0 / 3.0) + lift;
            let path = clip(&clip(&curve(c1, c2), a), b);
            (path.len() >= 2).then(|| (crossings(&path), polyline_length(&path), path))
        })
        .min_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)))
        .map(|(_, _, path)| path)
        .unwrap_or_default()
}

/// The part of a polyline outside `r`, which one of its ends is inside: cut where it crosses
/// `r`'s edge. A polyline with neither end inside is returned as it is.
fn clip(path: &[Pos2], r: Rect) -> Vec<Pos2> {
    let reversed = !path.first().is_some_and(|p| r.contains(*p));
    let mut path = path.to_vec();
    if reversed {
        path.reverse();
    }
    if path.first().is_some_and(|p| r.contains(*p)) {
        let Some(k) = path.iter().position(|p| !r.contains(*p)) else {
            return Vec::new();
        };
        // Bisects the segment from the inside point k - 1 to the outside point k.
        let (mut inside, mut outside) = (path[k - 1], path[k]);
        for _ in 0..20 {
            let mid = inside.lerp(outside, 0.5);
            if r.contains(mid) {
                inside = mid;
            } else {
                outside = mid;
            }
        }
        path.splice(..k, [outside]);
    }
    if reversed {
        path.reverse();
    }
    path
}

/// Points every `step` along a polyline, with the direction there.
fn along(path: &[Pos2], step: f32) -> Vec<(Pos2, egui::Vec2)> {
    let mut out = Vec::new();
    let (mut next, mut at) = (0.0, 0.0);
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

/// A worktree's rebase in progress is a zigzag from its HEAD's node to the branch being
/// rebased. A merge in progress, or a stopped cherry-pick, is the dashed edge it would add, to
/// the commit being taken, with its arrowhead where the graph's arrows go.
pub fn paint_in_progress(
    painter: &Painter,
    canvas: Rect,
    view: &View,
    scene: &Scene,
    palette: &Palette,
    settings: &Settings,
    marks: &Marks,
) {
    let (repo, graph) = (&*scene.repo, &scene.graph);
    let node = |c: CommitIx| graph.node_of(c).map(|n| n as usize);
    let zoom = view.fixed(view.zoom).max(0.5);
    let amplitude = 3.0 * zoom;
    let stroke = Stroke::new(line_width(view), palette.stuck);
    let (margin, style) = (amplitude + 3.0, settings.edge_style);
    let route = |head: Option<usize>, other: Option<usize>| {
        let (head, other) = (head?, other?);
        (head != other).then(|| special_route(scene, view, canvas, head, other, margin, style))
    };
    for &(head, branch) in &marks.rebasing {
        let Some(path) = route(node(head), node_of_ref(repo, graph, branch)) else {
            continue;
        };
        let zigzag: Vec<Pos2> = along(&path, 4.0 * zoom)
            .into_iter()
            .enumerate()
            .map(|(i, (p, dir))| {
                let side = if i % 2 == 0 { 1.0 } else { -1.0 };
                p + dir.rot90() * (amplitude * side)
            })
            .collect();
        painter.add(Shape::line(zigzag, stroke));
    }
    // An arrow even with the graph's turned off: it says which way the merge goes.
    let arrows = match settings.arrows {
        Arrows::None => Arrows::ToParent,
        arrows => arrows,
    };
    let arrow_len = view.fixed(ARROW_LEN * view.zoom.max(0.4));
    for &(head, theirs) in &marks.merging {
        let Some(path) = route(node(head), node(theirs)) else {
            continue;
        };
        let dash = 6.0 * zoom;
        painter.extend(Shape::dashed_line(&path, stroke, dash, dash * 0.7));
        let triangles = arrowhead_points(&path, arrows, arrow_len)
            .into_iter()
            .flatten();
        for triangle in triangles {
            painter.add(Shape::convex_polygon(
                triangle.to_vec(),
                palette.stuck,
                Stroke::NONE,
            ));
        }
    }
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

/// `feature/x 3|2` for the hovered node's branch, else the selected node's, else HEAD's.
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
    let weak = ui.visuals().weak_text_color();
    ui.label(RichText::new(&repo.refs[u.branch].name).color(weak))
        .on_hover_text(u.short_name());
    let spacing = ui.spacing().item_spacing.x;
    ui.spacing_mut().item_spacing.x = 0.0;
    if u.is_gone() {
        ui.label(RichText::new("gone").monospace().color(weak))
            .on_hover_text(u.short_name());
    } else {
        counts_ui(ui, u.ahead(), u.behind());
    }
    ui.spacing_mut().item_spacing.x = spacing;
    ui.separator();
}

/// git's ahead|behind counts, `3|2`. Leaves the item spacing at zero.
pub fn counts_ui(ui: &mut egui::Ui, ahead: usize, behind: usize) {
    let palette = Palette::new(ui.visuals().dark_mode, &[]);
    let weak = ui.visuals().weak_text_color();
    let count = |n: usize, color: Color32| {
        RichText::new(n.to_string())
            .monospace()
            .strong()
            .color(if n == 0 { weak } else { color })
    };
    ui.spacing_mut().item_spacing.x = 0.0;
    ui.label(count(ahead, palette.ahead)).on_hover_text("ahead");
    ui.label(RichText::new("|").monospace().color(weak));
    ui.label(count(behind, palette.lost))
        .on_hover_text("behind");
}

/// The commits *Compare → Upstream* compares, from the upstream to the branch, or why there
/// are none.
pub type ComparePair = Result<(parterre_core::Oid, parterre_core::Oid), &'static str>;

/// For *Compare → Upstream*: the upstream of each branch on `node` that has one.
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

const COUNT_FONT_SIZE: f32 = 11.5;
const COUNT_SPACING: f32 = 5.0;
const COUNT_ARROW_SIZE: f32 = 10.0;
const COUNT_ARROW_GAP: f32 = 1.0;

/// The width [`paint_counts`] takes.
pub fn counts_width(painter: &Painter, (ahead, behind): (usize, usize)) -> f32 {
    let font = FontId::proportional(COUNT_FONT_SIZE);
    [ahead, behind]
        .into_iter()
        .filter(|&n| n > 0)
        .map(|n| {
            let g = painter.layout_no_wrap(n.to_string(), font.clone(), Color32::WHITE);
            COUNT_SPACING + COUNT_ARROW_SIZE + COUNT_ARROW_GAP + g.size().x
        })
        .sum()
}

/// `↑2 ↓1`, left-aligned at `at`.
pub fn paint_counts(painter: &Painter, at: Pos2, (ahead, behind): (usize, usize), color: Color32) {
    let font = FontId::proportional(COUNT_FONT_SIZE);
    let mut x = at.x;
    for (n, glyph) in [(ahead, glyphs::AHEAD), (behind, glyphs::BEHIND)] {
        if n == 0 {
            continue;
        }
        x += COUNT_SPACING;
        let icon = Rect::from_min_size(
            pos2(x, at.y - COUNT_ARROW_SIZE / 2.0),
            vec2(COUNT_ARROW_SIZE, COUNT_ARROW_SIZE),
        );
        crate::widgets::paint_glyph(painter, icon, glyph, color);
        x += COUNT_ARROW_SIZE + COUNT_ARROW_GAP;
        let g = painter.layout_no_wrap(n.to_string(), font.clone(), color);
        let w = g.size().x;
        painter.galley(pos2(x, at.y - g.size().y / 2.0), g, color);
        x += w;
    }
}
