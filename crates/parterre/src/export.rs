use std::fmt::Write as _;
use std::path::Path;

use eframe::egui::epaint::Primitive;
use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, TextureId, Vec2, ViewportId, vec2};
use parterre_core::glyphs::{self, Glyph, Part};

use crate::raster::{self, PixelRect};
use crate::render::{
    ARROW_LEN, Marks, arrowhead_points, edge_path, node_rows, paint_scene, pull_request_label,
    row_colors, worktree_glyph, worktree_label,
};
use crate::scene::{CORNER_RADIUS, FONT_SIZE, MARGIN_X, RowKind, Scene};
use crate::settings::Settings;
use crate::theme::Palette;
use crate::view::{MAX_ZOOM, MIN_ZOOM, View};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Svg,
    Png,
    WebP,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Svg, Format::Png, Format::WebP];

    /// No extension means SVG, as before PNG existed.
    pub fn from_path(path: &Path) -> Option<Format> {
        let Some(ext) = path.extension() else {
            return Some(Format::Svg);
        };
        Format::ALL
            .into_iter()
            .find(|f| ext.eq_ignore_ascii_case(f.extension()))
    }

    pub fn extension(self) -> &'static str {
        match self {
            Format::Svg => "svg",
            Format::Png => "png",
            Format::WebP => "webp",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Format::Svg => "SVG",
            Format::Png => "PNG",
            Format::WebP => "WebP",
        }
    }

    /// Many PNG decoders stop at 16 bits, and WebP can't hold more than 14.
    fn max_side_px(self) -> f64 {
        match self {
            Format::Svg | Format::Png => 65_535.0,
            Format::WebP => 16_383.0,
        }
    }
}

/// Returns what was written, for the status line. `zoom` and `pixels_per_point` only matter for
/// PNG and WebP.
pub fn write(
    path: &Path,
    format: Format,
    scene: &Scene,
    settings: &Settings,
    palette: &Palette,
    zoom: f32,
    pixels_per_point: f32,
) -> anyhow::Result<String> {
    let encoding = match format {
        Format::Svg => {
            std::fs::write(path, to_svg(scene, settings, palette))?;
            return Ok("SVG at 100%".into());
        }
        Format::Png => image::ImageFormat::Png,
        Format::WebP => image::ImageFormat::WebP,
    };
    let size = raster_size(scene, format, zoom, pixels_per_point);
    to_image(scene, settings, palette, &size).save_with_format(path, encoding)?;
    Ok(format!("{} {}", format.name(), size.describe()))
}

const GRAPH_MARGIN: f32 = 20.0;
const EDGE_WIDTH: f32 = 2.0;

fn export_bounds(scene: &Scene) -> Rect {
    scene.bounds().expand(GRAPH_MARGIN)
}

fn svg_points(points: &[Pos2]) -> String {
    let points: Vec<String> = points
        .iter()
        .map(|p| format!("{:.1},{:.1}", p.x, p.y))
        .collect();
    points.join(" ")
}

