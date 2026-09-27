//! Pan and zoom: the mapping between world (layout) coordinates and the screen.

use eframe::egui::{Pos2, Rect, Vec2, pos2};

pub const MIN_ZOOM: f32 = 0.02;
pub const MAX_ZOOM: f32 = 4.0;
/// Panning and zooming keep at least this fraction of the canvas (along each axis) covered by
/// the graph, or all of the graph if it is smaller, so that it can't be lost off screen.
pub const KEEP_IN_SIGHT: f32 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// World coordinate shown at the top-left corner of the canvas.
    pub offset: Vec2,
    /// The graph's zoom, as shown: at 1.0 a world unit takes a point at 100% text size.
    pub zoom: f32,
    /// The text size (egui's zoom factor) the graph is drawn under. Points grow with it, so
    /// the graph is drawn that many times smaller in points and keeps its size on screen: the
    /// text size and the graph's zoom are separate.
    pub text_size: f32,
}

impl Default for View {
    fn default() -> Self {
        View {
            offset: Vec2::ZERO,
            zoom: 1.0,
            text_size: 1.0,
        }
    }
}

impl View {
    /// Points per world unit.
    pub fn scale(self) -> f32 {
        self.zoom / self.text_size
    }

    /// `len` points at 100% text size, in points now: a length on the graph (a line width, a
    /// margin) that keeps its size on screen whatever the text size.
    pub fn fixed(self, len: f32) -> f32 {
        len / self.text_size
    }

    pub fn to_screen(self, canvas: Rect, world: Pos2) -> Pos2 {
        canvas.min + (world.to_vec2() - self.offset) * self.scale()
    }

    pub fn to_world(self, canvas: Rect, screen: Pos2) -> Pos2 {
        pos2(0.0, 0.0) + self.offset + (screen - canvas.min) / self.scale()
    }

    pub fn rect_to_screen(self, canvas: Rect, world: Rect) -> Rect {
        Rect::from_min_max(
            self.to_screen(canvas, world.min),
            self.to_screen(canvas, world.max),
        )
    }

    /// The part of the world currently visible.
    pub fn visible_world(self, canvas: Rect) -> Rect {
        Rect::from_min_max(
            self.to_world(canvas, canvas.min),
            self.to_world(canvas, canvas.max),
        )
    }

    /// Multiplies the zoom by `factor`, keeping the world point under `anchor` fixed.
    pub fn zoom_around(&mut self, canvas: Rect, anchor: Pos2, factor: f32) {
        let world = self.to_world(canvas, anchor);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.offset = world.to_vec2() - (anchor - canvas.min) / self.scale();
    }

    pub fn pan_screen(&mut self, delta: Vec2) {
        self.offset -= delta / self.scale();
    }

    /// Places `world` at the given fraction of the canvas (0.5, 0.5 = centre).
    pub fn show_at(&mut self, canvas: Rect, world: Pos2, fraction: Vec2) {
        let screen_offset = canvas.size() * fraction;
        self.offset = world.to_vec2() - screen_offset / self.scale();
    }

    /// Pans back as far as needed to keep [`KEEP_IN_SIGHT`] of `world` (the graph) on the
    /// canvas, but never further than where the view was `before` panning or zooming: a view
    /// already further off (after the graph changed, say) can come back, but not drift further
    /// away, and doesn't jump.
    pub fn keep_in_sight(&mut self, canvas: Rect, world: Rect, before: View) {
        if !canvas.is_positive() || !world.is_finite() || world.is_negative() {
            return;
        }
        let axis = |i: usize, v: View| {
            let visible = canvas.size()[i] / v.scale();
            let keep = (KEEP_IN_SIGHT * visible).min(world.size()[i]).max(0.0);
            // Offsets that keep `keep` of the graph between the canvas edges.
            (world.min[i] + keep - visible, world.max[i] - keep)
        };
        for i in 0..2 {
            // How far off (in points) `before` was, which is allowed to stay.
            let (lo, hi) = axis(i, before);
            let off = before.offset[i];
            let slack = (lo - off).max(off - hi).max(0.0) * before.scale();
            let (lo, hi) = axis(i, *self);
            let slack = slack / self.scale();
            self.offset[i] = self.offset[i].clamp(lo - slack, hi + slack);
        }
    }

