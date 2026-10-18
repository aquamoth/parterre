//! Rasterises egui's meshes on the CPU, so that a PNG export is drawn by the window's painting
//! code without a GPU or a window. It does what egui's glow painter does: per-vertex colours
//! times the texture (the font atlas), premultiplied alpha blended in gamma space, with
//! anti-aliasing from the tessellator's feathering. The target is the opaque background, so it
//! keeps RGB only.

use eframe::egui::epaint::{ImageData, ImageDelta, Mesh, Vertex};
use eframe::egui::{Color32, Pos2};
use image::RgbImage;

#[derive(Default)]
pub struct Texture {
    size: [usize; 2],
    pixels: Vec<Color32>,
}

impl Texture {
    pub fn apply(&mut self, delta: &ImageDelta) {
        let ImageData::Color(image) = &delta.image;
        match delta.pos {
            None => {
                self.size = image.size;
                self.pixels = image.pixels.clone();
            }
            Some([x0, y0]) => {
                let [w, h] = image.size;
                for y in 0..h {
                    let at = (y0 + y) * self.size[0] + x0;
                    if let Some(row) = self.pixels.get_mut(at..at + w) {
                        row.copy_from_slice(&image.pixels[y * w..(y + 1) * w]);
                    }
                }
            }
        }
    }

    /// Bilinear, as egui samples its font atlas, and clamped to the edges.
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let [w, h] = self.size;
        if w == 0 || h == 0 {
            return [255.0; 4];
        }
        let x = u * w as f32 - 0.5;
        let y = v * h as f32 - 0.5;
        let (fx, fy) = (x.floor(), y.floor());
        let (tx, ty) = (x - fx, y - fy);
        let clamp = |i: f32, n: usize| (i.max(0.0) as usize).min(n - 1);
        let (x0, x1) = (clamp(fx, w), clamp(fx + 1.0, w));
        let (y0, y1) = (clamp(fy, h), clamp(fy + 1.0, h));
        let px = |x: usize, y: usize| self.pixels[y * w + x].to_array().map(f32::from);
        let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bottom = c[i] + (d[i] - c[i]) * tx;
            top + (bottom - top) * ty
        })
    }
}

/// `min` inclusive, `max` exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub min: [i64; 2],
    pub max: [i64; 2],
}

impl PixelRect {
    pub fn intersect(self, other: PixelRect) -> PixelRect {
        PixelRect {
            min: [self.min[0].max(other.min[0]), self.min[1].max(other.min[1])],
            max: [self.max[0].min(other.max[0]), self.max[1].min(other.max[1])],
        }
    }
}

/// Point (0, 0) of `mesh` lands on the pixel `origin` of `target`, and only pixels inside `clip`
/// (relative to `origin`) are touched.
pub fn draw_mesh(
    target: &mut RgbImage,
    origin: [i64; 2],
    clip: PixelRect,
    pixels_per_point: f32,
    mesh: &Mesh,
    texture: &Texture,
) {
    let bounds = PixelRect {
        min: [-origin[0], -origin[1]],
        max: [
            i64::from(target.width()) - origin[0],
            i64::from(target.height()) - origin[1],
        ],
    };
    let clip = clip.intersect(bounds);
    if clip.min[0] >= clip.max[0] || clip.min[1] >= clip.max[1] {
        return;
    }
    for triangle in mesh.indices.as_chunks::<3>().0 {
        let vertices = triangle.map(|i| &mesh.vertices[i as usize]);
        fill_triangle(target, origin, clip, pixels_per_point, vertices, texture);
    }
}

