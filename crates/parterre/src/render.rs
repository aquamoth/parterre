use eframe::egui::text::{LayoutJob, TextFormat};
use eframe::egui::{
    Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2,
    vec2,
};

use crate::scene::{
    CORNER_RADIUS, Checkout, FONT_SIZE, MARGIN_X, Row, RowKind, Scene, WORKTREE_LABEL_INDENT,
    to_pos,
};
use crate::settings::{Arrows, EdgeStyle, Settings};
use crate::theme::{Palette, text_on};
use crate::view::View;
use crate::widgets;
use parterre_core::glyphs;

#[derive(Clone, Debug, Default)]
pub struct Marks {
    pub hovered: Option<usize>,
    pub hovered_edge: Option<usize>,
    /// Per node.
    pub selected: Vec<bool>,
    /// Per node: would move along if the hovered node were dragged.
    pub preview: Vec<bool>,
    pub selected_edge: Option<usize>,
    /// An index into [`Scene::pull_requests`].
    pub hovered_pull_request: Option<usize>,
    /// Per node.
    pub search_hits: Vec<bool>,
    /// The node marked for comparison.
    pub marked: Option<usize>,
    pub show_upstreams: bool,
    /// Each worktree with a rebase in progress: its HEAD, and the branch it is rebasing (an
    /// index into [`parterre_core::Repo::refs`]).
    pub rebasing: Vec<(parterre_core::CommitIx, usize)>,
    /// Each worktree with a merge or a cherry-pick in progress: its HEAD, and the commit being
    /// merged or picked.
    pub merging: Vec<(parterre_core::CommitIx, parterre_core::CommitIx)>,
}

impl Marks {
    fn is_hit(&self, node: usize) -> bool {
        is_flagged(&self.search_hits, node)
    }

    fn is_selected(&self, node: usize) -> bool {
        is_flagged(&self.selected, node)
    }

    fn is_previewed(&self, node: usize) -> bool {
        is_flagged(&self.preview, node)
    }

    fn is_emphasised(&self, node: usize) -> bool {
        self.hovered == Some(node) || self.is_selected(node)
    }
}

fn is_flagged(per_node: &[bool], node: usize) -> bool {
    per_node.get(node).copied().unwrap_or(false)
}

const MIN_DRAWN_TEXT_PX: f32 = 4.0;
/// At 100%. TortoiseGit's is smaller; this one stays visible over bundled edges.
pub const ARROW_LEN: f32 = 13.0;
/// How far (at 100%) an edge running against the flow (a node dragged past its parent) goes on
/// along the flow before it turns round, and how far its detour keeps off the boxes.
const HOOK_LEN: f32 = parterre_core::route::TURN_LENGTH;
const OFFSCREEN_MARGIN: f32 = 40.0;