    /// Zooms and pans so `world` fills the canvas, never zooming in beyond `max_zoom`.
    pub fn fit(&mut self, canvas: Rect, world: Rect, max_zoom: f32) {
        let margin = 24.0;
        let avail = (canvas.size() - Vec2::splat(2.0 * margin)).max(Vec2::splat(1.0));
        let size = world.size().max(Vec2::splat(1.0));
        let scale = (avail.x / size.x).min(avail.y / size.y);
        self.zoom = (scale * self.text_size).clamp(MIN_ZOOM, max_zoom.min(MAX_ZOOM));
        self.show_at(canvas, world.center(), Vec2::splat(0.5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::vec2;

    fn canvas() -> Rect {
        Rect::from_min_size(pos2(10.0, 20.0), Vec2::new(800.0, 600.0))
    }

    #[test]
    fn screen_and_world_roundtrip() {
        let v = View {
            offset: Vec2::new(100.0, -50.0),
            zoom: 0.5,
            ..View::default()
        };
        let w = pos2(123.0, 456.0);
        let back = v.to_world(canvas(), v.to_screen(canvas(), w));
        assert!((back - w).length() < 1e-3);
    }

    #[test]
    fn zoom_keeps_anchor_fixed() {
        let mut v = View::default();
        let anchor = pos2(300.0, 200.0);
        let before = v.to_world(canvas(), anchor);
        v.zoom_around(canvas(), anchor, 1.7);
        assert!((v.to_world(canvas(), anchor) - before).length() < 1e-3);
    }

    #[test]
    fn fit_shows_everything() {
        let mut v = View::default();
        let world = Rect::from_min_max(pos2(-500.0, 0.0), pos2(2500.0, 9000.0));
        v.fit(canvas(), world, 1.0);
        assert!(canvas().contains_rect(v.rect_to_screen(canvas(), world)));
    }

    fn at_text_size(text_size: f32) -> View {
        View {
            offset: Vec2::new(100.0, -50.0),
            zoom: 0.8,
            text_size,
        }
    }

    #[test]
    fn larger_text_draws_the_graph_smaller_in_points() {
        // Points grow with the text size, so the graph keeps its size on screen.
        let (a, b) = (pos2(0.0, 0.0), pos2(300.0, 200.0));
        let length = |v: View| (v.to_screen(canvas(), b) - v.to_screen(canvas(), a)).length();
        let normal = length(at_text_size(1.0));
        assert!((length(at_text_size(1.5)) * 1.5 - normal).abs() < 1e-3);
        assert!((at_text_size(1.5).fixed(3.0) * 1.5 - 3.0).abs() < 1e-5);
        let v = at_text_size(1.5);
        let w = pos2(123.0, 456.0);
        assert!((v.to_world(canvas(), v.to_screen(canvas(), w)) - w).length() < 1e-3);
    }

    #[test]
    fn panning_follows_the_pointer_at_any_text_size() {
        let mut v = at_text_size(2.0);
        let w = pos2(10.0, 20.0);
        let before = v.to_screen(canvas(), w);
        v.pan_screen(Vec2::new(30.0, -40.0));
        assert!((v.to_screen(canvas(), w) - before - Vec2::new(30.0, -40.0)).length() < 1e-3);
    }

    #[test]
    fn zooming_keeps_the_anchor_at_any_text_size() {
        let mut v = at_text_size(1.5);
        let anchor = pos2(300.0, 200.0);
        let before = v.to_world(canvas(), anchor);
        v.zoom_around(canvas(), anchor, 1.7);
        assert!((v.to_world(canvas(), anchor) - before).length() < 1e-3);
        assert!((v.zoom - 0.8 * 1.7).abs() < 1e-5);
    }

    #[test]
    fn fitting_limits_the_zoom_as_shown() {
        // A tiny graph fits at 100% as shown, not at 100% in points.
        let mut v = at_text_size(2.0);
        let world = Rect::from_min_max(pos2(0.0, 0.0), pos2(50.0, 50.0));
        v.fit(canvas(), world, 1.0);
        assert_eq!(v.zoom, 1.0);
        let screen = v.rect_to_screen(canvas(), world);
        assert!((screen.width() - 25.0).abs() < 1e-3);

        // A big one still fits the canvas.
        let world = Rect::from_min_max(pos2(-500.0, 0.0), pos2(2500.0, 9000.0));
        v.fit(canvas(), world, 1.0);
        assert!(canvas().contains_rect(v.rect_to_screen(canvas(), world)));
    }

    fn graph() -> Rect {
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 2000.0))
    }

    /// The part of the graph on the canvas, in points along x and y.
    fn in_sight(v: View) -> Vec2 {
        let shown = v.rect_to_screen(canvas(), graph()).intersect(canvas());
        shown.size().max(Vec2::ZERO)
    }

    #[test]
    fn panning_stops_before_the_graph_is_lost() {
        let mut v = View::default();
        v.show_at(canvas(), graph().center(), Vec2::splat(0.5));
        for delta in [vec2(5000.0, 0.0), vec2(-5000.0, 0.0), vec2(0.0, 9000.0)] {
            let mut w = v;
            w.pan_screen(delta);
            w.keep_in_sight(canvas(), graph(), v);
            let seen = in_sight(w);
            assert!(
                seen.x >= 80.0 - 1e-3 && seen.y >= 60.0 - 1e-3,
                "{delta:?}: {seen:?}"
            );
            // Right at the limit, not held back before it.
            assert!(seen.x < 81.0 || seen.y < 61.0, "{delta:?}: {seen:?}");
        }
    }

    #[test]
    fn panning_within_the_limits_is_left_alone() {
        let mut v = View::default();
        v.show_at(canvas(), graph().center(), Vec2::splat(0.5));
        let mut w = v;
        w.pan_screen(vec2(300.0, -200.0));
        let wanted = w;
        w.keep_in_sight(canvas(), graph(), v);
        assert_eq!(w, wanted);
    }

    #[test]
    fn a_small_graph_stays_in_sight_whole() {
        let tiny = Rect::from_min_max(pos2(0.0, 0.0), pos2(20.0, 10.0));
        let mut v = View::default();
        v.show_at(canvas(), tiny.center(), Vec2::splat(0.5));
        let before = v;
        v.pan_screen(vec2(-2000.0, 2000.0));
        v.keep_in_sight(canvas(), tiny, before);
        assert!(canvas().contains_rect(v.rect_to_screen(canvas(), tiny)));
    }

    #[test]
    fn zooming_in_beside_the_graph_keeps_it_in_sight() {
        let mut v = View::default();
        v.show_at(canvas(), pos2(-300.0, 1000.0), Vec2::splat(0.5));
        let before = v;
        v.zoom_around(canvas(), canvas().center(), 3.0);
        v.keep_in_sight(canvas(), graph(), before);
        assert!(in_sight(v).x >= 80.0 - 1e-3);
    }

    #[test]
    fn a_view_already_off_the_graph_can_come_back_but_not_go_further() {
        let mut v = View::default();
        v.show_at(canvas(), pos2(-3000.0, 1000.0), Vec2::splat(0.5));
        let lost = v;
        // Further away: held where it was.
        v.pan_screen(vec2(500.0, 0.0));
        v.keep_in_sight(canvas(), graph(), lost);
        assert!((v.offset - lost.offset).length() < 1e-3);
        // Back towards the graph: free.
        v.pan_screen(vec2(-500.0, 0.0));
        let wanted = v;
        v.keep_in_sight(canvas(), graph(), lost);
        assert_eq!(v, wanted);
        // Untouched: stays.
        let mut w = lost;
        w.keep_in_sight(canvas(), graph(), lost);
        assert!((w.offset - lost.offset).length() < 1e-3);
    }

    #[test]
    fn nothing_to_keep_in_sight_before_there_is_a_canvas() {
        let mut v = View::default();
        v.keep_in_sight(Rect::NOTHING, graph(), View::default());
        v.keep_in_sight(canvas(), Rect::NOTHING, View::default());
        assert_eq!(v, View::default());
    }

    #[test]
    fn showing_a_point_puts_it_there_at_any_text_size() {
        let mut v = at_text_size(1.25);
        let w = pos2(700.0, -300.0);
        v.show_at(canvas(), w, Vec2::splat(0.5));
        assert!((v.to_screen(canvas(), w) - canvas().center()).length() < 1e-3);
    }
}
