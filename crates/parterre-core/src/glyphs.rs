//! The icons of the toolbar and menus: outlines in a 24 × 24 box, stroked [`STROKE`] wide, in
//! the style of Lucide (where the generic ones come from; the graph and drag-mode ones are drawn
//! here). Paths are SVG path data, so icons can be copied from an SVG editor; [`flatten`] turns
//! them into polylines for painting.
//!
//! Chosen on 2026-09-26 with the toolbar; the prototype is on the branch `prototype/menus`
//! (`crates/parterre/prototype-menus/index.html`).

/// The side of the box the icons are drawn in.
pub const SIZE: f32 = 24.0;
/// Line width, in the units of the box.
pub const STROKE: f32 = 1.8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Part {
    /// SVG path data, stroked.
    Path(&'static str),
    Circle {
        center: [f32; 2],
        radius: f32,
        filled: bool,
    },
    Rect {
        min: [f32; 2],
        size: [f32; 2],
        radius: f32,
        filled: bool,
    },
}

pub type Glyph = &'static [Part];

const fn ring(x: f32, y: f32, radius: f32) -> Part {
    Part::Circle {
        center: [x, y],
        radius,
        filled: false,
    }
}

const fn dot(x: f32, y: f32, radius: f32) -> Part {
    Part::Circle {
        center: [x, y],
        radius,
        filled: true,
    }
}

const fn rect(x: f32, y: f32, w: f32, h: f32, radius: f32, filled: bool) -> Part {
    Part::Rect {
        min: [x, y],
        size: [w, h],
        radius,
        filled,
    }
}

pub const MENU: Glyph = &[Part::Path("M4 7h16M4 12h16M4 17h16")];
pub const SEARCH: Glyph = &[ring(11.0, 11.0, 6.5), Part::Path("M16 16l4.5 4.5")];
pub const PLUS: Glyph = &[Part::Path("M12 5v14M5 12h14")];
pub const MINUS: Glyph = &[Part::Path("M5 12h14")];
pub const CLOSE: Glyph = &[Part::Path("M18 6 6 18M6 6l12 12")];
pub const CHECK: Glyph = &[Part::Path("M5 12.5l4.5 4.5L19 7.5")];
pub const CHEVRON_DOWN: Glyph = &[Part::Path("M6 9l6 6 6-6")];
pub const CHEVRON_UP: Glyph = &[Part::Path("M6 15l6-6 6 6")];
pub const CHEVRON_RIGHT: Glyph = &[Part::Path("M9 6l6 6-6 6")];
pub const ZOOM: Glyph = &[
    ring(11.0, 11.0, 6.5),
    Part::Path("M16 16l4.5 4.5M8.3 11h5.4M11 8.3v5.4"),
];
/// Go to HEAD: a crosshair.
pub const HEAD: Glyph = &[
    ring(12.0, 12.0, 5.0),
    dot(12.0, 12.0, 1.6),
    Part::Path("M12 2.5v4M12 17.5v4M2.5 12h4M17.5 12h4"),
];
/// The overview map: a small picture in the corner of a big one.
pub const OVERVIEW: Glyph = &[
    rect(3.0, 4.0, 18.0, 16.0, 2.0, false),
    rect(12.0, 11.5, 6.0, 5.5, 1.0, true),
];
/// Local branches: Lucide's git-branch.
pub const LOCAL: Glyph = &[
    Part::Path("M6 3.5v11"),
    ring(18.0, 6.0, 2.5),
    ring(6.0, 17.5, 2.5),
    Part::Path("M18 8.5a9 9 0 0 1-9 9"),
];
/// Remote branches: a cloud.
pub const REMOTE: Glyph = &[Part::Path(
    "M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z",
)];
pub const TAGS: Glyph = &[
    Part::Path(
        "M12.6 2.6A2 2 0 0 0 11.2 2H4a2 2 0 0 0-2 2v7.2a2 2 0 0 0 .6 1.4l8.7 8.7a2.4 2.4 0 0 0 \
         3.4 0l6.6-6.6a2.4 2.4 0 0 0 0-3.4z",
    ),
    dot(7.5, 7.5, 1.3),
];
/// Labelled commits: two commits with labels beside them.
pub const LABELLED: Glyph = &[
    Part::Path("M6.5 7v10"),
    dot(6.5, 5.5, 2.2),
    dot(6.5, 18.5, 2.2),
    rect(11.0, 3.5, 10.0, 4.0, 2.0, false),
    rect(11.0, 16.5, 7.0, 4.0, 2.0, false),
];
/// Branchings and merges: a fork point and the merge that joins its two sides.
pub const BRANCHINGS: Glyph = &[
    dot(12.0, 4.5, 2.2),
    dot(12.0, 19.5, 2.2),
    Part::Path("M12 6.7c0 3-6 3-6 5.3s6 2.3 6 5.3M12 6.7c0 3 6 3 6 5.3s-6 2.3-6 5.3"),
];
/// All commits: a chain of them, with a side branch.
pub const ALL_COMMITS: Glyph = &[
    Part::Path("M12 4v16"),
    dot(12.0, 4.0, 1.9),
    dot(12.0, 9.3, 1.9),
    dot(12.0, 14.7, 1.9),
    dot(12.0, 20.0, 1.9),
    Part::Path("M12 7c4 0 5.5 1 5.5 4.5S16 17.5 12 17.5"),
    dot(17.5, 12.0, 1.9),
];
/// Drag mode Adapt: the dragged node pulls its neighbours along on springs.
pub const ADAPT: Glyph = &[
    dot(12.0, 12.0, 3.0),
    ring(4.5, 5.0, 2.0),
    ring(19.5, 19.0, 2.0),
    Part::Path("M6 6.4l1.6.2-.4 1.6 1.6.2-.4 1.6 1.4.3M18 17.6l-1.6-.2.4-1.6-1.6-.2.4-1.6-1.4-.3"),
];
/// Drag mode Free: only this node moves.
pub const FREE: Glyph = &[
    dot(12.0, 12.0, 2.8),
    Part::Path(
        "M12 2.5v5M12 16.5v5M2.5 12h5M16.5 12h5M9.5 5 12 2.5 14.5 5M9.5 19l2.5 2.5 2.5-2.5\
         M5 9.5 2.5 12 5 14.5M19 9.5l2.5 2.5-2.5 2.5",
    ),
];
/// Drag mode Subtree: the node and what grows out of it move together.
pub const SUBTREE: Glyph = &[
    dot(12.0, 4.5, 2.3),
    dot(6.0, 12.5, 2.3),
    dot(18.0, 12.5, 2.3),
    dot(6.0, 20.0, 2.3),
    Part::Path("M12 4.5 6 12.5M12 4.5l6 8M6 12.5V20"),
];

/// Log layout A, stacked: three panes one above the other.
pub const LAYOUT_STACKED: Glyph = &[
    rect(3.0, 4.0, 18.0, 16.0, 2.0, false),
    Part::Path("M3 10.5h18M3 15.5h18"),
];
/// Log layout B, side by side: a pane on the left, two above each other on the right.
pub const LAYOUT_SIDE_BY_SIDE: Glyph = &[
    rect(3.0, 4.0, 18.0, 16.0, 2.0, false),
    Part::Path("M13 4v16M13 11h8"),
];
/// Log layout C, details and files below: a pane on top, two side by side under it.
pub const LAYOUT_DETAILS_BELOW: Glyph = &[
    rect(3.0, 4.0, 18.0, 16.0, 2.0, false),
    Part::Path("M3 12h18M10.5 12v8"),
];
/// Log layout D, files on the right: two panes above each other, a tall one on the right.
pub const LAYOUT_FILES_RIGHT: Glyph = &[
    rect(3.0, 4.0, 18.0, 16.0, 2.0, false),
    Part::Path("M14 4v16M3 14h11"),
];
/// Pull requests: Lucide's git-pull-request-arrow. Also the label of a pull request in the
/// graph.
pub const PULL_REQUEST: Glyph = &[
    ring(5.0, 6.0, 3.0),
    Part::Path("M5 9v12"),
    ring(19.0, 18.0, 3.0),
    Part::Path("M15 9 12 6l3-3M12 6h5a2 2 0 0 1 2 2v7"),
];
/// Reset: Lucide's rotate-ccw.
pub const RESET: Glyph = &[Part::Path(
    "M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8M3 3v5h5",
)];

/// Diff window, side by side: Lucide's columns-2.
pub const DIFF_SIDE_BY_SIDE: Glyph = &[
    rect(3.0, 3.0, 18.0, 18.0, 2.0, false),
    Part::Path("M12 3v18"),
];
/// Diff window, unified: Lucide's rows-2.
pub const DIFF_UNIFIED: Glyph = &[
    rect(3.0, 3.0, 18.0, 18.0, 2.0, false),
    Part::Path("M3 12h18"),
];
/// Fold unchanged lines: Lucide's fold-vertical, two arrows closing on a dashed line.
pub const FOLD: Glyph = &[Part::Path(
    "M12 22v-6M12 8V2M4 12H2M10 12H8M16 12h-2M22 12h-2M15 19l-3-3-3 3M15 5l-3 3-3-3",
)];
/// Show the whole file: Lucide's unfold-vertical, two arrows opening from a dashed line.
pub const UNFOLD: Glyph = &[Part::Path(
    "M12 22v-6M12 8V2M4 12H2M10 12H8M16 12h-2M22 12h-2M15 19l-3 3-3-3M15 5l-3-3-3 3",
)];
/// Whitespace counts: Lucide's pilcrow.
pub const WHITESPACE_COMPARE: Glyph =
    &[Part::Path("M13 4v16M17 4v16M19 4H9.5a4.5 4.5 0 0 0 0 9H13")];
/// Changes in whitespace don't count: Lucide's fold-horizontal, blanks squeezed together.
pub const WHITESPACE_IGNORE_CHANGES: Glyph = &[Part::Path(
    "M2 12h6M22 12h-6M12 2v2M12 8v2M12 14v2M12 20v2M19 9l-3 3 3 3M5 15l3-3-3-3",
)];
/// No whitespace counts: the pilcrow struck through.
pub const WHITESPACE_IGNORE_ALL: Glyph = &[Part::Path(
    "M13 4v16M17 4v16M19 4H9.5a4.5 4.5 0 0 0 0 9H13M4 3l16 18",
)];

/// Every glyph, for tests.
pub const ALL: [Glyph; 34] = [
    MENU,
    SEARCH,
    PLUS,
    MINUS,
    CLOSE,
    CHECK,
    CHEVRON_DOWN,
    CHEVRON_UP,
    CHEVRON_RIGHT,
    ZOOM,
    HEAD,
    OVERVIEW,
    LOCAL,
    REMOTE,
    TAGS,
    LABELLED,
    BRANCHINGS,
    ALL_COMMITS,
    ADAPT,
    FREE,
    SUBTREE,
    LAYOUT_STACKED,
    LAYOUT_SIDE_BY_SIDE,
    LAYOUT_DETAILS_BELOW,
    LAYOUT_FILES_RIGHT,
    PULL_REQUEST,
    RESET,
    DIFF_SIDE_BY_SIDE,
    DIFF_UNIFIED,
    FOLD,
    UNFOLD,
    WHITESPACE_COMPARE,
    WHITESPACE_IGNORE_CHANGES,
    WHITESPACE_IGNORE_ALL,
];

/// One stroke of a flattened path.
#[derive(Clone, Debug, PartialEq)]
pub struct Polyline {
    pub points: Vec<[f32; 2]>,
    /// Ends where it started (the path had a `Z`).
    pub closed: bool,
}

/// Path data that [`flatten`] can't read; the offset is in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("bad path data at byte {0}")]
pub struct PathError(pub usize);

/// Flattens SVG path data (the commands `M L H V C S Q A Z`, absolute and relative) into
/// polylines, keeping curves within about `tolerance` of the true shape.
pub fn flatten(d: &str, tolerance: f32) -> Result<Vec<Polyline>, PathError> {
    let mut lx = Lexer {
        s: d.as_bytes(),
        i: 0,
    };
    let mut out = Vec::new();
    let mut cur: Vec<[f32; 2]> = Vec::new();
    let (mut pos, mut start) = ([0.0, 0.0], [0.0, 0.0]);
    // The second control point of the previous C or S, for S's reflection.
    let mut last_ctrl: Option<[f32; 2]> = None;
    let mut cmd: Option<u8> = None;
    loop {
        lx.skip();
        if lx.i >= lx.s.len() {
            break;
        }
        match lx.s[lx.i] {
            c if c.is_ascii_alphabetic() => {
                cmd = Some(c);
                lx.i += 1;
            }
            _ if cmd.is_none() => return Err(PathError(lx.i)),
            _ => {}
        }
        let c = cmd.ok_or(PathError(lx.i))?;
        let rel = c.is_ascii_lowercase();
        let origin = if rel { pos } else { [0.0, 0.0] };
        let upper = c.to_ascii_uppercase();
        if !matches!(upper, b'M' | b'Z') && cur.is_empty() {
            cur.push(pos);
        }
        match upper {
            b'M' => {
                if cur.len() > 1 {
                    out.push(Polyline {
                        points: std::mem::take(&mut cur),
                        closed: false,
                    });
                }
                pos = lx.point(origin)?;
                start = pos;
                cur = vec![pos];
                // Further coordinate pairs are line-tos.
                cmd = Some(if rel { b'l' } else { b'L' });
            }
            b'L' => {
                pos = lx.point(origin)?;
                cur.push(pos);
            }
            b'H' => {
                pos = [lx.number()? + origin[0], pos[1]];
                cur.push(pos);
            }
            b'V' => {
                pos = [pos[0], lx.number()? + origin[1]];
                cur.push(pos);
            }
            b'C' | b'S' => {
                let c1 = if upper == b'C' {
                    lx.point(origin)?
                } else {
                    last_ctrl.map_or(pos, |k| [2.0 * pos[0] - k[0], 2.0 * pos[1] - k[1]])
                };
                let c2 = lx.point(origin)?;
                let end = lx.point(origin)?;
                cubic(pos, c1, c2, end, tolerance, &mut cur);
                pos = end;
                last_ctrl = Some(c2);
                continue;
            }
            b'Q' => {
                let q = lx.point(origin)?;
                let end = lx.point(origin)?;
                let c1 = [
                    pos[0] + 2.0 / 3.0 * (q[0] - pos[0]),
                    pos[1] + 2.0 / 3.0 * (q[1] - pos[1]),
                ];
                let c2 = [
                    end[0] + 2.0 / 3.0 * (q[0] - end[0]),
                    end[1] + 2.0 / 3.0 * (q[1] - end[1]),
                ];
                cubic(pos, c1, c2, end, tolerance, &mut cur);
                pos = end;
            }
            b'A' => {
                let radii = [lx.number()?, lx.number()?];
                let rotation = lx.number()?;
                let large = lx.flag()?;
                let sweep = lx.flag()?;
                let end = lx.point(origin)?;
                arc(pos, radii, rotation, large, sweep, end, tolerance, &mut cur);
                pos = end;
            }
            b'Z' => {
                if cur.last() == Some(&start) && cur.len() > 1 {
                    cur.pop();
                }
                if cur.len() > 1 {
                    out.push(Polyline {
                        points: std::mem::take(&mut cur),
                        closed: true,
                    });
                }
                cur.clear();
                pos = start;
                // A Z takes no coordinates; a number after it is an error.
                cmd = None;
            }
            _ => return Err(PathError(lx.i - 1)),
        }
        last_ctrl = None;
    }
    if cur.len() > 1 {
        out.push(Polyline {
            points: cur,
            closed: false,
        });
    }
    Ok(out)
}

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl Lexer<'_> {
    fn skip(&mut self) {
        while self
            .s
            .get(self.i)
            .is_some_and(|c| c.is_ascii_whitespace() || *c == b',')
        {
            self.i += 1;
        }
    }

    /// A number such as `-1.5e3`; a second `.` starts the next one (`1.6.2` is 1.6 and .2).
    fn number(&mut self) -> Result<f32, PathError> {
        self.skip();
        let begin = self.i;
        let at = |i: usize| self.s.get(i).copied();
        let mut i = self.i;
        if matches!(at(i), Some(b'-' | b'+')) {
            i += 1;
        }
        let mut dot = false;
        while let Some(c) = at(i) {
            match c {
                b'0'..=b'9' => {}
                b'.' if !dot => dot = true,
                _ => break,
            }
            i += 1;
        }
        if matches!(at(i), Some(b'e' | b'E')) {
            i += 1;
            if matches!(at(i), Some(b'-' | b'+')) {
                i += 1;
            }
            while at(i).is_some_and(|c| c.is_ascii_digit()) {
                i += 1;
            }
        }
        let text = std::str::from_utf8(&self.s[begin..i]).map_err(|_| PathError(begin))?;
        let n = text.parse().map_err(|_| PathError(begin))?;
        self.i = i;
        Ok(n)
    }

    fn point(&mut self, origin: [f32; 2]) -> Result<[f32; 2], PathError> {
        Ok([self.number()? + origin[0], self.number()? + origin[1]])
    }

    /// An arc flag: a single `0` or `1`, which may run into what follows.
    fn flag(&mut self) -> Result<bool, PathError> {
        self.skip();
        let flag = match self.s.get(self.i) {
            Some(b'0') => false,
            Some(b'1') => true,
            _ => return Err(PathError(self.i)),
        };
        self.i += 1;
        Ok(flag)
    }
}