fn visible_area(canvas: Rect) -> Rect {
    canvas.expand(OFFSCREEN_MARGIN)
}

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
    let visible = visible_area(canvas);
    // Sizes are worked out at 100% text size, as `zoom` is shown, and drawn `fixed`, so the
    // graph looks the same at any text size.
    let zoom = view.zoom;
    let fixed = |len: f32| view.fixed(len);
    let width = fixed((2.0 * zoom).max(1.0));

    // Edges first (TortoiseGit draws them on top; boxes on top reads better with dragging).
    let mut emphasised = Vec::new();
    for (e, edge) in scene.graph.edges.iter().enumerate() {
        let (c, p) = (edge.child as usize, edge.parent as usize);
        if marks.hovered_edge == Some(e)
            || marks.selected_edge == Some(e)
            || settings.highlight_edges && (marks.is_emphasised(c) || marks.is_emphasised(p))
        {
            emphasised.push(e);
            continue;
        }
        let stroke = Stroke::new(width, palette.edge);
        paint_edge(painter, canvas, view, scene, settings, e, stroke);
    }
    for e in emphasised {
        let stroke = Stroke::new(width * 1.6, palette.selection);
        paint_edge(painter, canvas, view, scene, settings, e, stroke);
    }
    if marks.show_upstreams {
        crate::upstreams::paint(painter, canvas, view, scene, palette, settings, marks);
    }
    crate::upstreams::paint_in_progress(painter, canvas, view, scene, settings, palette, marks);

    let font = FontId::monospace(fixed(FONT_SIZE * zoom));
    let draw_text = FONT_SIZE * zoom >= MIN_DRAWN_TEXT_PX;
    let radius = fixed(CORNER_RADIUS * zoom).round().clamp(0.0, 255.0) as u8;
    let row_height = fixed(scene.row_height * zoom);
    for (i, visual) in scene.visuals.iter().enumerate() {
        let rect = view.rect_to_screen(canvas, scene.node_rect(i));
        if !visible.intersects(rect) {
            continue;
        }
        for ((row_rect, corners), row) in
            node_rows(rect, visual.rows.len(), row_height, radius).zip(&visual.rows)
        {
            let (fill, border, text) = row_colors(row, palette);
            painter.rect(
                row_rect,
                corners,
                fill,
                Stroke::new(fixed(1.0), border),
                StrokeKind::Inside,
            );
            if !draw_text {
                continue;
            }
            if let RowKind::PullRequest { index, .. } = row.kind {
                // The label's lengths all grow with the zoom, so this keeps them their size
                // on screen, whatever the text size.
                let (end, icon) = pull_request_label(row_rect, row.width, fixed(zoom));
                let number =
                    painter.text(end, Align2::RIGHT_CENTER, &row.label, font.clone(), text);
                widgets::paint_glyph(painter, icon, glyphs::PULL_REQUEST, text);
                if marks.hovered_pull_request == Some(index) {
                    let y = number.bottom() - fixed(zoom);
                    let stroke = Stroke::new(fixed(zoom.max(1.0)), text);
                    painter.hline(number.x_range(), y, stroke);
                }
            } else if let Some(c) = row.worktree() {
                let (start, icon) = worktree_label(row_rect, fixed(zoom));
                widgets::paint_glyph(painter, icon, worktree_glyph(c), text);
                let format = TextFormat {
                    font_id: font.clone(),
                    color: text,
                    italics: row.is_detached_worktree(),
                    ..TextFormat::default()
                };
                let galley =
                    painter.layout_job(LayoutJob::single_section(row.label.clone(), format));
                let at = Pos2::new(start.x, start.y - galley.size().y / 2.0);
                painter.galley(at, galley, text);
            } else {
                painter.text(
                    Pos2::new(row_rect.min.x + fixed(MARGIN_X * zoom), row_rect.center().y),
                    Align2::LEFT_CENTER,
                    &row.label,
                    font.clone(),
                    text,
                );
            }
        }

        let outline = |w: f32, color: Color32| {
            let w = fixed(w);
            painter.rect_stroke(
                rect.expand(w / 2.0 + fixed(1.0)),
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
        if marks.marked == Some(i) {
            paint_mark(painter, rect, zoom, palette);
        }
    }

    if settings.show_hidden_counts && FONT_SIZE * zoom * 0.85 >= MIN_DRAWN_TEXT_PX {
        paint_hidden_counts(painter, canvas, view, scene, palette, visible);
    }
}

fn paint_mark(painter: &Painter, node: Rect, zoom: f32, palette: &Palette) {
    let z = zoom.max(0.5);
    let size = vec2(10.0 * z, 16.0 * z);
    let min = Pos2::new(node.right() - 8.0 * z - size.x, node.top() - 5.0 * z);
    let stroke = Stroke::new(z.min(1.5), palette.background);
    widgets::paint_ribbon(
        painter,
        Rect::from_min_size(min, size),
        palette.marked,
        stroke,
    );
}

fn paint_edge(
    painter: &Painter,
    canvas: Rect,
    view: &View,
    scene: &Scene,
    settings: &Settings,
    e: usize,
    stroke: Stroke,
) {
    let Some(path) = edge_path(
        scene,
        e,
        settings.edge_style,
        |p| view.to_screen(canvas, p),
        Some(visible_area(canvas)),
    ) else {
        return;
    };
    let len = view.fixed(ARROW_LEN * view.zoom.max(0.4));
    let head = arrowhead_points(&path, settings.arrows, len);
    painter.add(Shape::line(path, stroke));
    for triangle in head.into_iter().flatten() {
        painter.add(Shape::convex_polygon(
            triangle.to_vec(),
            stroke.color,
            Stroke::NONE,
        ));
    }
}

/// `to_screen` must be a view transform: a uniform scale and a translation. Either style leaves
/// the child from its older side and enters the parent from its newer side. `None` if the edge
/// lies entirely outside `visible`.
pub fn edge_path(
    scene: &Scene,
    e: usize,
    style: EdgeStyle,
    to_screen: impl Fn(Pos2) -> Pos2,
    visible: Option<Rect>,
) -> Option<Vec<Pos2>> {
    let edge = scene.graph.edges[e];
    let (c, p) = (edge.child as usize, edge.parent as usize);
    let points: Vec<Pos2> = scene
        .net
        .edge_points(e)
        .map(|point| to_screen(to_pos(point)))
        .collect();
    let map_rect = |r: Rect| Rect::from_two_pos(to_screen(r.min), to_screen(r.max));
    let child = map_rect(scene.node_rect(c));
    let parent = map_rect(scene.node_rect(p));
    let zoom = to_screen(Pos2::new(1.0, 0.0)).x - to_screen(Pos2::ZERO).x;
    let hook = HOOK_LEN * zoom;
    // Detours run beside the end boxes, so those bound the drawing too.
    let bounds = Rect::from_points(&points)
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
    let turned = scene.net.has_turned_route(e) && points.len() >= 4;
    let path = match style {
        EdgeStyle::Straight => straight_path(&points, child, parent, flow, hook, turned),
        EdgeStyle::Curved => curved_path(&points, child, parent, flow, hook, turned),
    };
    (path.len() >= 2).then_some(path)
}

pub fn arrowhead_points(path: &[Pos2], arrows: Arrows, len: f32) -> Option<[[Pos2; 3]; 2]> {
    let n = path.len();
    match arrows {
        Arrows::ToParent => arrowhead(path[n - 2], path[n - 1], len),
        Arrows::ToChild => arrowhead(path[1], path[0], len),
        Arrows::None => None,
    }
}

/// In screen space.
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

/// The points an edge is drawn through: the middle of the child's side facing older commits,
/// the bend points, and the middle of the parent's side facing newer ones.
fn knots(points: &[Pos2], child: Rect, parent: Rect, flow: Vec2) -> Vec<Pos2> {
    let half_depth = |r: Rect| (flow.x.abs() * r.width() + flow.y.abs() * r.height()) / 2.0;
    let n = points.len();
    let mut knots = Vec::with_capacity(n);
    knots.push(child.center() + flow * half_depth(child));
    knots.extend_from_slice(&points[1..n - 1]);
    knots.push(parent.center() - flow * half_depth(parent));
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

/// A segment against the flow takes a [`Detour`], leaving and arriving along the flow, unless
/// the edge is `turned`: routed round its nodes already (see
/// [`parterre_core::physics::Net::has_turned_route`]).
///
/// Deliberately unlike TortoiseGit, which aims every edge at the box centres and clips it at
/// the border, so that edges can meet a box on any side and direction is hard to see.
fn straight_path(
    points: &[Pos2],
    child: Rect,
    parent: Rect,
    flow: Vec2,
    hook: f32,
    turned: bool,
) -> Vec<Pos2> {
    let knots = knots(points, child, parent, flow);
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

/// Leaves the child, passes every bend point and enters the parent along the flow. A segment
/// against the flow turns round along a [`Detour`].
///
/// A `turned` edge's route starts and ends with its turns round the nodes (see
/// [`parterre_core::physics::Net::has_turned_route`]). The curve makes those turns itself, and
/// passes the bend points in between against the flow.
fn curved_path(
    points: &[Pos2],
    child: Rect,
    parent: Rect,
    flow: Vec2,
    hook: f32,
    turned: bool,
) -> Vec<Pos2> {
    let n = points.len();
    let mut knots = knots(points, child, parent, flow);
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

const PULL_REQUEST_GAP: f32 = 4.0;

/// For a number `width` wide at 100%: where the number ends, level with the row's middle, and
/// the glyph's box. The number is right-aligned, so it stands apart from the ref names above.
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

/// Where the name starts, level with the row's middle, and the folder glyph's box.
pub fn worktree_label(row: Rect, zoom: f32) -> (Pos2, Rect) {
    let left = row.min.x + MARGIN_X * zoom;
    let side = FONT_SIZE * zoom;
    let icon = Rect::from_center_size(
        Pos2::new(left + side / 2.0, row.center().y),
        Vec2::splat(side),
    );
    (
        Pos2::new(left + WORKTREE_LABEL_INDENT * zoom, row.center().y),
        icon,
    )
}

pub fn worktree_glyph(c: Checkout) -> glyphs::Glyph {
    if c.missing {
        glyphs::FOLDER_GONE
    } else {
        glyphs::FOLDER
    }
}

/// Fill, border and text colour.
pub fn row_colors(row: &Row, palette: &Palette) -> (Color32, Color32, Color32) {
    let fill = row_fill(row, palette);
    match &row.kind {
        RowKind::Hash => (fill, palette.plain_border, palette.plain_text),
        RowKind::Ref { .. } | RowKind::Worktree(_) | RowKind::PullRequest { .. } => {
            (fill, fill, text_on(fill))
        }
    }
}

fn worktree_fill(c: Checkout, palette: &Palette) -> Color32 {
    if c.missing {
        palette.missing_worktree
    } else {
        palette.worktree
    }
}

fn row_fill(row: &Row, palette: &Palette) -> Color32 {
    match &row.kind {
        RowKind::Hash => palette.plain_fill,
        RowKind::Worktree(c) => worktree_fill(*c, palette),
        // Every worktree is in the worktree colour, but the open one's HEAD stays red, and
        // the colour beats the branch colour rules (#132).
        RowKind::Ref {
            worktree: Some(c),
            head: false,
            ..
        } => worktree_fill(*c, palette),
        RowKind::Ref { kind, head, .. } => palette.ref_fill(*kind, *head, &row.label),
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
    let font = FontId::proportional(view.fixed(FONT_SIZE * 0.85 * view.zoom));
    let color = palette.edge.gamma_multiply(0.7);
    for (e, edge) in scene.graph.edges.iter().enumerate() {
        if edge.hidden == 0 {
            continue;
        }
        let points: Vec<Pos2> = scene.net.edge_points(e).map(to_pos).collect();
        let half = points.len() / 2;
        let mid = if points.len() % 2 == 1 {
            points[half]
        } else {
            points[half - 1].lerp(points[half], 0.5)
        };
        let at = view.to_screen(canvas, mid);
        if visible.contains(at) {
            painter.text(
                at + vec2(view.fixed(4.0), 0.0),
                Align2::LEFT_CENTER,
                format!("+{}", edge.hidden),
                font.clone(),
                color,
            );
        }
    }
}

/// Returns the world rectangle the miniature shows, and its scale.
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
    let mini_edge = |e: usize| -> Vec<Pos2> {
        scene
            .net
            .edge_points(e)
            .map(|p| to_mini(to_pos(p)))
            .collect()
    };
    let edge_stroke = Stroke::new(1.0, palette.edge.gamma_multiply(0.35));
    for e in 0..scene.graph.edges.len() {
        painter.add(Shape::line(mini_edge(e), edge_stroke));
    }
    if let Some(e) = selected_edge {
        let stroke = Stroke::new(2.0, palette.selection);
        painter.add(Shape::line(mini_edge(e), stroke));
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
    use parterre_core::RefKind;

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

    #[test]
    fn every_worktree_row_is_in_the_worktree_colour_but_head() {
        let palette = Palette::light();
        let row = |label: &str, kind| Row {
            label: label.into(),
            kind,
            width: 0.0,
        };
        let checkout = |missing| Some(Checkout { index: 1, missing });
        let branch = |worktree, head| RowKind::Ref {
            kind: RefKind::LocalBranch,
            head,
            worktree,
        };
        assert_eq!(
            row_fill(&row("topic", branch(None, false)), &palette),
            palette.local_branch
        );
        assert_eq!(
            row_fill(&row("topic", branch(checkout(false), false)), &palette),
            palette.worktree
        );
        assert_eq!(
            row_fill(&row("topic", branch(checkout(true), false)), &palette),
            palette.missing_worktree
        );
        // The open worktree's branch and detached HEAD are red as ever.
        assert_eq!(
            row_fill(&row("main", branch(checkout(false), true)), &palette),
            palette.current_branch
        );
        let detached = RowKind::Ref {
            kind: RefKind::DetachedHead,
            head: true,
            worktree: checkout(false),
        };
        assert_eq!(
            row_fill(&row("wt", detached), &palette),
            palette.current_branch
        );
        // A detached worktree differs by its italics alone.
        assert_eq!(
            row_fill(
                &row("wt", RowKind::Worktree(checkout(false).unwrap())),
                &palette
            ),
            palette.worktree
        );
    }
}