/// Fills the pixels whose centres lie inside the triangle, half-open on its right and bottom
/// edges so that triangles sharing an edge never both cover a pixel (which would show as a seam
/// in translucent shapes).
fn fill_triangle(
    target: &mut RgbImage,
    origin: [i64; 2],
    clip: PixelRect,
    pixels_per_point: f32,
    v: [&Vertex; 3],
    texture: &Texture,
) {
    let p = v.map(|v| (v.pos.to_vec2() * pixels_per_point).to_pos2());
    let cross = |a: Pos2, b: Pos2, c: Pos2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let area = cross(p[0], p[1], p[2]);
    if area == 0.0 || !area.is_finite() {
        return;
    }
    let y_min = p.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
    let y_max = p.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
    let first = ((y_min - 0.5).ceil() as i64).max(clip.min[1]);
    let end = ((y_max - 0.5).ceil() as i64).min(clip.max[1]);
    // Each edge with its ends ordered by y, so that two triangles sharing it compute exactly
    // the same crossings.
    let edges = [(p[0], p[1]), (p[1], p[2]), (p[2], p[0])]
        .map(|(a, b)| if a.y <= b.y { (a, b) } else { (b, a) });
    let colors = v.map(|v| v.color.to_array().map(f32::from));
    let solid = v[0].uv == v[1].uv && v[1].uv == v[2].uv;
    let solid_texel = texture.sample(v[0].uv.x, v[0].uv.y);

    for y in first..end {
        let yc = y as f32 + 0.5;
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for (a, b) in edges {
            if a.y <= yc && yc < b.y {
                let x = a.x + (yc - a.y) * (b.x - a.x) / (b.y - a.y);
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
        if lo >= hi {
            continue;
        }
        let x_first = ((lo - 0.5).ceil() as i64).max(clip.min[0]);
        let x_end = ((hi - 0.5).ceil() as i64).min(clip.max[0]);
        for x in x_first..x_end {
            let pc = Pos2::new(x as f32 + 0.5, yc);
            let l1 = cross(p[0], pc, p[2]) / area;
            let l2 = cross(p[0], p[1], pc) / area;
            let lerp = |a: f32, b: f32, c: f32| a + l1 * (b - a) + l2 * (c - a);
            let texel = if solid {
                solid_texel
            } else {
                texture.sample(
                    lerp(v[0].uv.x, v[1].uv.x, v[2].uv.x),
                    lerp(v[0].uv.y, v[1].uv.y, v[2].uv.y),
                )
            };
            let src: [f32; 4] = std::array::from_fn(|i| {
                lerp(colors[0][i], colors[1][i], colors[2][i]) * texel[i] / 255.0
            });
            let keep = 1.0 - src[3] / 255.0;
            let px = target.get_pixel_mut((x + origin[0]) as u32, (y + origin[1]) as u32);
            for (dst, s) in px.0.iter_mut().zip(src) {
                *dst = (s + f32::from(*dst) * keep).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::epaint::{Rect, TextureId, WHITE_UV};
    use image::Rgb;

    fn white_texture() -> Texture {
        Texture {
            size: [1, 1],
            pixels: vec![Color32::WHITE],
        }
    }

    fn quad(rect: Rect, color: Color32) -> Mesh {
        let mut mesh = Mesh::with_texture(TextureId::default());
        mesh.add_rect_with_uv(rect, Rect::from_min_max(WHITE_UV, WHITE_UV), color);
        mesh
    }

    fn everything() -> PixelRect {
        PixelRect {
            min: [i64::MIN / 2; 2],
            max: [i64::MAX / 2; 2],
        }
    }

    #[test]
    fn fills_exactly_the_covered_pixels() {
        let mut img = RgbImage::from_pixel(6, 6, Rgb([0, 0, 0]));
        let rect = Rect::from_min_max(Pos2::new(1.0, 2.0), Pos2::new(4.0, 5.0));
        let mesh = quad(rect, Color32::from_rgb(200, 100, 50));
        draw_mesh(&mut img, [0, 0], everything(), 1.0, &mesh, &white_texture());
        for (x, y, px) in img.enumerate_pixels() {
            let inside = (1..4).contains(&x) && (2..5).contains(&y);
            let want = if inside { [200, 100, 50] } else { [0, 0, 0] };
            assert_eq!(px.0, want, "pixel {x},{y}");
        }
    }

    #[test]
    fn shared_edges_are_drawn_once() {
        // A translucent quad split along its diagonal: a pixel covered twice would be darker.
        let mut img = RgbImage::from_pixel(8, 8, Rgb([255, 255, 255]));
        let rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(8.0, 8.0));
        let mesh = quad(rect, Color32::from_black_alpha(128));
        draw_mesh(&mut img, [0, 0], everything(), 1.0, &mesh, &white_texture());
        let first = img.get_pixel(0, 0).0;
        assert!(img.pixels().all(|p| p.0 == first), "uneven coverage");
        assert_eq!(first, [127, 127, 127]);
    }

    #[test]
    fn respects_clip_origin_and_scale() {
        let mut img = RgbImage::from_pixel(10, 10, Rgb([0, 0, 0]));
        let rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(5.0, 5.0));
        let mesh = quad(rect, Color32::WHITE);
        // 2 pixels per point: the quad covers 10 × 10 pixels from (2, 3), clipped to 4 × 4.
        let clip = PixelRect {
            min: [0, 0],
            max: [4, 4],
        };
        draw_mesh(&mut img, [2, 3], clip, 2.0, &mesh, &white_texture());
        let lit = img.pixels().filter(|p| p.0 == [255; 3]).count();
        assert_eq!(lit, 16);
        assert_eq!(img.get_pixel(2, 3).0, [255; 3]);
        assert_eq!(img.get_pixel(5, 6).0, [255; 3]);
        assert_eq!(img.get_pixel(6, 6).0, [0; 3]);
    }

    #[test]
    fn applies_partial_texture_updates() {
        let mut tex = Texture::default();
        tex.apply(&ImageDelta::full(
            eframe::egui::ColorImage::filled([4, 4], Color32::TRANSPARENT),
            Default::default(),
        ));
        tex.apply(&ImageDelta::partial(
            [1, 2],
            eframe::egui::ColorImage::filled([2, 1], Color32::RED),
            Default::default(),
        ));
        assert_eq!(tex.pixels[2 * 4 + 1], Color32::RED);
        assert_eq!(tex.pixels[2 * 4 + 2], Color32::RED);
        assert_eq!(tex.pixels[2 * 4 + 3], Color32::TRANSPARENT);
        // The centre of a texel samples exactly that texel.
        assert_eq!(tex.sample(1.5 / 4.0, 2.5 / 4.0), [255.0, 0.0, 0.0, 255.0]);
    }
}