fn to_svg(scene: &Scene, settings: &Settings, palette: &Palette) -> String {
    let bounds = export_bounds(scene);
    let origin = bounds.min.to_vec2();
    let map = |p: Pos2| p - origin;
    let mut svg = String::new();
    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.0} {h:.0}">"#,
        w = bounds.width(),
        h = bounds.height()
    );
    let _ = writeln!(
        svg,
        r#"<rect width="100%" height="100%" fill="{}"/>"#,
        hex(palette.background)
    );

    let _ = writeln!(
        svg,
        r#"<g fill="none" stroke="{}" stroke-width="{EDGE_WIDTH}" stroke-linejoin="round" stroke-linecap="round">"#,
        hex(palette.edge)
    );
    let mut heads = Vec::new();
    for e in 0..scene.graph.edges.len() {
        let Some(path) = edge_path(scene, e, settings.edge_style, map, None) else {
            continue;
        };
        let _ = writeln!(svg, r#"<polyline points="{}"/>"#, svg_points(&path));
        if let Some(head) = arrowhead_points(&path, settings.arrows, ARROW_LEN) {
            heads.extend(head);
        }
    }
    svg.push_str("</g>\n");
    let _ = writeln!(svg, r#"<g fill="{}">"#, hex(palette.edge));
    for triangle in heads {
        let _ = writeln!(svg, r#"<polygon points="{}"/>"#, svg_points(&triangle));
    }
    svg.push_str("</g>\n");

    let _ = writeln!(
        svg,
        r#"<g font-family="Consolas, 'DejaVu Sans Mono', Menlo, monospace" font-size="{FONT_SIZE}">"#
    );
    let radius = CORNER_RADIUS as u8;
    for (i, visual) in scene.visuals.iter().enumerate() {
        let r = scene.node_rect(i);
        let rect = Rect::from_min_max(map(r.min), map(r.max));
        for ((row_rect, corners), row) in
            node_rows(rect, visual.rows.len(), scene.row_height, radius).zip(&visual.rows)
        {
            let (fill, border, text) = row_colors(row, palette);
            let _ = writeln!(
                svg,
                r#"<path d="{}" fill="{}" stroke="{}"/>"#,
                rounded_rect(row_rect, corners),
                hex(fill),
                hex(border)
            );
            let (x, anchor) = match row.kind {
                RowKind::PullRequest { .. } => {
                    let (end, icon) = pull_request_label(row_rect, row.width, 1.0);
                    svg.push_str(&svg_glyph(glyphs::PULL_REQUEST, icon, text));
                    (end.x, "end")
                }
                _ => match row.worktree() {
                    Some(c) => {
                        let (start, icon) = worktree_label(row_rect, 1.0);
                        svg.push_str(&svg_glyph(worktree_glyph(c), icon, text));
                        (start.x, "start")
                    }
                    None => (row_rect.min.x + MARGIN_X, "start"),
                },
            };
            let style = if row.is_detached_worktree() {
                r#" font-style="italic""#
            } else {
                ""
            };
            let _ = writeln!(
                svg,
                r#"<text x="{:.1}" y="{:.1}" text-anchor="{anchor}" dominant-baseline="central" fill="{}"{style}>{}</text>"#,
                x,
                row_rect.center().y,
                hex(text),
                escape(&row.label)
            );
        }
    }
    svg.push_str("</g>\n</svg>\n");
    svg
}

/// The whole image is held in memory for encoding: 100 megapixels take 300 MB.
const MAX_PIXELS: f64 = 100e6;

#[derive(Clone, Copy, Debug, PartialEq)]
struct RasterSize {
    width: u32,
    height: u32,
    zoom: f32,
    pixels_per_point: f32,
    zoom_reduced: bool,
    max_side_px: f64,
}

impl RasterSize {
    fn describe(&self) -> String {
        let mut s = format!(
            "{} × {} px at {:.0}%",
            self.width,
            self.height,
            self.zoom * 100.0
        );
        if self.zoom_reduced {
            let _ = write!(
                s,
                ", scaled down to stay within {:.0} megapixels and {:.0} px a side",
                MAX_PIXELS / 1e6,
                self.max_side_px
            );
        }
        s
    }
}

/// Deliberately unlike TortoiseGit, which draws raster exports at the current zoom with no
/// limit and shows "not enough memory" when Windows cannot allocate the bitmap: parterre
/// lowers the zoom instead, so that an export always gives a picture.
fn raster_size(scene: &Scene, format: Format, zoom: f32, pixels_per_point: f32) -> RasterSize {
    fit(
        export_bounds(scene).size(),
        zoom,
        pixels_per_point,
        format.max_side_px(),
    )
}

fn fit(world: Vec2, zoom: f32, pixels_per_point: f32, max_side_px: f64) -> RasterSize {
    let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    let scale = f64::from(zoom * pixels_per_point);
    let (w, h) = (
        f64::from(world.x.max(1.0)) * scale,
        f64::from(world.y.max(1.0)) * scale,
    );
    // The largest `fit` with (w·fit + 1)(h·fit + 1) ≤ MAX_PIXELS, leaving a pixel a side for
    // rounding up.
    let (a, b, c) = (w * h, w + h, 1.0 - MAX_PIXELS);
    let fit = ((-b + (b * b - 4.0 * a * c).sqrt()) / (2.0 * a))
        .min((max_side_px - 1.0) / w)
        .min((max_side_px - 1.0) / h);
    let (scale, zoom_reduced) = if fit < 1.0 {
        (scale * fit, true)
    } else {
        (scale, false)
    };
    RasterSize {
        width: (f64::from(world.x) * scale).ceil().max(1.0) as u32,
        height: (f64::from(world.y) * scale).ceil().max(1.0) as u32,
        zoom: (scale / f64::from(pixels_per_point)) as f32,
        pixels_per_point,
        zoom_reduced,
        max_side_px,
    }
}

/// Each tile is one egui pass, so only the shapes near it are tessellated at a time.
const TILE_SIDE_PX: u32 = 2048;

fn to_image(
    scene: &Scene,
    settings: &Settings,
    palette: &Palette,
    size: &RasterSize,
) -> image::RgbImage {
    let [r, g, b, _] = palette.background.to_array();
    let mut img = image::RgbImage::from_pixel(size.width, size.height, image::Rgb([r, g, b]));
    let ppp = size.pixels_per_point;
    let world = export_bounds(scene).min.to_vec2();
    // A context of its own, so that the fonts are rasterised for this scale and the window's
    // context is left alone. The theme decides how glyph coverage becomes alpha.
    let ctx = egui::Context::default();
    ctx.set_theme(if palette.is_dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    let mut atlas = raster::Texture::default();
    let marks = Marks::default();
    for ty in (0..size.height).step_by(TILE_SIDE_PX as usize) {
        for tx in (0..size.width).step_by(TILE_SIDE_PX as usize) {
            let tw = TILE_SIDE_PX.min(size.width - tx);
            let th = TILE_SIDE_PX.min(size.height - ty);
            let canvas = Rect::from_min_size(Pos2::ZERO, vec2(tw as f32, th as f32) / ppp);
            let view = View {
                offset: world + vec2(tx as f32, ty as f32) / (ppp * size.zoom),
                zoom: size.zoom,
                // Its own context, at the zoom factor egui starts with.
                text_size: 1.0,
            };
            let mut input = egui::RawInput {
                screen_rect: Some(canvas),
                ..Default::default()
            };
            input
                .viewports
                .entry(ViewportId::ROOT)
                .or_default()
                .native_pixels_per_point = Some(ppp);
            let mut output = ctx.run_ui(input, |ui| {
                paint_scene(
                    ui.painter(),
                    canvas,
                    &view,
                    scene,
                    palette,
                    settings,
                    &marks,
                );
            });
            for (id, deltas) in &output.textures_delta.set {
                if *id == TextureId::default() {
                    deltas.iter().for_each(|d| atlas.apply(d));
                }
            }
            output.textures_delta.clear();
            let tile = PixelRect {
                min: [0, 0],
                max: [i64::from(tw), i64::from(th)],
            };
            let origin = [i64::from(tx), i64::from(ty)];
            for clipped in ctx.tessellate(output.shapes, output.pixels_per_point) {
                let Primitive::Mesh(mesh) = &clipped.primitive else {
                    continue;
                };
                // Rounded as egui's glow painter rounds its scissor rectangle.
                let to_px = |points: f32| (points * ppp).round() as i64;
                let c = clipped.clip_rect;
                let clip = PixelRect {
                    min: [to_px(c.min.x), to_px(c.min.y)],
                    max: [to_px(c.max.x), to_px(c.max.y)],
                };
                raster::draw_mesh(&mut img, origin, tile.intersect(clip), ppp, mesh, &atlas);
            }
        }
    }
    img
}

/// Inset by half a pixel, so that the 1 px border stays inside, as on screen.
fn rounded_rect(r: Rect, c: CornerRadius) -> String {
    let r = r.shrink(0.5);
    let (x0, y0, x1, y1) = (r.min.x, r.min.y, r.max.x, r.max.y);
    let (nw, ne, se, sw) = (c.nw as f32, c.ne as f32, c.se as f32, c.sw as f32);
    format!(
        "M{:.1},{y0:.1} H{:.1} A{ne},{ne} 0 0 1 {x1:.1},{:.1} V{:.1} A{se},{se} 0 0 1 {:.1},{y1:.1} H{:.1} A{sw},{sw} 0 0 1 {x0:.1},{:.1} V{:.1} A{nw},{nw} 0 0 1 {:.1},{y0:.1} Z",
        x0 + nw,
        x1 - ne,
        y0 + ne,
        y1 - se,
        x1 - se,
        x0 + sw,
        y1 - sw,
        y0 + nw,
        x0 + nw,
    )
}

/// A glyph's paths are SVG path data already.
fn svg_glyph(glyph: Glyph, rect: Rect, color: Color32) -> String {
    let color = hex(color);
    let mut g = format!(
        r#"<g transform="translate({:.1} {:.1}) scale({:.3})" fill="none" stroke="{color}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round">"#,
        rect.min.x,
        rect.min.y,
        rect.width() / glyphs::SIZE,
        glyphs::STROKE,
    );
    for part in glyph {
        let _ = match *part {
            Part::Path(d) => write!(g, r#"<path d="{d}"/>"#),
            Part::Circle {
                center: [x, y],
                radius,
                filled,
            } => {
                let fill = if filled { color.as_str() } else { "none" };
                write!(
                    g,
                    r#"<circle cx="{x}" cy="{y}" r="{radius}" fill="{fill}"/>"#
                )
            }
            Part::Rect {
                min: [x, y],
                size: [w, h],
                radius,
                filled,
            } => {
                let fill = if filled { color.as_str() } else { "none" };
                write!(
                    g,
                    r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{radius}" fill="{fill}"/>"#
                )
            }
        };
    }
    g.push_str("</g>\n");
    g
}

fn hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_markup() {
        assert_eq!(escape("a<b>&\"c\""), "a&lt;b&gt;&amp;&quot;c&quot;");
    }

    #[test]
    fn glyphs_are_svg_groups() {
        let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::splat(12.0));
        let g = svg_glyph(glyphs::PULL_REQUEST, rect, Color32::BLACK);
        assert!(g.starts_with(r#"<g transform="translate(10.0 20.0) scale(0.500)""#));
        assert!(g.contains(r#"<circle cx="5" cy="6" r="3" fill="none"/>"#));
        assert!(g.contains(r#"<path d="M5 9v12"/>"#));
        assert!(g.ends_with("</g>\n"));
    }

    #[test]
    fn format_follows_the_extension() {
        let f = |p: &str| Format::from_path(Path::new(p));
        assert_eq!(f("graph.svg"), Some(Format::Svg));
        assert_eq!(f("graph.SVG"), Some(Format::Svg));
        assert_eq!(f("graph"), Some(Format::Svg));
        assert_eq!(f("dir.d/graph.Png"), Some(Format::Png));
        assert_eq!(f("graph.WebP"), Some(Format::WebP));
        assert_eq!(f("graph.jpg"), None);
    }

    #[test]
    fn png_keeps_the_zoom_when_it_fits() {
        let s = fit(vec2(3000.0, 4000.0), 1.0, 2.0, 65_535.0);
        assert_eq!(
            (s.width, s.height, s.zoom, s.zoom_reduced),
            (6000, 8000, 1.0, false)
        );
    }

    #[test]
    fn png_scales_down_to_the_pixel_limit() {
        let s = fit(vec2(24_000.0, 8_000.0), 1.0, 1.0, 65_535.0);
        assert!(s.zoom_reduced);
        let pixels = f64::from(s.width) * f64::from(s.height);
        assert!(pixels <= MAX_PIXELS, "{pixels}");
        assert!(pixels > 0.99 * MAX_PIXELS, "{pixels}");
    }

    #[test]
    fn png_scales_down_to_the_side_limit() {
        // A tall all-commits view: few pixels, but too tall.
        let s = fit(vec2(600.0, 640_000.0), 1.0, 1.0, 65_535.0);
        assert!(s.zoom_reduced);
        assert!(f64::from(s.height) <= 65_535.0);
        assert!(f64::from(s.height) > 65_535.0 - 2.0);
        // WebP allows less: Apps in the Classic look, 24,232 px wide at 100%.
        let s = fit(
            vec2(24_232.0, 3_745.0),
            1.0,
            1.0,
            Format::WebP.max_side_px(),
        );
        assert!(s.zoom_reduced);
        assert!(f64::from(s.width) <= 16_383.0);
    }

    fn chain_of_three_commits() -> Scene {
        use parterre_core::repo::{Commit, CommitIx, Head};
        let commits = (0..3u32)
            .map(|i| Commit {
                oid: parterre_core::Oid::from_hex(&format!("{:040x}", 0xabc0 + i)).unwrap(),
                parents: if i < 2 { vec![CommitIx(i + 1)] } else { vec![] },
                truncated: false,
                empty_tree: false,
                author_name: String::new(),
                author_email: String::new(),
                author_time: 10 - i64::from(i),
                author_date: String::new(),
                commit_time: 10 - i64::from(i),
                subject: String::new(),
            })
            .collect();
        let repo = parterre_core::Repo::new(
            "/x".into(),
            commits,
            Vec::new(),
            Head::Detached(CommitIx(0)),
        );
        let mut settings = Settings::default();
        settings.graph.simplification = parterre_core::revgraph::Simplification::AllCommits;
        Scene::headless(&std::sync::Arc::new(repo), &settings)
    }

    #[test]
    fn png_draws_the_boxes_and_their_text() {
        let scene = chain_of_three_commits();
        assert_eq!(scene.node_count(), 3);
        let palette = Palette::light();
        let size = raster_size(&scene, Format::Png, 1.0, 1.0);
        let img = to_image(&scene, &Settings::default(), &palette, &size);
        assert_eq!((img.width(), img.height()), (size.width, size.height));
        let rgb = |c: Color32| [c.r(), c.g(), c.b()];
        assert_eq!(img.get_pixel(0, 0).0, rgb(palette.background));
        let count = |c: [u8; 3]| img.pixels().filter(|p| p.0 == c).count();
        // Box fills, and dark text inside them.
        assert!(count(rgb(palette.plain_fill)) > 3 * 500);
        assert!(img.pixels().filter(|p| p.0.iter().all(|&v| v < 80)).count() > 100);

        // Twice the zoom, twice the size, and fills that scale with the area.
        let big = raster_size(&scene, Format::Png, 2.0, 1.0);
        assert!(big.width.abs_diff(2 * size.width) <= 1);
        let img2 = to_image(&scene, &Settings::default(), &palette, &big);
        let fills2 = img2
            .pixels()
            .filter(|p| p.0 == rgb(palette.plain_fill))
            .count();
        let ratio = fills2 as f32 / count(rgb(palette.plain_fill)) as f32;
        assert!((3.0..5.0).contains(&ratio), "{ratio}");
    }

    #[test]
    fn colours_are_hex() {
        assert_eq!(hex(Color32::from_rgb(255, 221, 170)), "#ffddaa");
    }
}