/// Number of straight pieces for a curve of about `length`.
fn pieces(length: f32, tolerance: f32) -> usize {
    ((length / tolerance.max(1e-3)).sqrt().ceil() as usize).clamp(2, 64)
}

fn cubic(
    p0: [f32; 2],
    c1: [f32; 2],
    c2: [f32; 2],
    p1: [f32; 2],
    tol: f32,
    out: &mut Vec<[f32; 2]>,
) {
    let dist = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let n = pieces(dist(p0, c1) + dist(c1, c2) + dist(c2, p1), tol);
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        out.push([
            a * p0[0] + b * c1[0] + c * c2[0] + d * p1[0],
            a * p0[1] + b * c1[1] + c * c2[1] + d * p1[1],
        ]);
    }
}

/// An elliptical arc, from its SVG endpoint form (SVG 1.1, appendix F.6.5).
#[allow(clippy::too_many_arguments)]
fn arc(
    p0: [f32; 2],
    radii: [f32; 2],
    rotation_deg: f32,
    large: bool,
    sweep: bool,
    p1: [f32; 2],
    tol: f32,
    out: &mut Vec<[f32; 2]>,
) {
    let (mut rx, mut ry) = (radii[0].abs(), radii[1].abs());
    if p0 == p1 {
        return;
    }
    if rx == 0.0 || ry == 0.0 {
        out.push(p1);
        return;
    }
    let (sin, cos) = rotation_deg.to_radians().sin_cos();
    let (dx, dy) = ((p0[0] - p1[0]) / 2.0, (p0[1] - p1[1]) / 2.0);
    let x1 = cos * dx + sin * dy;
    let y1 = -sin * dx + cos * dy;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let sign = if large == sweep { -1.0 } else { 1.0 };
    let coef = sign * (num / den).max(0.0).sqrt();
    let (cx1, cy1) = (coef * rx * y1 / ry, -coef * ry * x1 / rx);
    let cx = cos * cx1 - sin * cy1 + (p0[0] + p1[0]) / 2.0;
    let cy = sin * cx1 + cos * cy1 + (p0[1] + p1[1]) / 2.0;
    let angle =
        |u: [f32; 2], v: [f32; 2]| (u[0] * v[1] - u[1] * v[0]).atan2(u[0] * v[0] + u[1] * v[1]);
    let theta = angle([1.0, 0.0], [(x1 - cx1) / rx, (y1 - cy1) / ry]);
    let mut delta = angle(
        [(x1 - cx1) / rx, (y1 - cy1) / ry],
        [(-x1 - cx1) / rx, (-y1 - cy1) / ry],
    );
    let tau = std::f32::consts::TAU;
    if !sweep && delta > 0.0 {
        delta -= tau;
    } else if sweep && delta < 0.0 {
        delta += tau;
    }
    let n = pieces(delta.abs() * rx.max(ry), tol);
    for i in 1..n {
        let t = theta + delta * i as f32 / n as f32;
        let (st, ct) = t.sin_cos();
        out.push([
            cos * rx * ct - sin * ry * st + cx,
            sin * rx * ct + cos * ry * st + cy,
        ]);
    }
    out.push(p1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(d: &str) -> Vec<Vec<[f32; 2]>> {
        flatten(d, 0.1)
            .unwrap()
            .into_iter()
            .map(|p| p.points)
            .collect()
    }

    #[test]
    fn lines_absolute_and_relative() {
        assert_eq!(points("M4 7h16"), [vec![[4.0, 7.0], [20.0, 7.0]]]);
        assert_eq!(
            points("M4 7h16M4 12H20"),
            [
                vec![[4.0, 7.0], [20.0, 7.0]],
                vec![[4.0, 12.0], [20.0, 12.0]]
            ]
        );
        assert_eq!(
            points("m1 1 2 2L5 5v-1"),
            [vec![[1.0, 1.0], [3.0, 3.0], [5.0, 5.0], [5.0, 4.0]]]
        );
    }

    #[test]
    fn compact_numbers() {
        let p = &points("M6 6.4l1.6.2-.4 1.6")[0];
        let want = [[6.0, 6.4], [7.6, 6.6], [7.2, 8.2]];
        for (a, b) in p.iter().zip(want) {
            assert!(
                (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5,
                "{p:?}"
            );
        }
    }

    #[test]
    fn close_path() {
        let lines = flatten("M0 0h4v4z", 0.1).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].closed);
        assert_eq!(lines[0].points, [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0]]);
    }

    #[test]
    fn arcs_stay_on_the_circle() {
        // Half a circle of radius 10 around (10, 10), clockwise on screen: over the top.
        let p = &points("M0 10a10 10 0 0 1 20 0")[0];
        assert_eq!(*p.last().unwrap(), [20.0, 10.0]);
        for q in p {
            let r = (q[0] - 10.0).hypot(q[1] - 10.0);
            assert!((r - 10.0).abs() < 1e-3, "{q:?} is {r} from the centre");
            assert!(q[1] <= 10.0 + 1e-3);
        }
        assert!(p.iter().any(|q| q[1] < 0.5));
    }

    #[test]
    fn curves_end_where_they_should() {
        let p = &points("M12 7c4 0 5.5 1 5.5 4.5S16 17.5 12 17.5")[0];
        let near =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4;
        assert!(p.iter().any(|&q| near(q, [17.5, 11.5])));
        assert!(near(*p.last().unwrap(), [12.0, 17.5]));
    }

    #[test]
    fn bad_data_is_an_error() {
        assert!(flatten("M0 0 L x", 0.1).is_err());
        assert!(flatten("10 10", 0.1).is_err());
        assert!(flatten("M0 0h4z 3", 0.1).is_err());
    }

    #[test]
    fn every_glyph_parses_and_fits_its_box() {
        for glyph in ALL {
            for part in glyph {
                if let Part::Path(d) = part {
                    let lines = flatten(d, 0.05).unwrap_or_else(|e| panic!("{d}: {e}"));
                    assert!(!lines.is_empty(), "{d}");
                    for q in lines.iter().flat_map(|l| &l.points) {
                        assert!(
                            (-0.5..=SIZE + 0.5).contains(&q[0])
                                && (-0.5..=SIZE + 0.5).contains(&q[1]),
                            "{d}: {q:?} is outside the box"
                        );
                    }
                }
            }
        }
    }
}
