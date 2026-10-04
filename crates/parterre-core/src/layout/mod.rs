//! Layered ("Sugiyama") layout of a revision graph.
//!
//! The pipeline is the classic one, as used by TortoiseGit through OGDF and by graphviz `dot`:
//!
//! 1. [`rank`]: assign every node to a layer so that parents lie below their children.
//! 2. [`LayeredGraph`]: split edges spanning several layers into chains of dummy items.
//! 3. [`order`]: permute items within each layer to reduce edge crossings.
//! 4. [`position`]: assign coordinates within each layer, keeping edges short and straight.
//! 5. Route every edge through its dummy items, and rotate the result to the requested
//!    [`Direction`].
//!
//! Everything is computed in an abstract frame where `u` runs along a layer and `v` runs across
//! layers (newest first); [`Direction`] maps that frame onto screen x/y at the end.

mod layered;
mod order;
mod position;
pub mod rank;
mod trunk;

use serde::{Deserialize, Serialize};

pub use layered::{Item, LayeredGraph};

/// A point or size in layout space (logical pixels at zoom 1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Point {
        Point { x, y }
    }
}

/// Where the newest commits are placed; history flows away from that side.
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

    /// Unit vector pointing from newer to older commits on screen.
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

/// How nodes are assigned to layers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Ranking {
    /// Minimise total edge length (network simplex, as OGDF's `OptimalRanking` and graphviz
    /// do). Produces the compact, tree-like look.
    #[default]
    Compact,
    /// Every node sits directly above its highest parent (longest path from the roots).
    /// Fast; branches hang close to where they forked.
    LongestPath,
    /// One layer per node, strictly ordered by commit date like a log view.
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

/// PROTOTYPE (centred trunk, throwaway): how the default branch's first-parent line is laid out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Trunk {
    /// As before: the trunk starts leftmost and bends towards its side branches.
    #[default]
    Current,
    /// One straight line, every side branch to its right.
    StraightLeft,
    /// One straight line, side branches on both sides, each where its rows stay narrowest.
    CentredBalanced,
    /// One straight line, side branches alternating right and left down the trunk.
    CentredAlternating,
}

impl Trunk {
    pub const ALL: [Trunk; 4] = [
        Trunk::Current,
        Trunk::StraightLeft,
        Trunk::CentredBalanced,
        Trunk::CentredAlternating,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Trunk::Current => "A — current",
            Trunk::StraightLeft => "B — straight trunk, branches right",
            Trunk::CentredBalanced => "C — centred trunk, balanced sides",
            Trunk::CentredAlternating => "D — centred trunk, alternating sides",
        }
    }

    /// `A`–`D`, for `PARTERRE_TRUNK`.
    pub fn from_letter(s: &str) -> Option<Trunk> {
        let i = "ABCD".find(s.trim().to_ascii_uppercase().as_str())?;
        Trunk::ALL.get(i).copied()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutOptions {
    pub direction: Direction,
    pub ranking: Ranking,
    /// Gap between adjacent layers (between node boxes), in layout units. TortoiseGit: 30.
    pub layer_gap: f32,
    /// Minimum gap between neighbouring nodes within a layer. TortoiseGit: 25.
    pub node_gap: f32,
    /// Width reserved for an edge passing through a layer.
    pub edge_gap: f32,
    /// Extra gap between layers per unit of the widest horizontal edge span across the gap,
    /// keeping long edges steep (0 = fixed spacing).
    pub gap_per_span: f32,
    /// Upper bound for the variable layer gap. TortoiseGit (OGDF): 300.
    pub max_layer_gap: f32,
    /// Layers wider than this are split into several (0 = never, as TortoiseGit).
    pub max_layer_width: f32,
    /// Merge edges that run into the same parent into one trunk.
    pub concentrate_edges: bool,
    /// PROTOTYPE (centred trunk).
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
            trunk: Trunk::Current,
        }
    }
}

/// Graph to be laid out. Nodes are identified by their index.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutInput {
    /// On-screen box size of every node (width, height), independent of direction.
    pub sizes: Vec<Point>,
    /// Commit timestamp of every node, used for chronological ranking and tie-breaking.
    pub times: Vec<i64>,
    /// Edges from child (newer) to parent (older).
    pub edges: Vec<LayoutEdge>,
    /// Nodes to place first (leftmost/topmost within their layers), e.g. the default branch.
    pub priority: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutEdge {
    pub child: u32,
    pub parent: u32,
    /// True if `parent` is the child's first parent (the "mainline" of that commit).
    pub first_parent: bool,
}

/// Result of a layout, in screen-oriented layout space.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub direction: Direction,
    /// Centre of every node.
    pub nodes: Vec<Point>,
    /// For every input edge: the polyline from the child's border through its bend points to
    /// the parent's border. Renderers should draw smooth curves with tangents along
    /// [`Direction::flow`] at every point.
    pub edges: Vec<Vec<Point>>,
    /// (child, parent) node of every edge.
    pub edge_ends: Vec<(u32, u32)>,
    /// Whether every edge leads to its child's first parent.
    pub edge_first_parent: Vec<bool>,
    /// Identity of every bend point of every edge (same id = the same point, shared by
    /// bundled edges). Parallel to the interior points of [`Layout::edges`].
    pub edge_bends: Vec<Vec<u32>>,
    /// Layer of every node (0 = newest).
    pub layers: Vec<u32>,
    /// Edge crossings between adjacent layers after ordering (bundled edges count once).
    pub crossings: u64,
    /// Top-left and bottom-right corner of the drawing.
    pub min: Point,
    pub max: Point,
}

/// Runs the full layout pipeline.
pub fn layout(input: &LayoutInput, options: &LayoutOptions) -> Layout {
    let n = input.sizes.len();
    if n == 0 {
        return Layout {
            direction: options.direction,
            ..Layout::default()
        };
    }
    let vertical = options.direction.is_vertical();
    // Extent of every node along a layer (breadth) and across layers (depth).
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
    let mut graph = LayeredGraph::build(
        input,
        &layers,
        &breadth,
        options.edge_gap,
        options.concentrate_edges,
    );
    graph.side = trunk::sides(&graph, input, options.trunk, options.node_gap);
    let crossings = order::minimize_crossings(&mut graph, input);
    let u = position::assign(&graph, options.node_gap);

    // Layer depth = deepest node in the layer. Layers are stacked with at least `layer_gap`
    // between them, more where edges cross the gap at a shallow angle (as OGDF's
    // FastHierarchyLayout does), so that edges stay steep enough to follow.
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

    let to_screen = |u_: f32, v_: f32| -> Point {
        let u_ = u_ - min_u;
        match options.direction {
            Direction::NewestTop => Point::new(u_, v_),
            Direction::NewestBottom => Point::new(u_, total_v - v_),
            Direction::NewestLeft => Point::new(v_, u_),
            Direction::NewestRight => Point::new(total_v - v_, u_),
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
