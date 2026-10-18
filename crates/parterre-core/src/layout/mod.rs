//! Layered ("Sugiyama") layout, as TortoiseGit (through OGDF) and graphviz `dot` do it: rank
//! the nodes into layers, split long edges into chains of dummy items, order every layer to
//! reduce crossings, assign coordinates, and route every edge through its dummies.
//!
//! It all happens in a frame where `u` runs along a layer and `v` across layers, newest first;
//! [`Direction`] maps that frame onto the screen at the end.

mod layered;
mod order;
mod position;
pub mod rank;
mod trunk;

use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

use serde::{Deserialize, Serialize};

use layered::LayeredGraph;

/// In logical pixels at zoom 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    pub fn dot(self, other: Point) -> f32 {
        self.x * other.x + self.y * other.y
    }

    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
}

impl Add for Point {
    type Output = Point;

    fn add(self, other: Point) -> Point {
        Point::new(self.x + other.x, self.y + other.y)
    }
}

impl AddAssign for Point {
    fn add_assign(&mut self, other: Point) {
        *self = *self + other;
    }
}

impl Sub for Point {
    type Output = Point;

    fn sub(self, other: Point) -> Point {
        Point::new(self.x - other.x, self.y - other.y)
    }
}

impl SubAssign for Point {
    fn sub_assign(&mut self, other: Point) {
        *self = *self - other;
    }
}

impl Mul<f32> for Point {
    type Output = Point;

    fn mul(self, factor: f32) -> Point {
        Point::new(self.x * factor, self.y * factor)
    }
}

impl Neg for Point {
    type Output = Point;

    fn neg(self) -> Point {
        Point::new(-self.x, -self.y)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    #[default]
    NewestTop,
    NewestBottom,
    NewestLeft,
    NewestRight,
}

impl Direction {
    pub const ALL: [Direction; 4] = [
        Direction::NewestTop,
        Direction::NewestBottom,
        Direction::NewestLeft,
        Direction::NewestRight,
    ];

    pub fn is_vertical(self) -> bool {
        matches!(self, Direction::NewestTop | Direction::NewestBottom)
    }

    /// The unit vector from newer to older commits on screen.
    pub fn flow(self) -> Point {
        match self {
            Direction::NewestTop => Point::new(0.0, 1.0),
            Direction::NewestBottom => Point::new(0.0, -1.0),
            Direction::NewestLeft => Point::new(1.0, 0.0),
            Direction::NewestRight => Point::new(-1.0, 0.0),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Direction::NewestTop => "Newest on top",
            Direction::NewestBottom => "Newest at bottom",
            Direction::NewestLeft => "Newest on the left",
            Direction::NewestRight => "Newest on the right",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Ranking {
    /// The least total edge length, by network simplex (as OGDF's `OptimalRanking` and
    /// graphviz).
    #[default]
    Compact,
    /// Every node directly above its highest parent.
    LongestPath,
    /// One layer per node, by commit date.
    Chronological,
}

impl Ranking {
    pub const ALL: [Ranking; 3] = [
        Ranking::Compact,
        Ranking::LongestPath,
        Ranking::Chronological,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Ranking::Compact => "Compact (tree-like)",
            Ranking::LongestPath => "Near fork point",
            Ranking::Chronological => "Chronological",
        }
    }
}

/// Where the trunk goes: the first-parent line of the first [`LayoutInput::priority`] node,
/// the default branch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Trunk {
    /// One straight line, each side branch on the side where its layers stay narrowest.
    #[default]
    Centred,
    /// One straight line, side branches taking turns right and left down the line.
    Alternating,
    /// Leftmost in its layers, bending towards its side branches like any other line.
    Leftmost,
}

impl Trunk {
    pub const ALL: [Trunk; 3] = [Trunk::Centred, Trunk::Alternating, Trunk::Leftmost];

