//! Painting a [`Scene`]: TortoiseGit-style nodes (one coloured row per ref) and edges.

use eframe::egui::{
    Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2,
    vec2,
};

use crate::scene::{CORNER_RADIUS, FONT_SIZE, MARGIN_X, Row, RowKind, Scene, to_pos};
use crate::settings::{Arrows, EdgeStyle, Settings};
use crate::theme::{Palette, text_on};
use crate::view::View;
use crate::widgets;
use parterre_core::glyphs;

/// Nodes and edges that get special emphasis.
#[derive(Clone, Debug, Default)]
pub struct Marks {
    pub hovered: Option<usize>,
    pub hovered_edge: Option<usize>,
    /// Per node: selected.
    pub selected: Vec<bool>,
    /// Per node: would move along if the hovered node were dragged.
    pub preview: Vec<bool>,
    pub selected_edge: Option<usize>,
    /// The pull request (an index into [`Scene::pull_requests`]) whose label is under the
    /// pointer: its number is underlined, as a link.
    pub hovered_pull_request: Option<usize>,
    /// Per node: matches the current search.
    pub search_hits: Vec<bool>,
}

impl Marks {
    fn is_hit(&self, node: usize) -> bool {
        self.search_hits.get(node).copied().unwrap_or(false)
    }

    fn is_selected(&self, node: usize) -> bool {
        self.selected.get(node).copied().unwrap_or(false)
    }

    fn is_previewed(&self, node: usize) -> bool {
        self.preview.get(node).copied().unwrap_or(false)
    }

    fn emphasised(&self, node: usize) -> bool {
        self.hovered == Some(node) || self.is_selected(node)
    }
}

/// Text smaller than this many pixels is not drawn.
const MIN_TEXT_PX: f32 = 4.0;
/// Arrowhead length at 100%. TortoiseGit's is smaller; this one stays visible over bundled
/// edges.
pub const ARROW_LEN: f32 = 13.0;
/// How far (at 100%) an edge that runs against the flow (a node dragged past its parent)
/// continues along the flow before it turns round, and how far its detour keeps off the boxes.
const HOOK_LEN: f32 = parterre_core::route::TURN;

