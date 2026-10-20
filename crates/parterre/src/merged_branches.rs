//! The marks of a squash- or rebase-merged branch (#384): stripes on its label, and a dashed
//! line from it to the commit its content landed as.
//!
//! The stripes are diagonal (45°), in a 20% darker shade of the label's own fill, half the
//! period each, so they work on every fill and leave the name readable. egui has no pattern
//! fill: each stripe is a polygon, a band cut to the label inside its rounded corners.

use eframe::egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, pos2};

use crate::render::{ARROW_LEN, arrowhead_points};
use crate::scene::Scene;
use crate::settings::{Arrows, Settings};
use crate::theme::Palette;
use crate::view::View;

/// Across the stripes, on a graph row at 100%.
pub const PERIOD: f32 = 8.0;
/// A graph row's height at 100% (12 px text, 5 px margins), which [`PERIOD`] is for.
const ROW_HEIGHT: f32 = 24.0;
/// Below this on screen, stripes would blur into a tint.
const MIN_PERIOD: f32 = 3.0;
const SEGMENTS_PER_CORNER: u16 = 4;

/// The stripes' period on a label `height` high, as on a graph row scaled to it.
pub fn period_for(height: f32) -> f32 {
    PERIOD * height / ROW_HEIGHT
}

/// 20% darker.
pub fn shade(fill: Color32) -> Color32 {
    let darker = |c: u8| (f32::from(c) * 0.8).round() as u8;
    Color32::from_rgba_unmultiplied(
        darker(fill.r()),
        darker(fill.g()),
        darker(fill.b()),
        fill.a(),
    )
}

/// The shaded stripes of a label `rect` with `corners`, every `period` across, as convex
/// polygons. They start at its top-left corner, so they move with it.
pub fn polygons(rect: Rect, corners: CornerRadius, period: f32) -> Vec<Vec<Pos2>> {
    let outline = rounded_outline(rect, corners);
    // Across the stripes: constant along each, which rises to the right.
    let across = |p: Pos2| ((p.x - rect.min.x) + (p.y - rect.min.y)) / std::f32::consts::SQRT_2;
    let period = period.max(MIN_PERIOD);
    let end = across(rect.max);
    let mut out = Vec::new();
    let mut start = 0.0;
    while start < end {
        let band = clip(&outline, |p| across(p) - start);
        let band = clip(&band, |p| start + period / 2.0 - across(p));
        if band.len() >= 3 {
            out.push(band);
        }
        start += period;
    }
    out
}

pub fn paint_stripes(
    painter: &Painter,
    rect: Rect,
    corners: CornerRadius,
    period: f32,
    fill: Color32,
) {
    let shade = shade(fill);
    for band in polygons(rect, corners, period) {
        painter.add(Shape::convex_polygon(band, shade, Stroke::NONE));
    }
}

/// Clockwise from the top-left corner's top end, each corner's arc in a few segments.
fn rounded_outline(rect: Rect, corners: CornerRadius) -> Vec<Pos2> {
    let half = rect.width().min(rect.height()) / 2.0;
    let radius = |r: u8| f32::from(r).min(half);
    // Each corner: its arc's centre, its radius, and the angle the arc starts at.
    let arcs = [
        (rect.left_top(), radius(corners.nw), 1.0, 1.0, 180.0),
        (rect.right_top(), radius(corners.ne), -1.0, 1.0, 270.0),
        (rect.right_bottom(), radius(corners.se), -1.0, -1.0, 0.0),
        (rect.left_bottom(), radius(corners.sw), 1.0, -1.0, 90.0),
    ];
    let mut out = Vec::new();
    for (corner, r, sx, sy, from) in arcs {
        if r <= 0.0 {
            out.push(corner);
            continue;
        }
        let centre = pos2(corner.x + sx * r, corner.y + sy * r);
        for i in 0..=SEGMENTS_PER_CORNER {
            let degrees = from + 90.0 * f32::from(i) / f32::from(SEGMENTS_PER_CORNER);
            let (sin, cos) = degrees.to_radians().sin_cos();
            out.push(pos2(centre.x + r * cos, centre.y + r * sin));
        }
    }
    out
}