    pub fn label(self) -> &'static str {
        match self {
            Trunk::Centred => "Centred",
            Trunk::Alternating => "Centred, alternating sides",
            Trunk::Leftmost => "Leftmost",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutOptions {
    pub direction: Direction,
    pub ranking: Ranking,
    /// The least gap between the boxes of adjacent layers. TortoiseGit: 30.
    pub layer_gap: f32,
    /// The least gap between neighbouring nodes in a layer. TortoiseGit: 25.
    pub node_gap: f32,
    /// The room an edge takes where it passes through a layer.
    pub edge_gap: f32,
    /// Extra gap between layers per unit of the widest span along the layers of an edge
    /// across the gap, keeping long edges steep (0 = fixed spacing).
    pub gap_per_span: f32,
    /// TortoiseGit (OGDF): 300.
    pub max_layer_gap: f32,
    /// Wider layers are split into several (0 = never, as TortoiseGit).
    pub max_layer_width: f32,
    /// Edges into the same parent share their bend points (graphviz's `concentrate`).
    pub concentrate_edges: bool,
    pub trunk: Trunk,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        LayoutOptions {
            direction: Direction::NewestTop,
            ranking: Ranking::Compact,
            layer_gap: 30.0,
            node_gap: 25.0,
            edge_gap: 12.0,
            gap_per_span: 0.1,
            max_layer_gap: 300.0,
            max_layer_width: 1800.0,
            concentrate_edges: false,
            trunk: Trunk::Centred,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutInput {
    /// The box of every node on screen, whatever the direction.
    pub sizes: Vec<Point>,
    /// The commit time of every node; missing ones count as 0.
    pub times: Vec<i64>,
    pub edges: Vec<LayoutEdge>,
    /// Nodes to place first, e.g. the default branch. The first one's first-parent line is the
    /// [`Trunk`].
    pub priority: Vec<u32>,
}

impl LayoutInput {
    fn time(&self, node: usize) -> i64 {
        self.times.get(node).copied().unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutEdge {
    pub child: u32,
    pub parent: u32,
    pub first_parent: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub direction: Direction,
    /// The centre of every node.
    pub nodes: Vec<Point>,
    /// For every input edge, the polyline from the child's border through its bend points to
    /// the parent's border. Drawn as smooth curves with tangents along [`Direction::flow`] at
    /// every point.
    pub edges: Vec<Vec<Point>>,
    /// The (child, parent) of every edge.
    pub edge_ends: Vec<(u32, u32)>,
    pub edge_first_parent: Vec<bool>,
    /// Every bend point's identity, parallel to the interior points of [`Layout::edges`]:
    /// bundled edges share their bend points.
    pub edge_bends: Vec<Vec<u32>>,
    /// The layer of every node, 0 the newest.
    pub layers: Vec<u32>,
    /// Bundled edges count once.
    pub crossings: u64,
    pub min: Point,
    pub max: Point,
}

fn layer_count(layers: &[u32]) -> usize {
    layers.iter().map(|&l| l as usize + 1).max().unwrap_or(0)
}

pub fn layout(input: &LayoutInput, options: &LayoutOptions) -> Layout {
    let n = input.sizes.len();
    if n == 0 {
        return Layout {
            direction: options.direction,
            ..Layout::default()
        };
    }
    let vertical = options.direction.is_vertical();
    // Every node's extent along a layer (breadth) and across layers (depth).
    let (breadth, depth): (Vec<f32>, Vec<f32>) = input
        .sizes
        .iter()
        .map(|s| if vertical { (s.x, s.y) } else { (s.y, s.x) })
        .unzip();

    let mut layers = rank::rank(input, options.ranking);
    if options.ranking != Ranking::Chronological {
        rank::limit_width(
            &mut layers,
            input,
            &breadth,
            options.max_layer_width,
            options.node_gap,
        );
    }
    let sides = trunk::sides(input, &layers, &breadth, options);
    let mut graph = LayeredGraph::build(input, &layers, &breadth, options, sides.as_ref());
    let crossings = order::minimize_crossings(&mut graph, input);
    let u = position::assign(&graph, options.node_gap);

    // Layers are stacked at least `layer_gap` apart, more where edges cross the gap at a
    // shallow angle (as OGDF's FastHierarchyLayout does), so that edges stay steep enough to
    // follow.
    let layer_count = graph.layers.len();
    let mut layer_depth = vec![0.0f32; layer_count];
    for (node, &l) in layers.iter().enumerate() {
        layer_depth[l as usize] = layer_depth[l as usize].max(depth[node]);
    }
    let mut span_below = vec![0.0f32; layer_count];
    for (i, item) in graph.items.iter().enumerate() {
        for &(below, _) in graph.down(i) {
            let dx = (u[i] - u[below as usize]).abs();
            span_below[item.layer as usize] = span_below[item.layer as usize].max(dx);
        }
    }
    // max/min rather than clamp: never panic on odd (e.g. NaN) settings.
    let gap_after = |l: usize| {
        (span_below[l] * options.gap_per_span)
            .min(options.max_layer_gap)
            .max(options.layer_gap)
    };
    let mut layer_v = Vec::with_capacity(layer_count);
    let mut v = 0.0;
    let mut last_gap = 0.0;
    for (l, d) in layer_depth.iter().enumerate() {
        layer_v.push(v + d / 2.0);
        last_gap = gap_after(l);
        v += d + last_gap;
    }
    let total_v = v - last_gap;

    let (min_u, max_u) =
        graph
            .items
            .iter()
            .enumerate()
            .fold((f32::MAX, f32::MIN), |(lo, hi), (i, item)| {
                (
                    lo.min(u[i] - item.breadth / 2.0),
                    hi.max(u[i] + item.breadth / 2.0),
                )
            });

    let to_screen = |u: f32, v: f32| -> Point {
        let u = u - min_u;
        match options.direction {
            Direction::NewestTop => Point::new(u, v),
            Direction::NewestBottom => Point::new(u, total_v - v),
            Direction::NewestLeft => Point::new(v, u),
            Direction::NewestRight => Point::new(total_v - v, u),
        }
    };

    let nodes: Vec<Point> = (0..n)
        .map(|i| to_screen(u[i], layer_v[layers[i] as usize]))
        .collect();

    let edges = graph
        .chains
        .iter()
        .zip(&input.edges)
        .map(|(chain, e)| {
            let (c, p) = (e.child as usize, e.parent as usize);
            let mut pts = Vec::with_capacity(chain.len() + 2);
            pts.push(to_screen(
                u[c],
                layer_v[layers[c] as usize] + depth[c] / 2.0,
            ));
            for &d in chain {
                pts.push(to_screen(
                    u[d as usize],
                    layer_v[graph.items[d as usize].layer as usize],
                ));
            }
            pts.push(to_screen(
                u[p],
                layer_v[layers[p] as usize] - depth[p] / 2.0,
            ));
            pts
        })
        .collect();

    let extent = to_screen(max_u, total_v);
    let origin = to_screen(min_u, 0.0);
    Layout {
        direction: options.direction,
        nodes,
        edges,
        edge_ends: input.edges.iter().map(|e| (e.child, e.parent)).collect(),
        edge_first_parent: input.edges.iter().map(|e| e.first_parent).collect(),
        edge_bends: graph.chains.clone(),
        layers,
        crossings,
        min: Point::new(origin.x.min(extent.x), origin.y.min(extent.y)),
        max: Point::new(origin.x.max(extent.x), origin.y.max(extent.y)),
    }
}