pub fn paint_scene(
    painter: &Painter,
    canvas: Rect,
    view: &View,
    scene: &Scene,
    palette: &Palette,
    settings: &Settings,
    marks: &Marks,
) {
    painter.rect_filled(canvas, 0.0, palette.background);
    let visible = canvas.expand(40.0);
    let zoom = view.zoom;
    let width = (2.0 * zoom).max(1.0);

    // Edges first (TortoiseGit draws them on top; boxes on top reads better with dragging).
    let mut emphasised = Vec::new();
    for (e, edge) in scene.graph.edges.iter().enumerate() {
        let (c, p) = (edge.child as usize, edge.parent as usize);
        if marks.hovered_edge == Some(e)
            || marks.selected_edge == Some(e)
            || settings.highlight_edges && (marks.emphasised(c) || marks.emphasised(p))
        {
            emphasised.push(e);
            continue;
        }
        paint_edge(
            painter,
            canvas,
            view,
            scene,
            settings,
            e,
            visible,
            Stroke::new(width, palette.edge),
        );
    }
    for e in emphasised {
        let stroke = Stroke::new(width * 1.6, palette.selection);
        paint_edge(painter, canvas, view, scene, settings, e, visible, stroke);
    }

    let font = FontId::monospace(FONT_SIZE * zoom);
    let draw_text = FONT_SIZE * zoom >= MIN_TEXT_PX;
    let radius = (CORNER_RADIUS * zoom).round().clamp(0.0, 255.0) as u8;
    let row_h = scene.row_height * zoom;
    for (i, visual) in scene.visuals.iter().enumerate() {
        let rect = view.rect_to_screen(canvas, scene.node_rect(i));
        if !visible.intersects(rect) {
            continue;
        }
        for ((row_rect, corners), row) in
            node_rows(rect, visual.rows.len(), row_h, radius).zip(&visual.rows)
        {
            let (fill, border, text) = row_colors(row, palette);
            painter.rect(
                row_rect,
                corners,
                fill,
                Stroke::new(1.0, border),
                StrokeKind::Inside,
            );
            if !draw_text {
                continue;
            }
            if let RowKind::PullRequest { index, .. } = row.kind {
                let (end, icon) = pull_request_label(row_rect, row.width, zoom);
                let number =
                    painter.text(end, Align2::RIGHT_CENTER, &row.label, font.clone(), text);
                widgets::paint_glyph(painter, icon, glyphs::PULL_REQUEST, text);
                if marks.hovered_pull_request == Some(index) {
                    let y = number.bottom() - zoom;
                    painter.hline(number.x_range(), y, Stroke::new(zoom.max(1.0), text));
                }
            } else {
                painter.text(
                    Pos2::new(row_rect.min.x + MARGIN_X * zoom, row_rect.center().y),
                    Align2::LEFT_CENTER,
                    &row.label,
                    font.clone(),
                    text,
                );
            }
        }

        let outline = |w: f32, color: Color32| {
            painter.rect_stroke(
                rect.expand(w / 2.0 + 1.0),
                radius,
                Stroke::new(w, color),
                StrokeKind::Middle,
            );
        };
        if marks.is_selected(i) {
            outline((4.0 * zoom).max(2.0), palette.selection);
        } else if marks.is_hit(i) {
            outline((3.0 * zoom).max(2.0), palette.search_hit);
        } else if marks.hovered == Some(i) {
            outline((2.0 * zoom).max(1.0), palette.selection);
        } else if marks.is_previewed(i) {
            outline((2.0 * zoom).max(1.0), palette.selection.gamma_multiply(0.5));
        }
    }

    if settings.show_hidden_counts && FONT_SIZE * zoom * 0.85 >= MIN_TEXT_PX {
        paint_hidden_counts(painter, canvas, view, scene, palette, visible);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_edge(
    painter: &Painter,
    canvas: Rect,
    view: &View,
    scene: &Scene,
    settings: &Settings,
    e: usize,
    visible: Rect,
    stroke: Stroke,
) {
    let Some(path) = edge_path(
        scene,
        e,
        settings.edge_style,
        |p| view.to_screen(canvas, p),
        Some(visible),
    ) else {
        return;
    };
    painter.add(Shape::line(path.clone(), stroke));
    let len = ARROW_LEN * view.zoom.max(0.4);
    if let Some(head) = arrowhead_points(&path, settings.arrows, len) {
        for tri in head {
            painter.add(Shape::convex_polygon(
                tri.to_vec(),
                stroke.color,
                Stroke::NONE,
            ));
        }
    }
}

/// The drawn path of edge `e`, mapped through `to_screen` (a view transform: uniform scale and
/// translation). Either style leaves the child from its older side and enters the parent from
/// its newer side (bottom and top, newest on top). `None` if it lies entirely outside `visible`.
pub fn edge_path(
    scene: &Scene,
    e: usize,
    style: EdgeStyle,
    to_screen: impl Fn(Pos2) -> Pos2,
    visible: Option<Rect>,
) -> Option<Vec<Pos2>> {
    let edge = scene.graph.edges[e];
    let (c, p) = (edge.child as usize, edge.parent as usize);
    let pts: Vec<Pos2> = scene
        .net
        .edge_points(e)
        .map(|pt| to_screen(to_pos(pt)))
        .collect();
    let map_rect = |r: Rect| Rect::from_two_pos(to_screen(r.min), to_screen(r.max));
    let child = map_rect(scene.node_rect(c));
    let parent = map_rect(scene.node_rect(p));
    let zoom = to_screen(Pos2::new(1.0, 0.0)).x - to_screen(Pos2::ZERO).x;
    let hook = HOOK_LEN * zoom;
    // Detours run beside the end boxes, so those bound the drawing too.
    let bounds = Rect::from_points(&pts)
        .union(child)
        .union(parent)
        .expand(hook);
    if let Some(visible) = visible
        && !visible.intersects(bounds)
    {
        return None;
    }
    let f = scene.layout.direction.flow();
    let flow = vec2(f.x, f.y);
    let turned = scene.net.turns(e) && pts.len() >= 4;
    let path = match style {
        EdgeStyle::Straight => straight_path(&pts, child, parent, flow, hook, turned),
        EdgeStyle::Curved => curved_path(&pts, child, parent, flow, hook, turned),
    };
    (path.len() >= 2).then_some(path)
}

/// The two triangles of an arrowhead for `path`, or `None` without arrows.
pub fn arrowhead_points(path: &[Pos2], arrows: Arrows, len: f32) -> Option<[[Pos2; 3]; 2]> {
    let n = path.len();
    match arrows {
        Arrows::ToParent => arrowhead(path[n - 2], path[n - 1], len),
        Arrows::ToChild => arrowhead(path[1], path[0], len),
        Arrows::None => None,
    }
}

/// The edge whose drawn path passes within `tolerance` of `pointer` (screen space), if any.
pub fn edge_at(
    scene: &Scene,
    style: EdgeStyle,
    to_screen: impl Fn(Pos2) -> Pos2 + Copy,
    pointer: Pos2,
    tolerance: f32,
) -> Option<usize> {
    let probe = Rect::from_center_size(pointer, Vec2::splat(2.0 * tolerance));
    let mut best = None;
    let mut best_d = tolerance;
    for e in 0..scene.graph.edges.len() {
        let Some(path) = edge_path(scene, e, style, to_screen, Some(probe)) else {
            continue;
        };
        for w in path.windows(2) {
            let d = distance_to_segment(pointer, w[0], w[1]);
            if d < best_d {
                best_d = d;
                best = Some(e);
            }
        }
    }
    best
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 {
        ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a + ab * t).distance(p)
}

/// Where edges attach to a box: the middle of the side facing older commits (`sign` 1), where
/// edges to parents leave, or of the side facing newer ones (`sign` -1), where edges from
/// children arrive.
fn port(rect: Rect, flow: Vec2, sign: f32) -> Pos2 {
    let depth = flow.x.abs() * rect.width() + flow.y.abs() * rect.height();
    rect.center() + flow * (sign * depth / 2.0)
}

/// The points an edge is drawn through: the child's port, the bend points, the parent's port.
fn knots(pts: &[Pos2], child: Rect, parent: Rect, flow: Vec2) -> Vec<Pos2> {
    let n = pts.len();
    let mut knots = Vec::with_capacity(n);
    knots.push(port(child, flow, 1.0));
    knots.extend_from_slice(&pts[1..n - 1]);
    knots.push(port(parent, flow, -1.0));
    knots
}

/// The line that segment `k` of an edge through `knots` detours along when it runs against the
/// flow (a node dragged past its parent): beside the boxes the segment joins, on the side it
/// is heading to, so the edge can still leave and arrive along the flow without crossing them.
struct Detour {
    /// Unit vector across the flow, pointing to the detour side.
    lateral: Vec2,
    /// Where the detour runs, as a coordinate along `lateral`.
    at: f32,
}

impl Detour {
    fn new(knots: &[Pos2], k: usize, child: Rect, parent: Rect, flow: Vec2, hook: f32) -> Detour {
        let (a, b) = (knots[k], knots[k + 1]);
        let across = vec2(flow.y.abs(), flow.x.abs());
        let lateral = if (b - a).dot(across) < 0.0 {
            -across
        } else {
            across
        };
        let boxes = [
            (k == 0).then_some(child),
            (k + 2 == knots.len()).then_some(parent),
        ];
        let far = boxes
            .into_iter()
            .flatten()
            .flat_map(|r| [r.min, r.max])
            .chain([a, b])
            .map(|p| p.to_vec2().dot(lateral))
            .fold(f32::MIN, f32::max);
        Detour {
            lateral,
            at: far + hook,
        }
    }

    /// `p` moved sideways onto the detour.
    fn beside(&self, p: Pos2) -> Pos2 {
        p + self.lateral * (self.at - p.to_vec2().dot(self.lateral))
    }
}

/// Straight segments from the child's port through the bend points to the parent's port. A
/// segment against the flow takes a [`Detour`], leaving and arriving along the flow, unless the
/// edge is `turned`: routed round its nodes already (see [`parterre_core::physics::Net::turns`]).
///
/// Deliberately unlike TortoiseGit, which aims every edge at the box centres and clips it at
/// the border, so that edges can meet a box on any side and direction is hard to see.
fn straight_path(
    pts: &[Pos2],
    child: Rect,
    parent: Rect,
    flow: Vec2,
    hook: f32,
    turned: bool,
) -> Vec<Pos2> {
    let knots = knots(pts, child, parent, flow);
    if turned {
        return knots;
    }
    let mut path = vec![knots[0]];
    for k in 0..knots.len() - 1 {
        let (a, b) = (knots[k], knots[k + 1]);
        if (b - a).dot(flow) < 0.0 {
            let detour = Detour::new(&knots, k, child, parent, flow, hook);
            let (leave, arrive) = (a + flow * hook, b - flow * hook);
            path.extend([leave, detour.beside(leave), detour.beside(arrive), arrive]);
        }
        path.push(b);
    }
    path
}

/// A smooth path that leaves the child's port along the history direction, passes through
/// every bend point with a tangent along that direction, and enters the parent's port the same
/// way. A segment against the flow turns round along a [`Detour`].
///
/// A `turned` edge's route starts and ends with its turns round the nodes (see
/// [`parterre_core::physics::Net::turns`]). The curve makes those turns itself, and passes the
/// bend points in between against the flow.
fn curved_path(
    pts: &[Pos2],
    child: Rect,
    parent: Rect,
    flow: Vec2,
    hook: f32,
    turned: bool,
) -> Vec<Pos2> {
    let n = pts.len();
    let mut knots = knots(pts, child, parent, flow);
    if turned {
        knots.remove(n - 2);
        knots.remove(1);
    }
    let against = turned && knots.len() > 2;
    let last = knots.len() - 2;
    let mut out = vec![knots[0]];
    for k in 0..=last {
        let (a, b) = (knots[k], knots[k + 1]);
        let ahead = (b - a).dot(flow);
        if against {
            // Which way along the flow the curve passes each end.
            let ta = if k == 0 { 1.0 } else { -1.0 };
            let tb = if k == last { 1.0 } else { -1.0 };
            let reach = if ta == tb {
                (ahead.abs() / 2.0).max(hook / 2.0)
            } else {
                (ahead.abs() / 2.0).max(hook)
            };
            cubic(
                &mut out,
                a,
                a + flow * (ta * reach),
                b - flow * (tb * reach),
                b,
            );
        } else if ahead < 0.0 {
            // Down from `a`, back up along the detour, and down into `b` from above.
            let detour = Detour::new(&knots, k, child, parent, flow, hook);
            let turn = detour.beside(a.lerp(b, 0.5));
            let reach = -ahead / 2.0 + hook;
            cubic(&mut out, a, a + flow * hook, turn + flow * reach, turn);
            cubic(&mut out, turn, turn - flow * reach, b - flow * hook, b);
        } else {
            let reach = (ahead / 2.0).max(hook / 2.0).max(4.0);
            cubic(&mut out, a, a + flow * reach, b - flow * reach, b);
        }
    }
    out
}

/// Appends points along the cubic Bézier curve from `a` (already in `out`) to `b`.
fn cubic(out: &mut Vec<Pos2>, a: Pos2, c1: Pos2, c2: Pos2, b: Pos2) {
    let hull = (c1 - a).length() + (c2 - c1).length() + (b - c2).length();
    let steps = ((hull / 6.0) as usize).clamp(4, 32);
    for s in 1..=steps {
        let t = s as f32 / steps as f32;
        let u = 1.0 - t;
        let p = a.to_vec2() * (u * u * u)
            + c1.to_vec2() * (3.0 * u * u * t)
            + c2.to_vec2() * (3.0 * u * t * t)
            + b.to_vec2() * (t * t * t);
        out.push(p.to_pos2());
    }
}

/// Arrowhead with its tip at `tip`, pointing away from `from` (TortoiseGit: wings at ±22.5°,
/// notch 0.6 of the wing length back), as two triangles.
fn arrowhead(from: Pos2, tip: Pos2, len: f32) -> Option<[[Pos2; 3]; 2]> {
    let d = tip - from;
    if d.length_sq() < 1e-6 {
        return None;
    }
    let dir = d.normalized();
    let angle = std::f32::consts::PI / 8.0;
    let rot = |v: Vec2, a: f32| vec2(v.x * a.cos() - v.y * a.sin(), v.x * a.sin() + v.y * a.cos());
    let wing1 = tip - rot(dir, angle) * len;
    let wing2 = tip - rot(dir, -angle) * len;
    let notch = tip - dir * (0.6 * len);
    Some([[tip, wing1, notch], [tip, notch, wing2]])
}

/// Screen rectangles and corner radii of a node's rows (only the outer corners are rounded).
pub fn node_rows(
    rect: Rect,
    rows: usize,
    row_height: f32,
    radius: u8,
) -> impl Iterator<Item = (Rect, CornerRadius)> {
    (0..rows).map(move |r| {
        let row_rect = Rect::from_min_size(
            rect.min + vec2(0.0, r as f32 * row_height),
            vec2(rect.width(), row_height),
        );
        let corners = CornerRadius {
            nw: if r == 0 { radius } else { 0 },
            ne: if r == 0 { radius } else { 0 },
            sw: if r + 1 == rows { radius } else { 0 },
            se: if r + 1 == rows { radius } else { 0 },
        };
        (row_rect, corners)
    })
}

/// Space between the pull-request glyph and the number, at 100%.
const PULL_REQUEST_GAP: f32 = 4.0;

/// Where a pull request's label goes in its row, for a number `width` wide at 100%: the right
/// end of the number, level with the row's middle, and the glyph's box just left of it. The
/// number is right-aligned, so it stands apart from the ref names above it.
pub fn pull_request_label(row: Rect, width: f32, zoom: f32) -> (Pos2, Rect) {
    let end = Pos2::new(row.max.x - MARGIN_X * zoom, row.center().y);
    let side = FONT_SIZE * zoom;
    let icon = Rect::from_center_size(
        Pos2::new(
            end.x - (width + PULL_REQUEST_GAP) * zoom - side / 2.0,
            end.y,
        ),
        Vec2::splat(side),
    );
    (end, icon)
}

/// Fill, border and text colour of a row.
pub fn row_colors(row: &Row, palette: &Palette) -> (Color32, Color32, Color32) {
    let fill = row_fill(row, palette);
    match &row.kind {
        RowKind::Hash => (fill, palette.plain_border, palette.plain_text),
        RowKind::Ref { .. } | RowKind::PullRequest { .. } => (fill, fill, text_on(fill)),
    }
}

fn row_fill(row: &Row, palette: &Palette) -> Color32 {
    match &row.kind {
        RowKind::Hash => palette.plain_fill,
        RowKind::Ref { kind, head } => palette.ref_fill(*kind, *head, &row.label),
        RowKind::PullRequest { draft: false, .. } => palette.pull_request,
        RowKind::PullRequest { draft: true, .. } => palette.draft_pull_request,
    }
}

fn paint_hidden_counts(
    painter: &Painter,
    canvas: Rect,
    view: &View,
    scene: &Scene,
    palette: &Palette,
    visible: Rect,
) {
    let font = FontId::proportional(FONT_SIZE * 0.85 * view.zoom);
    let color = palette.edge.gamma_multiply(0.7);
    for (e, edge) in scene.graph.edges.iter().enumerate() {
        if edge.hidden == 0 {
            continue;
        }
        let pts: Vec<Pos2> = scene
            .net
            .edge_points(e)
            .collect::<Vec<_>>()
            .into_iter()
            .map(to_pos)
            .collect();
        let mid = if pts.len() % 2 == 1 {
            pts[pts.len() / 2]
        } else {
            pts[pts.len() / 2 - 1].lerp(pts[pts.len() / 2], 0.5)
        };
        let at = view.to_screen(canvas, mid);
        if visible.contains(at) {
            painter.text(
                at + vec2(4.0, 0.0),
                Align2::LEFT_CENTER,
                format!("+{}", edge.hidden),
                font.clone(),
                color,
            );
        }
    }
}

/// Paints a miniature of the whole graph into `rect` with the visible area outlined.
/// Returns the world rectangle the miniature represents and the scale used.
pub fn paint_overview(
    painter: &Painter,
    rect: Rect,
    canvas: Rect,
    view: &View,
    scene: &Scene,
    palette: &Palette,
    selected_edge: Option<usize>,
) -> (Rect, f32) {
    let world = scene.bounds().expand(20.0);
    let scale = (rect.width() / world.width()).min(rect.height() / world.height());
    let to_mini = |p: Pos2| rect.center() + (p - world.center()) * scale;
    painter.rect(
        rect.expand(1.0),
        4.0,
        palette.background,
        Stroke::new(1.0, palette.edge.gamma_multiply(0.5)),
        StrokeKind::Outside,
    );
    let edge_stroke = Stroke::new(1.0, palette.edge.gamma_multiply(0.35));
    for e in 0..scene.graph.edges.len() {
        let pts: Vec<Pos2> = scene
            .net
            .edge_points(e)
            .map(|p| to_mini(to_pos(p)))
            .collect();
        painter.add(Shape::line(pts, edge_stroke));
    }
    if let Some(e) = selected_edge {
        let pts = scene
            .net
            .edge_points(e)
            .map(|p| to_mini(to_pos(p)))
            .collect();
        painter.add(Shape::line(pts, Stroke::new(2.0, palette.selection)));
    }
    for (i, v) in scene.visuals.iter().enumerate() {
        let r = Rect::from_center_size(
            to_mini(scene.node_center(i)),
            (v.size * scale).max(Vec2::splat(2.0)),
        );
        let row = &v.rows[0];
        let fill = match &row.kind {
            RowKind::Hash => palette.plain_border,
            _ => row_fill(row, palette),
        };
        painter.rect_filled(r, 0.0, fill);
    }
    let seen = view.visible_world(canvas);
    let seen_mini = Rect::from_min_max(to_mini(seen.min), to_mini(seen.max)).intersect(rect);
    painter.rect(
        seen_mini,
        0.0,
        Color32::from_black_alpha(40),
        Stroke::new(1.0, palette.selection),
        StrokeKind::Inside,
    );
    (world, scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_request_numbers_are_right_aligned_after_their_glyph() {
        for zoom in [1.0, 2.0] {
            let row = Rect::from_min_size(Pos2::new(100.0, 50.0), vec2(200.0, 22.0) * zoom);
            let (number, icon) = pull_request_label(row, 35.0, zoom);
            // The number ends where ref names would, a margin before the right edge.
            assert_eq!(number.x, row.max.x - MARGIN_X * zoom);
            assert_eq!(number.y, row.center().y);
            // The glyph sits just left of it, as high as the text, inside the row.
            assert!(icon.max.x < number.x - 35.0 * zoom);
            assert!(icon.max.x > number.x - 35.0 * zoom - 8.0 * zoom);
            assert_eq!(icon.height(), FONT_SIZE * zoom);
            assert_eq!(icon.center().y, row.center().y);
            assert!(row.contains_rect(icon));
        }
    }
}