/// The part of the convex polygon where `inside` is at least 0 (Sutherland–Hodgman).
fn clip(polygon: &[Pos2], inside: impl Fn(Pos2) -> f32) -> Vec<Pos2> {
    let mut out = Vec::new();
    for (i, &p) in polygon.iter().enumerate() {
        let q = polygon[(i + 1) % polygon.len()];
        let (dp, dq) = (inside(p), inside(q));
        if dp >= 0.0 {
            out.push(p);
        }
        if (dp >= 0.0) != (dq >= 0.0) {
            out.push(p.lerp(q, dp / (dp - dq)));
        }
    }
    out
}

/// A dashed line in ahead's colour from each merged branch shown to the commit it landed as,
/// with the graph's arrowhead where it landed, whatever the graph's arrows: it shows where the
/// branch went, not a parent. It says what the dashed line of a rebased branch says: these
/// commits replace those.
pub fn paint_lines(
    painter: &Painter,
    canvas: Rect,
    view: &View,
    scene: &Scene,
    palette: &Palette,
    settings: &Settings,
) {
    if scene.merged.is_empty() {
        return;
    }
    let (repo, graph) = (&*scene.repo, &scene.graph);
    let zoom = view.fixed(view.zoom).max(0.5);
    let stroke = Stroke::new(view.fixed((2.0 * view.zoom).max(1.0)), palette.ahead);
    let arrow_len = view.fixed(ARROW_LEN * view.zoom.max(0.4));
    for (n, node) in graph.nodes.iter().enumerate() {
        let landed = node
            .refs
            .iter()
            .filter_map(|&r| scene.merged.of(repo, r)?.landed)
            .filter_map(|oid| graph.node_of(repo.lookup(&oid)?))
            .map(|t| t as usize)
            .filter(|&t| t != n);
        for t in landed {
            let path = crate::upstreams::special_route(
                scene,
                view,
                canvas,
                n,
                t,
                4.0,
                settings.edge_style,
            );
            if path.len() < 2 {
                continue;
            }
            let dash = 6.0 * zoom;
            painter.extend(Shape::dashed_line(&path, stroke, dash, dash * 0.7));
            let triangles = arrowhead_points(&path, Arrows::ToParent, arrow_len)
                .into_iter()
                .flatten();
            for triangle in triangles {
                painter.add(Shape::convex_polygon(
                    triangle.to_vec(),
                    palette.ahead,
                    Stroke::NONE,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::vec2;

    fn inside(rect: Rect, p: Pos2) -> bool {
        rect.expand(1e-3).contains(p)
    }

    #[test]
    fn stripes_cover_half_the_label() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(120.0, 24.0));
        let area = |polygon: &[Pos2]| {
            let n = polygon.len();
            (0..n)
                .map(|i| {
                    let (p, q) = (polygon[i], polygon[(i + 1) % n]);
                    p.x * q.y - q.x * p.y
                })
                .sum::<f32>()
                .abs()
                / 2.0
        };
        let stripes = polygons(rect, CornerRadius::ZERO, PERIOD);
        let covered: f32 = stripes.iter().map(|s| area(s)).sum();
        let whole = rect.area();
        assert!((covered / whole - 0.5).abs() < 0.03, "{covered} of {whole}");
        assert!(stripes.iter().flatten().all(|&p| inside(rect, p)));
    }

    #[test]
    fn stripes_keep_inside_the_rounded_corners() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 24.0));
        let r = 6.0;
        let stripes = polygons(rect, CornerRadius::same(6), PERIOD);
        assert!(!stripes.is_empty());
        // A rounded rectangle is its inner one grown by the radius.
        let inner = rect.shrink(r);
        for &p in stripes.iter().flatten() {
            let nearest = pos2(
                p.x.clamp(inner.left(), inner.right()),
                p.y.clamp(inner.top(), inner.bottom()),
            );
            assert!(p.distance(nearest) <= r + 1e-3, "{p:?} outside the corners");
        }
    }

    #[test]
    fn the_shade_is_a_fifth_darker() {
        let shade = shade(Color32::from_rgb(200, 100, 50));
        assert_eq!(shade, Color32::from_rgb(160, 80, 40));
    }

    #[test]
    fn a_badge_has_the_graphs_stripes_scaled_to_it() {
        assert_eq!(period_for(24.0), PERIOD);
        assert!((period_for(17.0) - 5.667).abs() < 0.01);
    }
}
