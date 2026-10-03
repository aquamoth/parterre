//! Moving nodes around by hand: the graph as a net of weak springs and weakly repelling nodes.
//!
//! Every node and every edge bend point is a particle with a *rest position*, at first its
//! place in the layout. Springs along every edge keep the offsets between rest positions, so a
//! dragged node pulls its neighbours along. Nodes push each other away when they come closer
//! than they rest (up to a short range), like weak magnets; nodes side by side in a row keep
//! their order, like beads on a string, unless one is lifted out of the row. Edges are pushed
//! aside the same way by their neighbours in a layer. Each particle is weakly anchored to its
//! rest position, which limits how far a pull spreads.
//!
//! What a drag does depends on the [`DragModel`]. In [`DragModel::Adapt`] the rest of the graph
//! gives way a little, and edges keep running along the history direction, so children stay
//! above their parents; only the dragged nodes, and what was left out of order at rest, can
//! break that order. In the other models only the dragged nodes move and the edges to them
//! stretch. Dropping makes the new shape permanent: every particle that moved now rests where
//! it is, so the springs keep the *new* offsets and nothing drifts back. Dropped nodes are not
//! pinned; they give way to later drags like any other node. Every drop can be undone, and a
//! reset sends everything back to the layout.
//!
//! The state is kept as *displacements from the layout*, which stay small and therefore exact
//! even where layout coordinates run into the millions. Each frame of an adaptive drag has two
//! parts:
//!
//! 1. **Shape.** The net's shape for the current drag minimises
//!    `Σ k_s |d_b − d_a − (h_b − h_a)|² + Σ k_a |d_i − h_i|²` (plus the magnets) over
//!    displacements `d`, where `h` are the rest displacements and the dragged nodes are held
//!    fixed. It is found by Gauss-Seidel relaxation over the woken particles, warm-started from
//!    the previous frame. In a chain the displacement decays by a factor λ per hop when
//!    `k_a / k_s = (1 − λ)² / λ`, which is what [`NetParams::pull`] sets.
//! 2. **Motion.** Every particle follows its target through a damped spring, so the net moves
//!    with some inertia and, depending on [`NetParams::wobble`], overshoots a little.
//!
//! Only particles near the dragged nodes are simulated (breadth-first up to a budget), so
//! dragging stays smooth in graphs with a million bend points.
//!
//! Edges keep the layout's route (through their bend points) until it stops fitting: edges at
//! nodes moved by hand as soon as their nodes move relative to each other, other edges once
//! they are pulled far out of shape, and any edge a moved node comes to cover. Those are routed
//! afresh around the nodes ([`crate::route`]), while dragging and whenever the net settles.
//! Edges the layout bundled into a trunk stay bundled: a routed edge joins its bundle's trunk,
//! and the trunk is routed afresh as a whole once it no longer fits.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::layout::{Layout, Point};
use crate::route::{self, Obstacles};

/// What moves when nodes are dragged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DragModel {
    /// The dragged nodes follow the pointer and the rest of the graph gives way a little:
    /// neighbours are pulled along their edges, and nodes in the way are pushed aside.
    #[default]
    #[serde(alias = "Net", alias = "Strings")]
    Adapt,
    /// Only the dragged nodes move; the edges to them stretch.
    #[serde(alias = "Rigid")]
    Free,
    /// The dragged nodes move together with everything that grows out of them
    /// ([`crate::revgraph::RevGraph::subtree`]); nothing else moves. The caller passes the
    /// whole subtree to [`Net::grab`].
    Subtree,
}

impl DragModel {
    pub const ALL: [DragModel; 3] = [DragModel::Adapt, DragModel::Free, DragModel::Subtree];

    pub fn label(self) -> &'static str {
        match self {
            DragModel::Adapt => "Adapt",
            DragModel::Free => "Free",
            DragModel::Subtree => "Subtree",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            DragModel::Adapt => {
                "The graph gives way: neighbours follow, nodes in the way move aside"
            }
            DragModel::Free => "Move only the selected nodes; nothing else moves",
            DragModel::Subtree => {
                "Move the selected nodes and everything that grows out of them \
                 (their first-parent descendants); nothing else moves"
            }
        }
    }

    /// True if the rest of the graph reacts to a drag.
    pub fn adapts(self) -> bool {
        self == DragModel::Adapt
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetParams {
    pub model: DragModel,
    /// 0..=1: how far a pull spreads along the edges (Adapt).
    pub pull: f32,
    /// 0..=1: how strongly, and from how far, nodes push each other away (Adapt).
    pub push: f32,
    /// 0..=1: how much the net overshoots and wobbles before settling (Adapt).
    pub wobble: f32,
    /// Push overlapping nodes apart (Adapt).
    pub avoid_overlap: bool,
}

impl Default for NetParams {
    fn default() -> Self {
        NetParams {
            model: DragModel::Adapt,
            pull: 0.3,
            push: 0.5,
            wobble: 0.4,
            avoid_overlap: true,
        }
    }
}

/// At most this many particles are woken around the grabbed nodes; the rest of a huge graph
/// stays still (a pull has decayed to nothing long before that many hops).
const ACTIVE_BUDGET: usize = 8_000;
/// Particles woken around a node that gets pushed from outside the simulated neighbourhood.
const PUSH_WAKE: usize = 32;
/// Gauss-Seidel sweeps over the woken particles per frame.
const SWEEPS: usize = 12;
/// Sweeps when a drag ends, so that the shape that becomes permanent has converged.
const FINAL_SWEEPS: usize = 48;
/// Rounds of [`SWEEPS`] when settling after a restore (see [`Net::settle`]), and how many
/// particles around the overlapping nodes are woken for it at first.
const SETTLE_ROUNDS: usize = 4;
const SETTLE_WAKE: usize = 1_000;
/// At most this many passes of separating alone end settling after a restore, and they stop
/// after this many in a row clear no more overlap.
const SEPARATE_ROUNDS: usize = 64;
const SEPARATE_PATIENCE: usize = 4;
/// Integration substeps per frame for following the targets.
const SUBSTEPS: usize = 4;
/// Natural frequency (Hz) with which particles follow their targets.
const FOLLOW_HZ: f32 = 3.0;
/// Minimum gap kept between node boxes when avoiding overlap.
const OVERLAP_MARGIN: f32 = 6.0;
/// No bundle, for [`Net::bundle`].
const NO_BUNDLE: u32 = u32::MAX;
/// No first parent, for [`Net::first_parent`].
const NO_NODE: u32 = u32::MAX;
/// Minimum distance along the history direction between the boxes at the two ends of an edge
/// segment while the graph adapts, so that an edge always visibly leaves its child towards its
/// parent. More than twice [`route::CLEARANCE`], so that routes still fit between rows.
pub const FLOW_GAP: f32 = 24.0;
/// Flags set by [`Net::keep_flow_order`]: the particle can't move on along the flow, or back
/// against it, without moving a held particle.
const STUCK_ON: u8 = 1;
const STUCK_BACK: u8 = 2;
/// Flags set by [`Net::separate_nodes`] for the next flow-order pass: the particle was pushed
/// on along the flow, or back against it, to clear an overlap.
const SEPARATED_ON: u8 = 1;
const SEPARATED_BACK: u8 = 2;
/// At `push` = 1, nodes closer than this (gap between boxes, layout units) push each other
/// away, with this stiffness (edge springs have 1). Both scale linearly with `push`.
const MAGNET_RANGE: f32 = 64.0;
const MAGNET_STIFFNESS: f32 = 1.5;
/// Cell size (layout units) of the coarse grid that picks the nodes near moving ones.
const NEAR_CELL: f32 = 512.0;
/// Nearby nodes are collected once per frame, up to this far beyond the magnet range.
const NEAR_SLACK: f32 = 24.0;
/// A drop changes a particle's rest position only if it moved more than this.
const REST_EPSILON: f32 = 0.05;
/// An edge at a node moved by hand gets a route of its own once its nodes have moved this much
/// relative to each other (layout units); any other edge once they have moved this much.
const REROUTE_AFTER: f32 = 1.0;
const REROUTE_ANYWAY: f32 = 40.0;
/// Routes are brought up to date when a node has moved this much.
const ROUTE_STEP: f32 = 0.25;
/// A moved node blocks a layout route only if the route runs this far inside its box.
const BLOCK_SLACK: f32 = 2.0;
/// Number of drops kept for undo, and how many particle changes they may hold in all (about
/// 20 bytes each).
const UNDO_LIMIT: usize = 200;
const UNDO_ENTRIES: usize = 1_000_000;
/// The simulation sleeps once no particle moves faster than this (layout units per second)
/// and every particle is this close to its target.
const SLEEP_SPEED: f32 = 1.0;
const SLEEP_DISTANCE: f32 = 0.1;

#[derive(Clone, Copy, Debug)]
struct Spring {
    a: u32,
    b: u32,
    /// Offset of `b` from `a` in the layout.
    offset: Point,
    stiffness: f32,
    /// True for springs along edges, false for those between neighbours in a layer.
    along_edge: bool,
}

/// Spring ids at every particle, stored flat: particle `i`'s are `ids[start[i]..start[i + 1]]`.
/// A `Vec` per particle would cost 24 bytes plus a heap block each, for millions of particles
/// in all-commits views of big repositories.
#[derive(Clone, Debug)]
struct SpringsAt {
    start: Vec<u32>,
    ids: Vec<u32>,
}

impl SpringsAt {
    fn new(particle_count: usize, springs: &[Spring]) -> SpringsAt {
        let mut start = vec![0u32; particle_count + 1];
        for s in springs {
            start[s.a as usize + 1] += 1;
            start[s.b as usize + 1] += 1;
        }
        for i in 0..particle_count {
            start[i + 1] += start[i];
        }
        let mut next = start.clone();
        let mut ids = vec![0; start[particle_count] as usize];
        for (i, s) in springs.iter().enumerate() {
            for p in [s.a, s.b] {
                let slot = &mut next[p as usize];
                ids[*slot as usize] = i as u32;
                *slot += 1;
            }
        }
        SpringsAt { start, ids }
    }

    fn get(&self, particle: usize) -> &[u32] {
        &self.ids[self.start[particle] as usize..self.start[particle + 1] as usize]
    }
}

/// A route of an edge's own, used once its layout route no longer fits.
#[derive(Clone, Debug)]
struct Route {
    bends: Vec<Point>,
    /// Where the route joins its bundle's trunk (an index into [`Net::trunks`]); from there on
    /// it runs through the trunk's bend points.
    join: Option<u32>,
}

/// How the edges of a bundle run into their parent.
#[derive(Clone, Debug)]
enum Trunk {
    /// Through the trunk's bend points, as in the layout.
    Layout,
    /// Along a route of the trunk's own, from its first bend point on.
    Routed(Vec<Point>),
    /// The parent has been moved before the trunk: every edge takes a route of its own.
    Broken,
}

/// A drag in progress.
#[derive(Clone, Debug)]
struct Grab {
    /// The node under the pointer, and its displacement when grabbed.
    anchor: u32,
    anchor_start: Point,
    /// Held particles and their displacements when grabbed; they all move by `delta`.
    held: Vec<(u32, Point)>,
    /// Nodes that count as moved by hand once dropped.
    marked: Vec<u32>,
    /// Edges with a held end and bend points that are not all held: they stretch.
    stretched: Vec<u32>,
    /// Bend points of the stretched edges (which follow them exactly unless the net adapts).
    stretched_bends: Vec<u32>,
    delta: Point,
    /// True if the rest of the net gives way; false if it stays still.
    adapt: bool,
}

/// One undoable step: rest positions and "moved by hand" marks before and after.
#[derive(Clone, Debug, Default)]
struct Change {
    homes: Vec<(u32, Point, Point)>,
    marks: Vec<(u32, bool, bool)>,
}

impl Change {
    fn is_empty(&self) -> bool {
        self.homes.is_empty() && self.marks.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct Net {
    node_count: usize,
    /// Layout position of every particle.
    origin: Vec<Point>,
    /// Rest position of every particle, as a displacement from the layout.
    home: Vec<Point>,
    /// Number of particles that rest away from the layout.
    displaced: usize,
    /// Current displacement from the layout, its velocity, and where it is heading.
    disp: Vec<Point>,
    vel: Vec<Point>,
    target: Vec<Point>,
    /// Half extents of node boxes (zero for bend points).
    half: Vec<Point>,
    /// Where every woken particle was heading at the start of the frame.
    before: Vec<Point>,
    /// Whether layers run horizontally (newest on top or bottom), and the direction from newer
    /// to older commits.
    vertical: bool,
    flow: Point,
    /// Nodes the user moved by hand.
    moved: Vec<bool>,
    springs: Vec<Spring>,
    /// Spring ids per particle.
    adjacent: SpringsAt,
    /// Particles of every edge, child node first, parent node last.
    chains: Vec<Vec<u32>>,
    /// Edges at every node.
    node_edges: Vec<Vec<u32>>,
    /// The first parent of every node, or [`NO_NODE`].
    first_parent: Vec<u32>,
    /// Number of edges through every bend point (more than one where edges are bundled),
    /// indexed by particle minus `node_count`.
    bend_edges: Vec<u32>,
    grab: Option<Grab>,
    /// Particles held by the current grab.
    held: Vec<bool>,
    awake: bool,
    /// Particles follow their targets with some overshoot (during and after an adaptive drag)
    /// or critically damped.
    wobbly: bool,
    /// Particles being simulated; everything else is at rest.
    active: Vec<u32>,
    is_active: Vec<bool>,
    /// Counts changes to `active`, to know when `segments` is out of date.
    active_changes: u64,
    /// Edge segments at the woken particles, for [`Net::keep_flow_order`].
    segments: FlowSegments,
    /// [`STUCK_ON`] and [`STUCK_BACK`] flags from the latest flow-order pass that looked.
    flow_stuck: Vec<u8>,
    /// [`SEPARATED_ON`] and [`SEPARATED_BACK`] flags since the latest flow-order pass.
    separated: Vec<u8>,
    grid: Grid,
    /// Per frame: nodes near each node that push it away, and the gap they keep.
    magnets: Vec<Vec<(u32, f32)>>,
    magnet_nodes: Vec<u32>,
    undo: Vec<Change>,
    redo: Vec<Change>,
    /// Edges into the same parent that the layout bundled: the bend points they share, in
    /// order (the trunk), and the edges. Each edge's layout route ends with part of the trunk.
    trunks: Vec<Vec<u32>>,
    bundle_edges: Vec<Vec<u32>>,
    /// The bundle of every edge, or [`NO_BUNDLE`].
    bundle: Vec<u32>,
    /// How each bundle's trunk runs, worked out once per routing pass.
    trunk_cache: HashMap<u32, Trunk>,
    /// Edges whose layout route no longer fits get a route of their own.
    routes: Vec<Option<Route>>,
    /// Per edge: its route was made for an edge turned around (see [`Net::turns`]).
    turned: Vec<bool>,
    /// Node boxes for routing, set up when first needed.
    obstacles: Option<Obstacles>,
    /// Where each node was (as a displacement) when last placed among the obstacles.
    placed_at: Vec<Point>,
    /// Settling after a restore (see [`Net::settle`]): nodes resting away from the layout
    /// press on the others even at rest.
    settling: bool,
}

impl Net {
    /// Builds the net for a layout. `sizes` are the node box sizes.
    pub fn new(layout: &Layout, sizes: &[Point]) -> Net {
        let n = layout.nodes.len();
        let mut origin = layout.nodes.clone();
        let mut half: Vec<Point> = sizes
            .iter()
            .map(|s| Point::new(s.x / 2.0, s.y / 2.0))
            .collect();
        let mut layer_of: Vec<u32> = layout.layers.clone();
        let mut chains = Vec::with_capacity(layout.edges.len());
        let mut node_edges = vec![Vec::new(); n];
        let mut first_parent = vec![NO_NODE; n];
        let mut bend_edges: Vec<u32> = Vec::new();
        let mut springs = Vec::new();
        let mut seen_segments = HashSet::new();
        // Bend point identity -> particle, so bundled edges share their particles.
        let mut bend_particle: HashMap<u32, u32> = HashMap::new();

        for (e, pts) in layout.edges.iter().enumerate() {
            let (child, parent) = layout.edge_ends[e];
            node_edges[child as usize].push(e as u32);
            if parent != child {
                node_edges[parent as usize].push(e as u32);
                if layout.edge_first_parent[e] && first_parent[child as usize] == NO_NODE {
                    first_parent[child as usize] = parent;
                }
            }
            let mut chain = vec![child];
            for (k, p) in pts[1..pts.len() - 1].iter().enumerate() {
                let id = layout.edge_bends.get(e).and_then(|b| b.get(k)).copied();
                let particle = match id.and_then(|id| bend_particle.get(&id)) {
                    Some(&particle) => particle,
                    None => {
                        let particle = origin.len() as u32;
                        origin.push(*p);
                        half.push(Point::default());
                        bend_edges.push(0);
                        layer_of.push(layout.layers[child as usize] + 1 + k as u32);
                        if let Some(id) = id {
                            bend_particle.insert(id, particle);
                        }
                        particle
                    }
                };
                chain.push(particle);
            }
            chain.push(parent);
            // Count each edge once per bend point, even if it passes it twice.
            let mut bends: Vec<u32> = chain[1..chain.len() - 1].to_vec();
            bends.sort_unstable();
            bends.dedup();
            for p in bends {
                bend_edges[p as usize - n] += 1;
            }
            for w in chain.windows(2) {
                if seen_segments.insert((w[0], w[1])) {
                    springs.push(Spring {
                        a: w[0],
                        b: w[1],
                        offset: sub(origin[w[1] as usize], origin[w[0] as usize]),
                        stiffness: 1.0,
                        along_edge: true,
                    });
                }
            }
            chains.push(chain);
        }

        // Link each particle to its neighbours within its layer: the "weft" of the net.
        let layer_count = layer_of.iter().map(|&l| l as usize + 1).max().unwrap_or(0);
        let mut by_layer: Vec<Vec<u32>> = vec![Vec::new(); layer_count];
        for (i, &l) in layer_of.iter().enumerate() {
            by_layer[l as usize].push(i as u32);
        }
        let vertical = layout.direction.is_vertical();
        let along = |p: Point| if vertical { p.x } else { p.y };
        for layer in &mut by_layer {
            layer.sort_by(|&a, &b| along(origin[a as usize]).total_cmp(&along(origin[b as usize])));
            for w in layer.windows(2) {
                springs.push(Spring {
                    a: w[0],
                    b: w[1],
                    offset: sub(origin[w[1] as usize], origin[w[0] as usize]),
                    stiffness: 0.5,
                    along_edge: false,
                });
            }
        }

        // Bundles: edges whose last bend points are shared (all edges sharing a bend point run
        // into the same parent). The longest shared run is the trunk.
        let mut trunks: Vec<Vec<u32>> = Vec::new();
        let mut bundle = vec![NO_BUNDLE; chains.len()];
        let mut bundle_at: HashMap<u32, u32> = HashMap::new();
        fn shared_run<'a>(chain: &'a [u32], bend_edges: &[u32], n: usize) -> &'a [u32] {
            let bends = &chain[1..chain.len() - 1];
            let k = bends
                .iter()
                .rev()
                .take_while(|&&p| bend_edges[p as usize - n] > 1)
                .count();
            &bends[bends.len() - k..]
        }
        for (e, chain) in chains.iter().enumerate() {
            let run = shared_run(chain, &bend_edges, n);
            let parent = chain[chain.len() - 1];
            if run.is_empty() || parent == chain[0] {
                continue;
            }
            let b = *bundle_at.entry(parent).or_insert_with(|| {
                trunks.push(Vec::new());
                trunks.len() as u32 - 1
            });
            if run.len() > trunks[b as usize].len() {
                trunks[b as usize] = run.to_vec();
            }
            bundle[e] = b;
        }
        let mut bundle_edges = vec![Vec::new(); trunks.len()];
        for (e, chain) in chains.iter().enumerate() {
            let b = bundle[e];
            if b != NO_BUNDLE {
                if trunks[b as usize].ends_with(shared_run(chain, &bend_edges, n)) {
                    bundle_edges[b as usize].push(e as u32);
                } else {
                    bundle[e] = NO_BUNDLE;
                }
            }
        }

        let count = origin.len();
        let adjacent = SpringsAt::new(count, &springs);
        Net {
            node_count: n,
            origin,
            home: vec![Point::default(); count],
            displaced: 0,
            disp: vec![Point::default(); count],
            vel: vec![Point::default(); count],
            target: vec![Point::default(); count],
            half,
            before: vec![Point::default(); count],
            vertical,
            flow: layout.direction.flow(),
            moved: vec![false; n],
            springs,
            adjacent,
            chains,
            node_edges,
            first_parent,
            bend_edges,
            grab: None,
            held: vec![false; count],
            awake: false,
            wobbly: false,
            active: Vec::new(),
            is_active: vec![false; count],
            active_changes: 0,
            segments: FlowSegments::default(),
            flow_stuck: vec![0; count],
            separated: vec![0; count],
            grid: Grid::default(),
            magnets: vec![Vec::new(); n],
            magnet_nodes: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            trunks,
            bundle_edges,
            bundle,
            trunk_cache: HashMap::new(),
            routes: vec![None; layout.edges.len()],
            turned: vec![false; layout.edges.len()],
            obstacles: None,
            placed_at: vec![Point::default(); n],
            settling: false,
        }
    }

    pub fn node_count(&self) -> usize {
        self.node_count
    }

    fn pos(&self, i: usize) -> Point {
        add(self.origin[i], self.disp[i])
    }

    pub fn node_pos(&self, node: usize) -> Point {
        self.pos(node)
    }

    /// Current points of edge `e`: child centre, bend points, parent centre. Edges whose nodes
    /// have moved relative to each other, or that a moved node now covers, are routed afresh
    /// around the nodes (see [`crate::route`]); the others keep the layout's route.
    pub fn edge_points(&self, e: usize) -> impl ExactSizeIterator<Item = Point> + '_ {
        let chain = &self.chains[e];
        let pts: Vec<Point> = match &self.routes[e] {
            Some(route) => {
                let (c, p) = (chain[0] as usize, chain[chain.len() - 1] as usize);
                let trunk = match route.join {
                    Some(j) => &self.trunks[self.bundle[e] as usize][j as usize..],
                    None => &[],
                };
                std::iter::once(self.pos(c))
                    .chain(route.bends.iter().copied())
                    .chain(trunk.iter().map(|&t| self.pos(t as usize)))
                    .chain(std::iter::once(self.pos(p)))
                    .collect()
            }
            None => chain.iter().map(|&p| self.pos(p as usize)).collect(),
        };
        pts.into_iter()
    }

    /// How tidy the graph is now: see [`Tidiness`].
    pub fn tidiness(&self) -> Tidiness {
        let n = self.node_count;
        let mut obstacles = Obstacles::new(n);
        for i in 0..n {
            obstacles.place(i, self.pos(i), self.half[i]);
        }
        let mut out = Tidiness::default();
        let boxes: Vec<(u32, Point, Point)> = (0..n)
            .map(|i| (i as u32, self.pos(i), self.half[i]))
            .collect();
        let mut grid = Grid::default();
        grid.rebuild(&boxes);
        let mut pairs = Vec::new();
        grid.candidate_pairs(&mut pairs);
        out.overlaps = pairs
            .into_iter()
            .filter(|&(a, b)| {
                let (a, b) = (a as usize, b as usize);
                let d = sub(self.pos(b), self.pos(a));
                let ext = add(self.half[a], self.half[b]);
                d.x.abs() < ext.x - BLOCK_SLACK && d.y.abs() < ext.y - BLOCK_SLACK
            })
            .count();
        for e in 0..self.chains.len() {
            let chain = &self.chains[e];
            let ends = [chain[0], chain[chain.len() - 1]];
            if ends[0] == ends[1] {
                continue;
            }
            // As drawn: from the child's side facing its parents to the parent's other side.
            let mut pts: Vec<Point> = self.edge_points(e).collect();
            let last = pts.len() - 1;
            pts[0] = self.exit(ends[0] as usize);
            pts[last] = self.entry(ends[1] as usize);
            let mut through = false;
            for w in pts.windows(2) {
                let (origin, disp, half) = (&self.origin, &self.disp, &self.half);
                obstacles.near_segment(w[0], w[1], |i| {
                    if through || ends.contains(&i) {
                        return;
                    }
                    let i = i as usize;
                    let c = add(origin[i], disp[i]);
                    // Cutting a corner, as layout routes may, doesn't count.
                    let h = Point::new(half[i].x - OVERLAP_MARGIN, half[i].y - OVERLAP_MARGIN);
                    through = route::entry(w[0], w[1], sub(c, h), add(c, h)).is_some();
                });
            }
            out.edges_through_boxes += usize::from(through);
            // Turned-around edges run round their nodes by design.
            if !self.is_reversed(e) {
                let back: f32 = pts
                    .windows(2)
                    .map(|w| (-dot(sub(w[1], w[0]), self.flow)).max(0.0))
                    .sum();
                out.edges_doubling_back += usize::from(back > route::TURN);
                let length: f32 = pts.windows(2).map(|w| len(sub(w[1], w[0]))).sum();
                let direct = len(sub(pts[pts.len() - 1], pts[0]));
                out.edges_detouring += usize::from(length > 1.5 * direct + DETOUR_SLACK);
            }
        }
        out
    }

    /// True if edge `e` follows a route of its own instead of the layout's.
    pub fn is_rerouted(&self, e: usize) -> bool {
        self.routes[e].is_some()
    }

    /// True if edge `e` has a route made for an edge turned around (see [`Net::is_reversed`]):
    /// its first bend point lies straight on from the child along the direction of history,
    /// its last one straight before the parent, and the route runs round both between them.
    pub fn turns(&self, e: usize) -> bool {
        self.turned[e]
    }

    /// True if edge `e` now runs against the direction of history: its parent has been moved
    /// before its child. It still leaves the child and enters the parent on the usual sides,
    /// and its route (if it has one) starts and ends with the turns round them.
    pub fn is_reversed(&self, e: usize) -> bool {
        let chain = &self.chains[e];
        let (c, p) = (chain[0] as usize, chain[chain.len() - 1] as usize);
        dot(sub(self.pos(p), self.pos(c)), self.flow) < 0.0
    }

    /// True if the user moved `node` by hand (and has not returned it to the layout).
    pub fn is_moved(&self, node: usize) -> bool {
        self.moved[node]
    }

    /// True if `node` rests somewhere else than in the layout.
    pub fn is_displaced(&self, node: usize) -> bool {
        self.home[node] != Point::default()
    }

    /// True if anything rests somewhere else than in the layout.
    pub fn any_displaced(&self) -> bool {
        self.displaced > 0
    }

    fn set_home(&mut self, p: usize, h: Point) {
        let zero = Point::default();
        match (self.home[p] == zero, h == zero) {
            (true, false) => self.displaced += 1,
            (false, true) => self.displaced -= 1,
            _ => {}
        }
        self.home[p] = h;
    }

    /// True while the simulation still has motion to show.
    pub fn is_awake(&self) -> bool {
        self.awake || self.grab.is_some()
    }

    /// The node under the pointer while dragging.
    pub fn grabbed(&self) -> Option<usize> {
        self.grab.as_ref().map(|g| g.anchor as usize)
    }

    /// Number of particles currently simulated.
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Starts dragging `nodes` (usually including `anchor`, the node under the pointer), and
    /// with them the `carried` nodes. They all move rigidly together; when dropped, `nodes` and
    /// `anchor` count as moved by hand. If `adapt` is true the rest of the net gives way;
    /// otherwise it stays still and only the edges to the dragged nodes stretch.
    pub fn grab(&mut self, anchor: usize, nodes: &[usize], carried: &[usize], adapt: bool) {
        self.cancel_grab();
        let n = self.node_count;
        let mut marked: Vec<u32> = nodes
            .iter()
            .chain([&anchor])
            .filter(|&&node| node < n)
            .map(|&node| node as u32)
            .collect();
        marked.sort_unstable();
        marked.dedup();
        let mut held: Vec<u32> = Vec::new();
        let carried = carried.iter().filter(|&&c| c < n).map(|&c| c as u32);
        for node in marked.iter().copied().chain(carried) {
            if !self.held[node as usize] {
                self.held[node as usize] = true;
                held.push(node);
            }
        }
        // Edges at held nodes. Bend points whose edges all run between held nodes are held too.
        let mut edges: Vec<u32> = held
            .iter()
            .flat_map(|&node| self.node_edges[node as usize].iter().copied())
            .collect();
        edges.sort_unstable();
        edges.dedup();
        let mut internal: HashMap<u32, u32> = HashMap::new();
        for &e in &edges {
            let chain = &self.chains[e as usize];
            let (a, b) = (chain[0] as usize, chain[chain.len() - 1] as usize);
            if self.held[a] && self.held[b] {
                let mut bends: Vec<u32> = chain[1..chain.len() - 1].to_vec();
                bends.sort_unstable();
                bends.dedup();
                for p in bends {
                    *internal.entry(p).or_default() += 1;
                }
            }
        }
        for (p, k) in internal {
            if k == self.bend_edges[p as usize - n] {
                self.held[p as usize] = true;
                held.push(p);
            }
        }
        let stretched: Vec<u32> = edges
            .into_iter()
            .filter(|&e| {
                let chain = &self.chains[e as usize];
                chain[1..chain.len() - 1]
                    .iter()
                    .any(|&p| !self.held[p as usize])
            })
            .collect();
        let mut stretched_bends: Vec<u32> = stretched
            .iter()
            .flat_map(|&e| {
                let chain = &self.chains[e as usize];
                chain[1..chain.len() - 1].iter().copied()
            })
            .filter(|&p| !self.held[p as usize])
            .collect();
        stretched_bends.sort_unstable();
        stretched_bends.dedup();

        for &p in &held {
            self.activate(p as usize);
        }
        if adapt {
            self.wake_around(&held, ACTIVE_BUDGET);
        } else {
            for &p in &stretched_bends {
                self.activate(p as usize);
            }
        }
        self.wobbly = adapt;
        self.awake = true;
        self.grab = Some(Grab {
            anchor: anchor as u32,
            anchor_start: self.disp[anchor],
            held: held.iter().map(|&p| (p, self.disp[p as usize])).collect(),
            marked,
            stretched,
            stretched_bends,
            delta: Point::default(),
            adapt,
        });
    }

    /// Moves the node under the pointer's centre to `target`; the other dragged nodes keep
    /// their offsets from it.
    pub fn drag_to(&mut self, target: Point) {
        if let Some(g) = &mut self.grab {
            let at = sub(target, self.origin[g.anchor as usize]);
            g.delta = sub(at, g.anchor_start);
            self.awake = true;
        }
    }

    /// Drops the dragged nodes where they are. The shape the net has taken becomes its new
    /// resting shape, and the dragged nodes are marked as moved. Undoable.
    pub fn release(&mut self, params: &NetParams) {
        match &self.grab {
            None => return,
            // Let go without moving: nothing changes.
            Some(g) if len(g.delta) <= REST_EPSILON => {
                self.cancel_grab();
                return;
            }
            Some(_) => {}
        }
        self.shape(params, FINAL_SWEEPS);
        let grab = self.grab.take().expect("grab exists");
        let mut change = Change::default();
        for k in 0..self.active.len() {
            let p = self.active[k] as usize;
            let t = self.target[p];
            if len(sub(t, self.home[p])) > REST_EPSILON {
                change.homes.push((p as u32, self.home[p], t));
                self.set_home(p, t);
            }
            self.target[p] = self.home[p];
        }
        for &(p, _) in &grab.held {
            self.held[p as usize] = false;
        }
        for &node in &grab.marked {
            if !self.moved[node as usize] {
                self.moved[node as usize] = true;
                change.marks.push((node, false, true));
            }
        }
        self.record(change);
        self.awake = true;
    }

    /// Ends a drag without keeping anything from it.
    fn cancel_grab(&mut self) {
        if let Some(grab) = self.grab.take() {
            for &(p, _) in &grab.held {
                self.held[p as usize] = false;
            }
            self.awake = true;
        }
    }

    fn record(&mut self, change: Change) {
        if change.is_empty() {
            return;
        }
        self.undo.push(change);
        self.redo.clear();
        // Forget the oldest steps beyond the limits (but always keep the last one).
        let size = |c: &Change| c.homes.len() + c.marks.len();
        let mut total: usize = self.undo.iter().map(size).sum();
        let mut drop = 0;
        while self.undo.len() - drop > 1
            && (self.undo.len() - drop > UNDO_LIMIT || total > UNDO_ENTRIES)
        {
            total -= size(&self.undo[drop]);
            drop += 1;
        }
        self.undo.drain(..drop);
    }

    /// Applies a change (or reverts it); the particles concerned move there smoothly.
    fn apply(&mut self, change: &Change, forward: bool) {
        for &(p, before, after) in &change.homes {
            self.set_home(p as usize, if forward { after } else { before });
            self.activate(p as usize);
        }
        for &(node, before, after) in &change.marks {
            self.moved[node as usize] = if forward { after } else { before };
        }
        self.wobbly = false;
        self.awake = true;
    }

    /// Reverts the last drop, reset or return to the layout (ending any drag). Returns false if
    /// there is none.
    pub fn undo(&mut self) -> bool {
        let Some(change) = self.undo.pop() else {
            return false;
        };
        self.cancel_grab();
        self.apply(&change, false);
        self.redo.push(change);
        true
    }

    /// Repeats the last undone change (ending any drag). Returns false if there is none.
    pub fn redo(&mut self) -> bool {
        let Some(change) = self.redo.pop() else {
            return false;
        };
        self.cancel_grab();
        self.apply(&change, true);
        self.undo.push(change);
        true
    }

    /// Sends everything back to the layout. Undoable.
    pub fn reset(&mut self) {
        self.cancel_grab();
        let mut change = Change::default();
        for (p, &h) in self.home.iter().enumerate() {
            if h != Point::default() {
                change.homes.push((p as u32, h, Point::default()));
            }
        }
        for (node, &moved) in self.moved.iter().enumerate() {
            if moved {
                change.marks.push((node as u32, true, false));
            }
        }
        self.apply(&change, true);
        self.record(change);
    }

    /// Sends `nodes` back to their layout positions; nothing else moves, the edges at them
    /// stretch. Undoable.
    pub fn return_to_layout(&mut self, nodes: &[usize]) {
        self.cancel_grab();
        let by: HashMap<u32, Point> = nodes
            .iter()
            .filter(|&&node| node < self.node_count)
            .map(|&node| (node as u32, scale(self.home[node], -1.0)))
            .collect();
        let mut change = Change {
            homes: self.shifted_homes(&by),
            marks: Vec::new(),
        };
        for &node in by.keys() {
            if self.moved[node as usize] {
                change.marks.push((node, true, false));
            }
        }
        self.apply(&change, true);
        self.record(change);
    }

    /// Where nodes that rest away from the layout or were moved by hand rest, and the children
    /// of those that rest away (so that [`Net::restore`] can tell them from new commits). For
    /// saving; see [`Net::restore`].
    pub fn rest_places(&self) -> impl Iterator<Item = RestPlace> + '_ {
        let rest = |i: usize| add(self.origin[i], self.home[i]);
        (0..self.node_count)
            .filter(|&i| {
                self.moved[i]
                    || self.is_displaced(i)
                    || self.parents(i).any(|p| self.is_displaced(p as usize))
            })
            .map(move |i| {
                let first = self.first_parent[i];
                RestPlace {
                    node: i,
                    offset: self.home[i],
                    moved: self.moved[i],
                    beside: (first != NO_NODE)
                        .then(|| (first as usize, sub(rest(i), rest(first as usize)))),
                }
            })
    }

    /// The parents of `node`.
    fn parents(&self, node: usize) -> impl Iterator<Item = u32> + '_ {
        let node = node as u32;
        self.edge_ends(node)
            .filter_map(move |(child, parent)| (child == node).then_some(parent))
    }

    /// The children of `node`.
    fn children(&self, node: usize) -> impl Iterator<Item = u32> + '_ {
        let node = node as u32;
        self.edge_ends(node)
            .filter_map(move |(child, parent)| (parent == node).then_some(child))
    }

    /// (child, parent) of every edge at `node`, leaving out loops.
    fn edge_ends(&self, node: u32) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.node_edges[node as usize].iter().filter_map(|&e| {
            let chain = &self.chains[e as usize];
            let (child, parent) = (chain[0], chain[chain.len() - 1]);
            (parent != child).then_some((child, parent))
        })
    }

    /// Puts nodes back where they rested at once (the edges at them follow): those that rested
    /// away from the layout beside their first parent as they were, if they still have it
    /// ([`RestPlace::beside`]), else at their rest offset from the layout. A node that was not
    /// saved but has a parent resting away from the layout is a new commit (see
    /// [`Net::rest_places`]); it keeps its place beside its first parent, taking on its
    /// displacement, and moves on against the flow if that leaves it not clear above a parent.
    /// The layout may have changed since the places were saved, so the graph then settles
    /// where nodes overlap (see [`Net::settle`]). Meant for a fresh net; not recorded for
    /// undo.
    pub fn restore(&mut self, saved: impl IntoIterator<Item = RestPlace>, params: &NetParams) {
        self.cancel_grab();
        let n = self.node_count;
        let finite = |p: Point| p.x.is_finite() && p.y.is_finite();
        let mut saved: Vec<RestPlace> = saved
            .into_iter()
            .filter(|s| s.node < n && finite(s.offset))
            .collect();
        // Parents first (they lie farther along the flow), so that a node going back beside
        // its first parent finds it where it has gone.
        let along_flow = |i: usize| dot(self.origin[i], self.flow);
        saved.sort_by(|a, b| along_flow(b.node).total_cmp(&along_flow(a.node)));
        let mut by: HashMap<u32, Point> = HashMap::new();
        for s in &saved {
            let i = s.node;
            self.moved[i] = s.moved;
            // A node resting in the layout stays in the layout.
            let placed = s.offset != Point::default();
            let rest = match s.beside {
                Some((parent, apart))
                    if placed
                        && parent < n
                        && self.first_parent[i] == parent as u32
                        && finite(apart) =>
                {
                    let delta = by.get(&(parent as u32)).copied().unwrap_or_default();
                    add(
                        add(add(self.origin[parent], self.home[parent]), delta),
                        apart,
                    )
                }
                _ => add(self.origin[i], s.offset),
            };
            by.insert(i as u32, sub(sub(rest, self.origin[i]), self.home[i]));
        }
        self.place_new_nodes(&mut by);
        if by.values().all(|&d| d == Point::default()) {
            // Everything rests in the layout already, with the layout's routes.
            return;
        }
        for (p, _, after) in self.shifted_homes(&by) {
            let p = p as usize;
            self.set_home(p, after);
            self.disp[p] = after;
            self.target[p] = after;
            self.vel[p] = Point::default();
        }
        self.settle(params);
        // Everything may have moved at once: route from scratch.
        self.obstacles = None;
        let all: Vec<u32> = (0..self.node_count as u32).collect();
        self.update_routes(&all);
        self.route_blocked_edges();
    }

    /// Lets the graph settle at once where nodes resting away from the layout overlap others:
    /// the shape a drop of the nodes moved by hand would leave in [`DragModel::Adapt`], with
    /// overlapping boxes pushed apart and children kept above their parents. Nodes moved by
    /// hand stay put unless they leave no room between them; the others give way, and rest
    /// where they end up.
    fn settle(&mut self, params: &NetParams) {
        let overlapping = self.overlapping();
        let Some(&anchor) = overlapping.first() else {
            return;
        };
        let held: Vec<(u32, Point)> = (0..self.node_count as u32)
            .filter(|&i| self.moved[i as usize] && self.is_displaced(i as usize))
            .map(|i| (i, self.disp[i as usize]))
            .collect();
        for &(p, _) in &held {
            self.held[p as usize] = true;
        }
        self.wake_around(&overlapping, SETTLE_WAKE);
        self.settling = true;
        self.grab = Some(Grab {
            anchor,
            anchor_start: self.disp[anchor as usize],
            held,
            marked: Vec::new(),
            stretched: Vec::new(),
            stretched_bends: Vec::new(),
            delta: Point::default(),
            adapt: true,
        });
        let params = NetParams {
            model: DragModel::Adapt,
            avoid_overlap: true,
            ..params.clone()
        };
        // Nodes pushed aside may come up against others: look again each round, as each frame
        // of a drag does.
        for _ in 0..SETTLE_ROUNDS {
            self.shape(&params, SWEEPS);
        }
        // A row of nodes pushed along it clears one pair per sweep, while the springs pull it
        // back between sweeps: finish by separating alone, keeping children above parents, for
        // as long as that clears overlaps. If some are left (a node wedged between nodes moved
        // by hand), those give way too. The edges at the nodes it moves bend along.
        let n = self.node_count;
        let start: Vec<(u32, Point)> = self
            .active
            .iter()
            .filter(|&&i| (i as usize) < n)
            .map(|&i| (i, self.target[i as usize]))
            .collect();
        let (mut least, mut stalled) = (f32::INFINITY, 0);
        for _ in 0..SEPARATE_ROUNDS {
            let near = self.near_pairs(OVERLAP_MARGIN);
            let mut target = std::mem::take(&mut self.target);
            let overlap: f32 = near
                .iter()
                .map(|pair| (-self.gap(pair, &target, pair.margin()).0).max(0.0))
                .sum();
            if overlap < 0.99 * least {
                (least, stalled) = (overlap, 0);
            } else {
                stalled += 1;
            }
            let mut done = overlap <= REST_EPSILON || stalled >= SEPARATE_PATIENCE;
            if done && overlap > REST_EPSILON {
                let grab = self.grab.as_ref().expect("settling");
                if grab.held.iter().any(|&(p, _)| self.held[p as usize]) {
                    for &(p, _) in &grab.held {
                        self.held[p as usize] = false;
                    }
                    (least, stalled, done) = (overlap, 0, false);
                }
            }
            if !done {
                self.separate_nodes(&mut target, &near);
                let segments = self.flow_segments();
                self.keep_flow_order(&segments, true, &mut target);
            }
            self.target = target;
            if done {
                break;
            }
        }
        self.clear_along_layers();
        let by: HashMap<u32, Point> = start
            .into_iter()
            .map(|(i, was)| (i, sub(self.target[i as usize], was)))
            .filter(|&(_, d)| len(d) > REST_EPSILON)
            .collect();
        let mut edges: Vec<u32> = by
            .keys()
            .flat_map(|&i| self.node_edges[i as usize].iter().copied())
            .collect();
        edges.sort_unstable();
        edges.dedup();
        for (p, shift) in self.bend_shifts(&edges, |i| by.get(&i).copied().unwrap_or_default()) {
            let p = p as usize;
            self.activate(p);
            self.target[p] = add(self.target[p], shift);
        }
        self.settling = false;
        let grab = self.grab.take().expect("grab exists");
        for &(p, _) in &grab.held {
            self.held[p as usize] = false;
        }
        for k in 0..self.active.len() {
            let p = self.active[k] as usize;
            let t = self.target[p];
            if len(sub(t, self.home[p])) > REST_EPSILON {
                self.set_home(p, t);
            }
            self.target[p] = self.home[p];
        }
        self.deactivate_all();
        self.awake = false;
    }

    /// Clears whatever overlap settling leaves (where it and the order along the flow can't
    /// both be cleared) by pushing nodes along their layer, which never turns an edge around:
    /// of each overlapping pair, the one farther along moves on.
    fn clear_along_layers(&mut self) {
        let (along, _) = self.axes();
        for _ in 0..self.node_count {
            let near = self.near_pairs(OVERLAP_MARGIN);
            let mut pushed = false;
            for pair in &near {
                let (a, b) = (pair.a as usize, pair.b as usize);
                let place = |i: usize| add(self.origin[i], self.target[i]);
                let d = sub(place(b), place(a));
                let ext = add(self.half[a], self.half[b]);
                if d.x.abs() >= ext.x - BLOCK_SLACK || d.y.abs() >= ext.y - BLOCK_SLACK {
                    continue;
                }
                let apart = dot(d, along);
                let last = if apart < 0.0 { a } else { b };
                let short = dot(ext, along) + OVERLAP_MARGIN - apart.abs();
                self.activate(last);
                self.target[last] = add(self.target[last], scale(along, short));
                pushed = true;
            }
            if !pushed {
                break;
            }
        }
    }

    /// Nodes whose boxes overlap at rest (more than grazing), where at least one of the two
    /// rests away from the layout (the layout itself leaves none).
    fn overlapping(&mut self) -> Vec<u32> {
        let n = self.node_count;
        let rest = |i: usize| add(self.origin[i], self.home[i]);
        let boxes: Vec<(u32, Point, Point)> =
            (0..n).map(|i| (i as u32, rest(i), self.half[i])).collect();
        self.grid.rebuild(&boxes);
        let mut pairs = Vec::new();
        self.grid.candidate_pairs(&mut pairs);
        let mut out: Vec<u32> = pairs
            .into_iter()
            .filter(|&(a, b)| {
                let (a, b) = (a as usize, b as usize);
                let d = sub(rest(b), rest(a));
                let ext = add(self.half[a], self.half[b]);
                (self.is_displaced(a) || self.is_displaced(b))
                    && d.x.abs() < ext.x - BLOCK_SLACK
                    && d.y.abs() < ext.y - BLOCK_SLACK
            })
            .flat_map(|(a, b)| [a, b])
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Adds the new commits among the nodes not in `by` (see [`Net::restore`]) to `by`, which
    /// holds how far the saved nodes move.
    fn place_new_nodes(&self, by: &mut HashMap<u32, Point>) {
        let zero = Point::default();
        let rest = |by: &HashMap<u32, Point>, p: u32| {
            add(
                self.home[p as usize],
                by.get(&p).copied().unwrap_or_default(),
            )
        };
        // The new commits: children of saved nodes that rest away, and their children in turn.
        let mut new_commits: Vec<u32> = Vec::new();
        let mut seen: HashSet<u32> = HashSet::new();
        let mut queue: Vec<u32> = by
            .keys()
            .copied()
            .filter(|&p| rest(by, p) != zero)
            .collect();
        while let Some(p) = queue.pop() {
            for c in self.children(p as usize) {
                if !by.contains_key(&c) && seen.insert(c) {
                    new_commits.push(c);
                    queue.push(c);
                }
            }
        }
        // Parents first: they lie farther along the flow.
        let along_flow = |p: u32| dot(self.origin[p as usize], self.flow);
        new_commits.sort_unstable_by(|&a, &b| along_flow(b).total_cmp(&along_flow(a)));
        for c in new_commits {
            let ci = c as usize;
            let first = self.first_parent[ci];
            let mut d = if first == NO_NODE {
                self.home[ci]
            } else {
                rest(by, first)
            };
            for p in self.parents(ci) {
                let pi = p as usize;
                let apart = dot(sub(self.origin[pi], self.origin[ci]), self.flow);
                if apart <= 0.0 {
                    continue;
                }
                let needed = self.flow_gap(ci, pi, apart);
                let have = apart + dot(sub(rest(by, p), d), self.flow);
                if have < needed {
                    d = sub(d, scale(self.flow, needed - have));
                }
            }
            by.insert(c, sub(d, self.home[ci]));
        }
    }

    /// Rest positions after moving the given nodes by the given amounts, with the bend points
    /// of their edges moved proportionally: (particle, before, after).
    fn shifted_homes(&self, by: &HashMap<u32, Point>) -> Vec<(u32, Point, Point)> {
        let mut out: Vec<(u32, Point, Point)> = by
            .iter()
            .filter(|&(_, &d)| d != Point::default())
            .map(|(&node, &d)| {
                let h = self.home[node as usize];
                (node, h, add(h, d))
            })
            .collect();
        let mut edges: Vec<u32> = by
            .keys()
            .flat_map(|&node| self.node_edges[node as usize].iter().copied())
            .collect();
        edges.sort_unstable();
        edges.dedup();
        let shifts = self.bend_shifts(&edges, |node| by.get(&node).copied().unwrap_or_default());
        for (p, s) in shifts {
            let h = self.home[p as usize];
            if s != Point::default() {
                out.push((p, h, add(h, s)));
            }
        }
        out
    }

    /// How far the bend points of `edges` move when their end nodes move by `shift_of`: each
    /// edge's bend points share the movement of its ends in proportion to their distance along
    /// the edge. A bend point shared by several (bundled) edges gets the average over all of
    /// them.
    fn bend_shifts(&self, edges: &[u32], shift_of: impl Fn(u32) -> Point) -> HashMap<u32, Point> {
        let n = self.node_count;
        let mut out: HashMap<u32, Point> = HashMap::new();
        let rest = |p: u32| (self.origin[p as usize], self.home[p as usize]);
        let mut run = Vec::new();
        for &e in edges {
            let chain = &self.chains[e as usize];
            let last = chain.len() - 1;
            let (sa, sb) = (shift_of(chain[0]), shift_of(chain[last]));
            if last < 2 || (sa == Point::default() && sb == Point::default()) {
                continue;
            }
            // Distance along the edge's resting shape.
            run.clear();
            run.push(0.0f32);
            for w in chain.windows(2) {
                let ((oa, ha), (ob, hb)) = (rest(w[0]), rest(w[1]));
                let step = len(add(sub(ob, oa), sub(hb, ha)));
                run.push(run[run.len() - 1] + step);
            }
            let total = run[last];
            for (k, &p) in chain.iter().enumerate().take(last).skip(1) {
                let t = if total > 0.0 {
                    run[k] / total
                } else {
                    k as f32 / last as f32
                };
                let s = add(scale(sa, 1.0 - t), scale(sb, t));
                let share = scale(s, 1.0 / self.bend_edges[p as usize - n] as f32);
                let slot = out.entry(p).or_default();
                *slot = add(*slot, share);
            }
        }
        out
    }

    fn activate(&mut self, p: usize) {
        let t = self.target[p];
        self.activate_heading(p, t);
    }

    /// Wakes particle `p`, which is heading for `target` (for use while `self.target` is
    /// being relaxed elsewhere).
    fn activate_heading(&mut self, p: usize, target: Point) {
        if !self.is_active[p] {
            self.is_active[p] = true;
            self.before[p] = target;
            self.active.push(p as u32);
            self.active_changes += 1;
        }
    }

    /// Wakes the particles nearest to `from` (breadth-first over springs), up to `budget`
    /// more than are awake already.
    fn wake_around(&mut self, from: &[u32], budget: usize) {
        self.awake = true;
        let limit = self.active.len() + budget;
        let mut queue: VecDeque<usize> = from.iter().map(|&p| p as usize).collect();
        let mut seen: HashSet<usize> = queue.iter().copied().collect();
        while let Some(q) = queue.pop_front() {
            if self.active.len() >= limit {
                break;
            }
            self.activate(q);
            for &s in self.adjacent.get(q) {
                let s = self.springs[s as usize];
                let other = if s.a as usize == q { s.b } else { s.a } as usize;
                if seen.insert(other) {
                    queue.push_back(other);
                }
            }
        }
    }

    fn deactivate_all(&mut self) {
        for &p in &self.active {
            let p = p as usize;
            self.is_active[p] = false;
            self.vel[p] = Point::default();
            self.disp[p] = self.target[p];
            self.flow_stuck[p] = 0;
        }
        self.active.clear();
        self.active_changes += 1;
    }

    /// Advances the simulation by `dt` seconds. Returns true while anything is still moving.
    pub fn step(&mut self, dt: f32, params: &NetParams) -> bool {
        if !self.is_awake() {
            return false;
        }
        let dt = dt.clamp(1.0 / 240.0, 1.0 / 30.0);
        self.shape(params, SWEEPS);

        // Move towards the targets (damped springs, semi-implicit Euler). Held particles, and
        // the stretched edges of a drag that does not adapt, follow exactly.
        if let Some(g) = &self.grab
            && !g.adapt
        {
            for &p in &g.stretched_bends {
                self.disp[p as usize] = self.target[p as usize];
                self.vel[p as usize] = Point::default();
            }
        }
        let zeta = if self.wobbly {
            1.0 - 0.75 * params.wobble.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let omega = std::f32::consts::TAU * FOLLOW_HZ;
        let h = dt / SUBSTEPS as f32;
        let mut max_speed = 0.0f32;
        let mut max_distance = 0.0f32;
        for &i in &self.active {
            let i = i as usize;
            if self.held[i] {
                self.disp[i] = self.target[i];
                self.vel[i] = Point::default();
                continue;
            }
            for _ in 0..SUBSTEPS {
                let pull = scale(sub(self.target[i], self.disp[i]), omega * omega);
                let drag = scale(self.vel[i], 2.0 * zeta * omega);
                self.vel[i] = add(self.vel[i], scale(sub(pull, drag), h));
                self.disp[i] = add(self.disp[i], scale(self.vel[i], h));
            }
            max_speed = max_speed.max(len(self.vel[i]));
            max_distance = max_distance.max(len(sub(self.target[i], self.disp[i])));
        }
        self.awake = max_speed > SLEEP_SPEED || max_distance > SLEEP_DISTANCE;
        let settled = !self.awake && self.grab.is_none();
        let moved: Vec<u32> = self
            .active
            .iter()
            .copied()
            .filter(|&i| {
                (i as usize) < self.node_count
                    && len(sub(self.disp[i as usize], self.placed_at[i as usize])) > ROUTE_STEP
            })
            .collect();
        if settled {
            self.deactivate_all();
        }
        self.update_routes(&moved);
        if settled {
            self.route_blocked_edges();
        }
        self.is_awake()
    }

    /// Sets up the node boxes for routing, if not done yet.
    fn obstacles(&mut self) -> &mut Obstacles {
        if self.obstacles.is_none() {
            let mut o = Obstacles::new(self.node_count);
            for i in 0..self.node_count {
                o.place(i, self.pos(i), self.half[i]);
                self.placed_at[i] = self.disp[i];
            }
            self.obstacles = Some(o);
        }
        self.obstacles.as_mut().expect("just set up")
    }

    /// Brings routes up to date after the `moved` nodes have moved: those of their edges, and
    /// those of routed edges passing where they were or are now. (An edge a node has just left
    /// may have been routed round it; its route has to be looked at again even if the node has
    /// jumped clear of it in one frame.)
    fn update_routes(&mut self, moved: &[u32]) {
        if moved.is_empty() {
            return;
        }
        self.obstacles();
        self.trunk_cache.clear();
        let mut area: Option<(Point, Point)> = None;
        for &node in moved {
            let i = node as usize;
            let (centre, half) = (self.pos(i), self.half[i]);
            let mut boxes = [None, None];
            if let Some(o) = &mut self.obstacles {
                boxes[0] = o.box_of(i);
                o.place(i, centre, half);
                boxes[1] = o.box_of(i);
            }
            self.placed_at[i] = self.disp[i];
            for (lo, hi) in boxes.into_iter().flatten() {
                area = Some(match area {
                    None => (lo, hi),
                    Some((a, b)) => (
                        Point::new(a.x.min(lo.x), a.y.min(lo.y)),
                        Point::new(b.x.max(hi.x), b.y.max(hi.y)),
                    ),
                });
            }
        }
        let mut edges: Vec<u32> = moved
            .iter()
            .flat_map(|&node| self.node_edges[node as usize].iter().copied())
            .collect();
        if let Some((lo, hi)) = area {
            for e in 0..self.routes.len() {
                if self.routes[e].is_some() {
                    let (a, b) = self.span(e);
                    if a.x <= hi.x && b.x >= lo.x && a.y <= hi.y && b.y >= lo.y {
                        edges.push(e as u32);
                    }
                }
            }
        }
        self.refresh_routes(edges);
    }

    /// Refreshes the routes of `edges`, and of all edges bundled with them: they share their
    /// trunk, so they have to agree on how it runs.
    fn refresh_routes(&mut self, mut edges: Vec<u32>) {
        let bundles: Vec<u32> = edges
            .iter()
            .map(|&e| self.bundle[e as usize])
            .filter(|&b| b != NO_BUNDLE)
            .collect();
        for b in bundles {
            edges.extend_from_slice(&self.bundle_edges[b as usize]);
        }
        edges.sort_unstable();
        edges.dedup();
        for e in edges {
            self.refresh_route(e as usize);
        }
    }

    /// Bounding box of edge `e` as drawn, and of its layout route through its bend points
    /// (which [`Net::blocked`] looks at, and which a route of its own may leave far behind).
    fn span(&self, e: usize) -> (Point, Point) {
        let layout_route = self.chains[e].iter().map(|&p| self.pos(p as usize));
        let mut pts = self.edge_points(e).chain(layout_route);
        let first = pts.next().expect("edges have two ends");
        pts.fold((first, first), |(lo, hi), p| {
            (
                Point::new(lo.x.min(p.x), lo.y.min(p.y)),
                Point::new(hi.x.max(p.x), hi.y.max(p.y)),
            )
        })
    }

    /// Routes edge `e` afresh if its layout route no longer fits: its nodes have moved
    /// relative to each other, or a moved node covers it, or (for a bundled edge) its trunk
    /// has been routed afresh. Otherwise it follows the layout.
    fn refresh_route(&mut self, e: usize) {
        let chain = &self.chains[e];
        let (c, p) = (chain[0] as usize, chain[chain.len() - 1] as usize);
        let stretched = self.stretched(c, p);
        let bundle = self.bundle[e];
        let trunk = (bundle != NO_BUNDLE).then(|| self.trunk(bundle));
        let trunk_routed = matches!(trunk, Some(Trunk::Routed(_)));
        if !(stretched || trunk_routed || self.blocked(e)) {
            self.turned[e] = false;
            self.routes[e] = None;
            return;
        }
        // From where edges leave the child (its side along the direction of history) to
        // where they enter the parent (the side against it).
        let a = self.exit(c);
        let b = self.entry(p);
        let vertical = self.vertical;
        self.turned[e] = self.is_reversed(e);
        let route = if self.turned[e] {
            // Turned around: keep those sides, and run round both boxes from just past the
            // child to just before the parent (the route starts and ends with those turns).
            let a = add(a, scale(self.flow, route::TURN));
            let b = sub(b, scale(self.flow, route::TURN));
            let ends = [route::NO_END; 2];
            let mut pts = vec![a];
            pts.extend(route::route(self.obstacles(), vertical, a, b, ends));
            pts.push(b);
            Route {
                bends: pts,
                join: None,
            }
        } else {
            // A bundled edge joins its trunk at the first point far enough on from the child.
            let ahead = |q: Point| dot(sub(q, a), self.flow) >= FLOW_GAP;
            let ends = [c as u32, route::NO_END];
            match trunk {
                Some(Trunk::Layout) => {
                    let join = self.trunks[bundle as usize]
                        .iter()
                        .map(|&t| self.pos(t as usize))
                        .enumerate()
                        .find(|&(_, q)| ahead(q));
                    join.map(|(j, at)| Route {
                        bends: route::route(self.obstacles(), vertical, a, at, ends),
                        join: Some(j as u32),
                    })
                }
                Some(Trunk::Routed(pts)) => pts.iter().position(|&q| ahead(q)).map(|j| {
                    let mut bends = route::route(self.obstacles(), vertical, a, pts[j], ends);
                    bends.extend_from_slice(&pts[j..]);
                    Route { bends, join: None }
                }),
                Some(Trunk::Broken) | None => None,
            }
            .unwrap_or_else(|| Route {
                bends: route::route(self.obstacles(), vertical, a, b, [c as u32, p as u32]),
                join: None,
            })
        };
        self.routes[e] = Some(route);
    }

    /// True if nodes `c` and `p` have moved too far relative to each other for the layout's
    /// route between them. Edges at nodes moved by hand re-route as soon as they change;
    /// edges that merely gave way keep the layout's route unless pulled far out of shape.
    fn stretched(&self, c: usize, p: usize) -> bool {
        self.pulled(c, p, self.rearranged(c) || self.rearranged(p))
    }

    /// True if particles `a` and `b` have moved relative to each other by more than edges
    /// allow (less if one of them is `rearranged`).
    fn pulled(&self, a: usize, b: usize, rearranged: bool) -> bool {
        let limit = if rearranged {
            REROUTE_AFTER
        } else {
            REROUTE_ANYWAY
        };
        len(sub(self.disp[b], self.disp[a])) > limit
    }

    /// Where edges leave node `i` (its side along the direction of history).
    fn exit(&self, i: usize) -> Point {
        add(self.pos(i), scale(self.flow, self.depth(i)))
    }

    /// Where edges enter node `i` (its side against the direction of history).
    fn entry(&self, i: usize) -> Point {
        sub(self.pos(i), scale(self.flow, self.depth(i)))
    }

    /// Half the extent of node `i` along the direction of history.
    fn depth(&self, i: usize) -> f32 {
        self.flow.x.abs() * self.half[i].x + self.flow.y.abs() * self.half[i].y
    }

    /// How the edges of bundle `b` run into their parent (worked out once per routing pass).
    /// The trunk keeps the layout's route unless its parent has moved relative to it or a
    /// moved node covers it; then it runs from its first bend point along a route of its own.
    fn trunk(&mut self, b: u32) -> Trunk {
        if let Some(t) = self.trunk_cache.get(&b) {
            return t.clone();
        }
        let trunk = &self.trunks[b as usize];
        let chain = &self.chains[self.bundle_edges[b as usize][0] as usize];
        let p = chain[chain.len() - 1] as usize;
        let (first, last) = (trunk[0] as usize, trunk[trunk.len() - 1] as usize);
        let start = self.pos(first);
        let end = self.entry(p);
        let state = if dot(sub(end, start), self.flow) < FLOW_GAP {
            Trunk::Broken
        } else {
            let pts: Vec<Point> = trunk
                .iter()
                .map(|&t| self.pos(t as usize))
                .chain(std::iter::once(self.pos(p)))
                .collect();
            if self.pulled(last, p, self.rearranged(p))
                || self.covered(&pts, [route::NO_END, p as u32])
            {
                let vertical = self.vertical;
                let ends = [route::NO_END, p as u32];
                let mut pts = vec![start];
                pts.extend(route::route(self.obstacles(), vertical, start, end, ends));
                Trunk::Routed(pts)
            } else {
                Trunk::Layout
            }
        };
        self.trunk_cache.insert(b, state.clone());
        state
    }

    /// True if node `i` has been moved by hand (or is being dragged), or has been pushed far
    /// from its layout position.
    fn rearranged(&self, i: usize) -> bool {
        self.moved[i] || self.held[i] || len(self.disp[i]) > REROUTE_ANYWAY
    }

    /// True if a rearranged node covers the layout route of edge `e`.
    fn blocked(&mut self, e: usize) -> bool {
        let chain = &self.chains[e];
        let ends = [chain[0], chain[chain.len() - 1]];
        let pts: Vec<Point> = chain.iter().map(|&p| self.pos(p as usize)).collect();
        self.covered(&pts, ends)
    }

    /// True if a rearranged node other than `ends` covers the line through `pts`.
    fn covered(&mut self, pts: &[Point], ends: [u32; 2]) -> bool {
        let (origin, disp, half, moved, held) = (
            &self.origin,
            &self.disp,
            &self.half,
            &self.moved,
            &self.held,
        );
        let Some(obstacles) = &mut self.obstacles else {
            return false;
        };
        let mut hit = false;
        for w in pts.windows(2) {
            obstacles.near_segment(w[0], w[1], |i| {
                let i = i as usize;
                let rearranged = moved[i] || held[i] || len(disp[i]) > REROUTE_ANYWAY;
                if hit || ends.contains(&(i as u32)) || !rearranged || len(disp[i]) <= ROUTE_STEP {
                    return;
                }
                // The node's own box, a little smaller: grazing a corner is fine.
                let c = add(origin[i], disp[i]);
                let h = Point::new(half[i].x - BLOCK_SLACK, half[i].y - BLOCK_SLACK);
                hit = route::entry(w[0], w[1], sub(c, h), add(c, h)).is_some();
            });
            if hit {
                return true;
            }
        }
        false
    }

    /// Routes afresh every edge that a moved node now covers (once the net is at rest; while
    /// dragging, only the edges at moving nodes are looked at).
    fn route_blocked_edges(&mut self) {
        let n = self.node_count;
        if self.obstacles.is_none() || !(0..n).any(|i| self.rearranged(i)) {
            return;
        }
        self.trunk_cache.clear();
        let blocked: Vec<u32> = (0..self.routes.len())
            .filter(|&e| self.routes[e].is_none() && self.blocked(e))
            .map(|e| e as u32)
            .collect();
        self.refresh_routes(blocked);
    }

    /// Computes where every woken particle is heading.
    fn shape(&mut self, params: &NetParams, sweeps: usize) {
        let Some(grab) = &self.grab else {
            for &i in &self.active {
                self.target[i as usize] = self.home[i as usize];
            }
            return;
        };
        if !grab.adapt {
            // Everything else stays (or comes to rest) where it rests; the edges at the dragged
            // nodes stretch evenly.
            for &i in &self.active {
                let i = i as usize;
                if !self.held[i] {
                    self.target[i] = self.home[i];
                }
            }
            for &(p, start) in &grab.held {
                self.target[p as usize] = add(start, grab.delta);
            }
            let shifts = self.bend_shifts(&grab.stretched, |node| {
                sub(self.target[node as usize], self.home[node as usize])
            });
            for (p, s) in shifts {
                if !self.held[p as usize] {
                    self.target[p as usize] = add(self.home[p as usize], s);
                }
            }
            return;
        }

        // Where everything was heading at the start of the frame decides on which side of each
        // other things stay.
        for &i in &self.active {
            self.before[i as usize] = self.target[i as usize];
        }
        for &(p, start) in &grab.held {
            self.target[p as usize] = add(start, grab.delta);
        }
        let lambda = 0.3 + 0.6 * params.pull.clamp(0.0, 1.0);
        let k_anchor = (1.0 - lambda) * (1.0 - lambda) / lambda;
        let push = params.push.clamp(0.0, 1.0);
        let (range, k_magnet) = (MAGNET_RANGE * push, MAGNET_STIFFNESS * push);
        let weft_reach = range + OVERLAP_MARGIN;
        let near = if push > 0.0 || params.avoid_overlap {
            self.near_pairs(range + NEAR_SLACK)
        } else {
            Vec::new()
        };
        self.collect_magnets(&near, range, k_magnet > 0.0);

        let n = self.node_count;
        let mut segments = std::mem::take(&mut self.segments);
        if segments.built_at != Some(self.active_changes) {
            segments = self.flow_segments();
        }
        let mut target = std::mem::take(&mut self.target);
        for sweep in 0..sweeps {
            for &i in &self.active {
                let i = i as usize;
                if self.held[i] {
                    continue;
                }
                // Bend points hold on to their rest less, so edges bend before nodes move.
                let k_home = if i < n { k_anchor } else { k_anchor * 0.5 };
                let mut num = scale(self.home[i], k_home);
                let mut den = k_home;
                for &si in self.adjacent.get(i) {
                    let s = &self.springs[si as usize];
                    let (a, b) = (s.a as usize, s.b as usize);
                    let rest = sub(self.home[b], self.home[a]);
                    if !s.along_edge {
                        // Between neighbours in a layer: keep edges from being pushed into each
                        // other or through nodes (the magnets keep nodes apart). Only resist
                        // coming closer than they rest, and only once they are near.
                        if a < n && b < n {
                            continue;
                        }
                        let apart = add(s.offset, rest);
                        let dist = len(apart);
                        if dist == 0.0 {
                            continue;
                        }
                        let mut u = scale(apart, 1.0 / dist);
                        let ext = add(self.half[a], self.half[b]);
                        let reach = u.x.abs() * ext.x + u.y.abs() * ext.y + weft_reach;
                        // Keep them on the side they were on; once something has passed, it
                        // stays passed.
                        let was = |p: usize| {
                            if self.is_active[p] {
                                self.before[p]
                            } else {
                                target[p]
                            }
                        };
                        let passed = dot(add(s.offset, sub(was(b), was(a))), u) < 0.0;
                        let keep = if passed {
                            u = scale(u, -1.0);
                            reach
                        } else {
                            dist.min(reach)
                        };
                        let now = dot(add(s.offset, sub(target[b], target[a])), u);
                        if now >= keep {
                            continue;
                        }
                        let push = scale(u, keep - now);
                        let want = if i == a {
                            sub(target[a], push)
                        } else {
                            add(target[b], push)
                        };
                        num = add(num, scale(want, s.stiffness));
                        den += s.stiffness;
                        continue;
                    }
                    let want = if i == a {
                        sub(target[b], rest)
                    } else {
                        add(target[a], rest)
                    };
                    num = add(num, scale(want, s.stiffness));
                    den += s.stiffness;
                }
                if i < n {
                    for &(k, keep) in &self.magnets[i] {
                        let pair = &near[k as usize];
                        let (gap, away) = self.gap(pair, &target, 0.0);
                        if gap < keep {
                            let away = if pair.b as usize == i {
                                away
                            } else {
                                scale(away, -1.0)
                            };
                            let want = add(target[i], scale(away, keep - gap));
                            num = add(num, scale(want, k_magnet));
                            den += k_magnet;
                        }
                    }
                }
                target[i] = scale(num, 1.0 / den);
            }
            if params.avoid_overlap && sweep % 4 == 3 {
                self.separate_nodes(&mut target, &near);
            }
            // Every other sweep is enough, as long as it includes the last one and those
            // right before and after separating (which needs to know what is stuck).
            if sweep % 2 == 1 {
                let find_stuck = params.avoid_overlap && sweep % 4 == 1;
                self.keep_flow_order(&segments, find_stuck, &mut target);
            }
        }
        self.target = target;
        self.segments = segments;
        for &node in &self.magnet_nodes {
            self.magnets[node as usize].clear();
        }
        self.magnet_nodes.clear();
    }

    /// Pairs of nodes whose boxes are less than `pad` apart, at least one of them away from
    /// its rest position (two resting nodes cannot press on each other), and how they sat at
    /// the start of the frame.
    fn near_pairs(&mut self, pad: f32) -> Vec<Near> {
        let n = self.node_count;
        let place = |i: usize| add(self.origin[i], self.target[i]);
        let moving = |i: usize| {
            self.held[i]
                || len(sub(self.target[i], self.home[i])) > REST_EPSILON
                || self.settling && self.home[i] != Point::default()
        };
        // Only nodes near a moving one: the coarse cells the moving boxes (grown by `pad`)
        // touch. One box around them all would take in most of a big graph when they are
        // far apart.
        let cells_of = |p: Point, h: Point| {
            let cell = |v: f32| (v / NEAR_CELL).floor() as i32;
            let (x0, y0, x1, y1) = (
                cell(p.x - h.x),
                cell(p.y - h.y),
                cell(p.x + h.x),
                cell(p.y + h.y),
            );
            (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
        };
        let mut area: HashSet<(i32, i32)> = HashSet::new();
        for &i in &self.active {
            let i = i as usize;
            if i < n && moving(i) {
                let grown = add(self.half[i], Point::new(pad, pad));
                area.extend(cells_of(place(i), grown));
            }
        }
        if area.is_empty() {
            return Vec::new();
        }
        let items: Vec<(u32, Point, Point)> = (0..n)
            .filter(|&i| cells_of(place(i), self.half[i]).any(|c| area.contains(&c)))
            .map(|i| {
                let grown = Point::new(self.half[i].x + pad / 2.0, self.half[i].y + pad / 2.0);
                (i as u32, place(i), grown)
            })
            .collect();
        self.grid.rebuild(&items);
        let mut pairs = Vec::new();
        self.grid.candidate_pairs(&mut pairs);
        let (along, across) = self.axes();
        pairs
            .into_iter()
            .filter_map(|(a, b)| {
                let (a, b) = (a as usize, b as usize);
                if !moving(a) && !moving(b) {
                    return None;
                }
                let d = add(
                    sub(self.origin[b], self.origin[a]),
                    sub(self.target[b], self.target[a]),
                );
                let ext = add(self.half[a], self.half[b]);
                if d.x.abs() >= ext.x + pad || d.y.abs() >= ext.y + pad {
                    return None;
                }
                // Side by side in a row: they keep their order along it.
                let was = |p: usize| {
                    if self.is_active[p] {
                        self.before[p]
                    } else {
                        self.target[p]
                    }
                };
                let d = add(sub(self.origin[b], self.origin[a]), sub(was(b), was(a)));
                let in_row = dot(d, across).abs() < dot(ext, across);
                let side = match (in_row, dot(d, along) < 0.0) {
                    (false, _) => 0.0,
                    (true, true) => -1.0,
                    (true, false) => 1.0,
                };
                let rest = add(
                    sub(self.origin[b], self.origin[a]),
                    sub(self.home[b], self.home[a]),
                );
                // Settling clears overlaps left at rest too: they were not left so by hand.
                let rest_gap = box_gap(rest, ext).0;
                Some(Near {
                    a: a as u32,
                    b: b as u32,
                    side,
                    rest_gap: if self.settling {
                        rest_gap.max(OVERLAP_MARGIN)
                    } else {
                        rest_gap
                    },
                })
            })
            .collect()
    }

    /// Unit vectors along the layers and across them.
    fn axes(&self) -> (Point, Point) {
        if self.vertical {
            (Point::new(1.0, 0.0), Point::new(0.0, 1.0))
        } else {
            (Point::new(0.0, 1.0), Point::new(1.0, 0.0))
        }
    }

    /// Gap between the boxes of a pair of nodes, grown by `margin`, for displacements `t`;
    /// and the unit direction in which moving `b` widens it. Nodes side by side in a row
    /// measure it along the row, in their order.
    fn gap(&self, pair: &Near, t: &[Point], margin: f32) -> (f32, Point) {
        let (a, b) = (pair.a as usize, pair.b as usize);
        let d = add(sub(self.origin[b], self.origin[a]), sub(t[b], t[a]));
        let ext = add(add(self.half[a], self.half[b]), Point::new(margin, margin));
        if pair.side != 0.0 {
            let (along, _) = self.axes();
            let gap = dot(d, along) * pair.side - dot(ext, along);
            (gap, scale(along, pair.side))
        } else {
            box_gap(d, ext)
        }
    }

    /// Sets up this frame's magnets between the nearby pairs, and wakes resting nodes that are
    /// pressed on. A pair keeps the gap it has at rest, or `range` if that is smaller.
    fn collect_magnets(&mut self, near: &[Near], range: f32, magnets: bool) {
        for (k, pair) in near.iter().enumerate() {
            let (a, b) = (pair.a as usize, pair.b as usize);
            if self.held[a] && self.held[b] {
                continue;
            }
            let keep = pair.rest_gap.min(range);
            // Wake a resting node only once something actually presses on it.
            let now = self.gap(pair, &self.target, 0.0).0;
            let pressed = now < pair.margin() || magnets && now < keep - REST_EPSILON;
            if pressed {
                for p in [a, b] {
                    if !self.is_active[p] {
                        self.wake_around(&[p as u32], PUSH_WAKE);
                    }
                }
            }
            if magnets {
                for p in [a, b] {
                    if self.magnets[p].is_empty() {
                        self.magnet_nodes.push(p as u32);
                    }
                    self.magnets[p].push((k as u32, keep));
                }
            }
        }
    }

    /// Pushes overlapping node boxes apart (in `target` displacements): nodes side by side in
    /// a row along the row, in their order, so that they never get pushed past each other;
    /// others along their axis of least overlap. A resting node that gets hit is woken up.
    fn separate_nodes(&mut self, target: &mut [Point], near: &[Near]) {
        for pair in near {
            let (gap, away) = self.gap(pair, target, pair.margin());
            if gap >= 0.0 {
                continue;
            }
            let (a, b) = (pair.a as usize, pair.b as usize);
            self.activate_heading(a, target[a]);
            self.activate_heading(b, target[b]);
            let push = scale(away, -gap);
            let (mut wa, mut wb) = (self.yields(a, scale(push, -1.0)), self.yields(b, push));
            if wa + wb == 0.0 && self.settling && self.held[a] && self.held[b] {
                // Two nodes moved by hand, put back on each other: both give way.
                (wa, wb) = (1.0, 1.0);
            }
            if wa + wb == 0.0 {
                continue;
            }
            let (da, db) = (scale(push, -wa / (wa + wb)), scale(push, wb / (wa + wb)));
            for (i, d) in [(a, da), (b, db)] {
                target[i] = add(target[i], d);
                let along = dot(d, self.flow);
                if along > 0.0 {
                    self.separated[i] |= SEPARATED_ON;
                } else if along < 0.0 {
                    self.separated[i] |= SEPARATED_BACK;
                }
            }
        }
    }

    /// 1 if particle `i` may be pushed in direction `dir` to clear an overlap, else 0: held
    /// particles stay put, and so do those [`Net::keep_flow_order`] wedged against them in
    /// that direction (or the two would fight and the overlap would remain).
    fn yields(&self, i: usize, dir: Point) -> f32 {
        let along = dot(dir, self.flow);
        let stuck = self.flow_stuck[i];
        let blocked = self.held[i]
            || (along > 0.0 && stuck & STUCK_ON != 0)
            || (along < 0.0 && stuck & STUCK_BACK != 0);
        if blocked { 0.0 } else { 1.0 }
    }

    /// The edge segments at the woken particles that [`Net::keep_flow_order`] looks after.
    fn flow_segments(&self) -> FlowSegments {
        let mut forward = Vec::new();
        for &i in &self.active {
            for &si in self.adjacent.get(i as usize) {
                let s = &self.springs[si as usize];
                // Each segment once: from its newer end, or its older end if the newer rests.
                let from_here = s.a == i || (s.b == i && !self.is_active[s.a as usize]);
                if from_here && s.along_edge && s.a != s.b && dot(s.offset, self.flow) > 0.0 {
                    forward.push(si);
                }
            }
        }
        let sorted = |end: fn(&Spring) -> u32, sign: f32| {
            let mut keyed: Vec<(f32, u32)> = forward
                .iter()
                .map(|&si| {
                    let p = end(&self.springs[si as usize]) as usize;
                    (sign * dot(self.origin[p], self.flow), si)
                })
                .collect();
            keyed.sort_unstable_by(|x, y| x.0.total_cmp(&y.0));
            keyed.into_iter().map(|(_, si)| si).collect()
        };
        FlowSegments {
            forward: sorted(|s| s.a, 1.0),
            backward: sorted(|s| s.b, -1.0),
            built_at: Some(self.active_changes),
        }
    }

    /// Keeps every edge segment pointing along the history direction, so children stay above
    /// their parents: the older end of a segment stays at least [`FLOW_GAP`] beyond the newer
    /// one (box borders, for nodes), or as far as at rest if that is less. Segments that rest
    /// reversed (left so by the user) are left alone, and held particles never give way; a
    /// resting particle that is pushed is woken.
    ///
    /// One pass pushes older ends on and one pushes newer ends back, each in an order where
    /// every push is final, so together they resolve whole chains. A particle that
    /// [`Net::separate_nodes`] just pushed the other way is spared if possible, so that the
    /// other end gives way instead; where an overlap and the order can't both be cleared, the
    /// order wins. With `find_stuck`, [`Net::flow_stuck`] then records which particles are
    /// wedged against a held one.
    fn keep_flow_order(&mut self, segments: &FlowSegments, find_stuck: bool, target: &mut [Point]) {
        let mut deferred = false;
        for spare in [true, false] {
            if !spare && !deferred {
                break;
            }
            for &si in &segments.forward {
                let s = self.springs[si as usize];
                deferred |= self.push_segment(&s, s.b as usize, spare, target);
            }
            for &si in &segments.backward {
                let s = self.springs[si as usize];
                deferred |= self.push_segment(&s, s.a as usize, spare, target);
            }
        }
        for &i in &self.active {
            self.separated[i as usize] = 0;
        }
        if !find_stuck {
            return;
        }
        // A particle can't move on along the flow if it is held, or if a taut segment ties it
        // to an older particle that can't either; likewise backwards.
        let held = |net: &Net, i: usize| {
            if net.held[i] {
                STUCK_ON | STUCK_BACK
            } else {
                0
            }
        };
        for &i in &self.active {
            self.flow_stuck[i as usize] = held(self, i as usize);
        }
        for &si in &segments.forward {
            let s = self.springs[si as usize];
            for p in [s.a, s.b] {
                self.flow_stuck[p as usize] = held(self, p as usize);
            }
        }
        let taut = |net: &Net, s: &Spring| net.slack(s, target).is_some_and(|x| x < 0.5);
        for &si in &segments.backward {
            let s = self.springs[si as usize];
            if self.flow_stuck[s.b as usize] & STUCK_ON != 0 && taut(self, &s) {
                self.flow_stuck[s.a as usize] |= STUCK_ON;
            }
        }
        for &si in &segments.forward {
            let s = self.springs[si as usize];
            if self.flow_stuck[s.a as usize] & STUCK_BACK != 0 && taut(self, &s) {
                self.flow_stuck[s.b as usize] |= STUCK_BACK;
            }
        }
    }

    /// How much farther along the flow than needed the older end of an edge segment is, or
    /// `None` if the segment is not kept in order (see [`Net::keep_flow_order`]).
    fn slack(&self, s: &Spring, target: &[Point]) -> Option<f32> {
        let (a, b) = (s.a as usize, s.b as usize);
        let flow = self.flow;
        let rest = dot(add(s.offset, sub(self.home[b], self.home[a])), flow);
        if !s.along_edge || a == b || dot(s.offset, flow) <= 0.0 || rest <= 0.0 {
            return None;
        }
        let needed = self.flow_gap(a, b, rest);
        let have = dot(
            sub(
                add(self.origin[b], target[b]),
                add(self.origin[a], target[a]),
            ),
            flow,
        );
        Some(have - needed)
    }

    /// How far along the flow the older end `b` of an edge segment stays beyond its newer end
    /// `a` (see [`Net::keep_flow_order`]), where they are `rest` apart at rest.
    fn flow_gap(&self, a: usize, b: usize, rest: f32) -> f32 {
        (self.depth(a) + self.depth(b) + FLOW_GAP).min(rest)
    }

    /// Puts segment `s` back in order by moving its end `moved` (`s.b` on along the flow, or
    /// `s.a` back), unless that end is held or, with `spare_separated`, was just pushed the
    /// other way by [`Net::separate_nodes`]. Returns true if the latter left it out of order.
    fn push_segment(
        &mut self,
        s: &Spring,
        moved: usize,
        spare_separated: bool,
        target: &mut [Point],
    ) -> bool {
        if self.held[moved] {
            return false;
        }
        let Some(slack) = self.slack(s, target).filter(|&x| x < 0.0) else {
            return false;
        };
        let forward = moved == s.b as usize;
        let against = if forward {
            SEPARATED_BACK
        } else {
            SEPARATED_ON
        };
        if spare_separated && self.separated[moved] & against != 0 {
            return true;
        }
        let was = target[moved];
        let push = scale(self.flow, if forward { -slack } else { slack });
        target[moved] = add(was, push);
        self.activate_heading(moved, was);
        false
    }
}

/// Where a node rests, for saving and putting back (see [`Net::rest_places`] and
/// [`Net::restore`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RestPlace {
    pub node: usize,
    /// Rest position as an offset from the node's place in the layout.
    pub offset: Point,
    /// Moved by hand.
    pub moved: bool,
    /// The node's first parent, and how far from it the node rests. A layout of a changed
    /// graph may put the node somewhere else relative to its parent; this keeps it where it
    /// was.
    pub beside: Option<(usize, Point)>,
}

/// Counts of what makes a rearranged graph hard to read (see [`Net::tidiness`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tidiness {
    /// Pairs of node boxes that overlap.
    pub overlaps: usize,
    /// Edges that run through a node box other than their own two.
    pub edges_through_boxes: usize,
    /// Edges, not turned around, that run back against the direction of history on the way.
    pub edges_doubling_back: usize,
    /// Edges, not turned around, half as long again as the straight line between their nodes
    /// (and more than [`DETOUR_SLACK`] longer).
    pub edges_detouring: usize,
}

/// How much longer than half as long again an edge may be before it counts as a detour.
pub const DETOUR_SLACK: f32 = 60.0;

/// Edge segments (springs) for [`Net::keep_flow_order`]: sorted by where their newer end lies
/// along the flow, and by where their older end lies, against the flow.
#[derive(Clone, Debug, Default)]
struct FlowSegments {
    forward: Vec<u32>,
    backward: Vec<u32>,
    /// [`Net::active_changes`] when these were collected.
    built_at: Option<u64>,
}

/// Two nodes near each other during a frame.
#[derive(Clone, Copy, Debug)]
struct Near {
    a: u32,
    b: u32,
    /// ±1 if they sat side by side in a row at the start of the frame (`b` after `a` along the
    /// row, or before it), 0 otherwise.
    side: f32,
    /// Gap between their boxes at rest.
    rest_gap: f32,
}

impl Near {
    /// The gap that overlap avoidance keeps: the margin, or less if they rest closer.
    fn margin(&self) -> f32 {
        self.rest_gap.min(OVERLAP_MARGIN)
    }
}

/// Gap between two boxes whose centres are `d` apart and whose half extents add up to `ext`
/// (negative if they overlap), and the unit direction in which moving the second box away
/// from the first widens it fastest.
fn box_gap(d: Point, ext: Point) -> (f32, Point) {
    let (gx, gy) = (d.x.abs() - ext.x, d.y.abs() - ext.y);
    let sx = if d.x < 0.0 { -1.0 } else { 1.0 };
    let sy = if d.y < 0.0 { -1.0 } else { 1.0 };
    if gx > 0.0 && gy > 0.0 {
        let g = (gx * gx + gy * gy).sqrt();
        (g, Point::new(sx * gx / g, sy * gy / g))
    } else if gx > gy {
        (gx, Point::new(sx, 0.0))
    } else {
        (gy, Point::new(0.0, sy))
    }
}

/// Uniform grid for finding overlapping boxes.
#[derive(Clone, Debug, Default)]
struct Grid {
    cell: f32,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl Grid {
    /// Indexes boxes given as (id, centre, half extents).
    fn rebuild(&mut self, items: &[(u32, Point, Point)]) {
        self.cells.clear();
        let max_half = items
            .iter()
            .fold(0.0f32, |m, &(_, _, h)| m.max(h.x).max(h.y));
        self.cell = (2.0 * max_half).max(32.0);
        for &(i, p, h) in items {
            let (x0, y0) = self.key(p.x - h.x, p.y - h.y);
            let (x1, y1) = self.key(p.x + h.x, p.y + h.y);
            for x in x0..=x1 {
                for y in y0..=y1 {
                    self.cells.entry((x, y)).or_default().push(i);
                }
            }
        }
    }

    fn key(&self, x: f32, y: f32) -> (i32, i32) {
        (
            (x / self.cell).floor() as i32,
            (y / self.cell).floor() as i32,
        )
    }

    fn candidate_pairs(&self, out: &mut Vec<(u32, u32)>) {
        for items in self.cells.values() {
            for (i, &a) in items.iter().enumerate() {
                for &b in &items[i + 1..] {
                    out.push((a.min(b), a.max(b)));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
    }
}

fn add(a: Point, b: Point) -> Point {
    Point::new(a.x + b.x, a.y + b.y)
}

fn sub(a: Point, b: Point) -> Point {
    Point::new(a.x - b.x, a.y - b.y)
}

fn scale(a: Point, s: f32) -> Point {
    Point::new(a.x * s, a.y * s)
}

fn dot(a: Point, b: Point) -> f32 {
    a.x * b.x + a.y * b.y
}

fn len(a: Point) -> f32 {
    dot(a, a).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{self, LayoutEdge, LayoutInput, LayoutOptions};

    fn net_for(input: &LayoutInput) -> (Layout, Net) {
        let l = layout::layout(input, &LayoutOptions::default());
        let net = Net::new(&l, &input.sizes);
        (l, net)
    }

    fn edge(child: u32, parent: u32) -> LayoutEdge {
        LayoutEdge {
            child,
            parent,
            first_parent: true,
        }
    }

    /// A chain of five nodes: 0 -> 1 -> 2 -> 3 -> 4.
    fn chain_net() -> (Layout, Net) {
        net_for(&LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 5],
            times: vec![5, 4, 3, 2, 1],
            edges: (0..4).map(|i| edge(i, i + 1)).collect(),
            priority: Vec::new(),
        })
    }

    fn settle(net: &mut Net, params: &NetParams) {
        for _ in 0..3000 {
            if !net.step(1.0 / 60.0, params) {
                return;
            }
        }
        panic!("net did not settle");
    }

    /// Drags `nodes` by `by` (grabbing `nodes[0]`) over half a second, then drops them.
    fn drag(net: &mut Net, nodes: &[usize], by: Point, params: &NetParams) {
        let start = net.node_pos(nodes[0]);
        net.grab(nodes[0], nodes, &[], params.model.adapts());
        for f in 1..=30 {
            let t = f as f32 / 30.0;
            net.drag_to(add(start, scale(by, t)));
            net.step(1.0 / 60.0, params);
        }
        net.release(params);
    }

    fn free() -> NetParams {
        NetParams {
            model: DragModel::Free,
            ..NetParams::default()
        }
    }

    fn close(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 0.5 && (a.y - b.y).abs() < 0.5
    }

    #[test]
    fn neighbours_give_way_and_the_drop_stays() {
        let (l, mut net) = chain_net();
        let params = NetParams::default();
        let target = Point::new(l.nodes[2].x + 200.0, l.nodes[2].y);
        drag(&mut net, &[2], Point::new(200.0, 0.0), &params);
        settle(&mut net, &params);
        assert!(close(net.node_pos(2), target), "dropped node stays put");
        assert!(net.is_moved(2) && !net.is_moved(1));
        let moved = |net: &Net, i: usize| net.node_pos(i).x - l.nodes[i].x;
        assert!(
            moved(&net, 1) > 20.0 && moved(&net, 3) > 20.0,
            "neighbours follow"
        );
        assert!(
            moved(&net, 1) < 200.0 && moved(&net, 0) < moved(&net, 1),
            "pull decays: {:?}",
            (0..5).map(|i| moved(&net, i)).collect::<Vec<_>>()
        );
        // Nothing drifts afterwards: the new shape is the resting shape.
        let before: Vec<Point> = (0..5).map(|i| net.node_pos(i)).collect();
        for _ in 0..120 {
            net.step(1.0 / 60.0, &params);
        }
        assert!(!net.is_awake());
        for (i, &p) in before.iter().enumerate() {
            assert_eq!(net.node_pos(i), p);
        }
    }

    #[test]
    fn children_stay_above_their_parents() {
        let params = NetParams::default();
        // Drag the middle node far above the newest node, and far below the oldest.
        for dy in [-400.0, 400.0] {
            let (_, mut net) = chain_net();
            drag(&mut net, &[2], Point::new(30.0, dy), &params);
            settle(&mut net, &params);
            for child in 0..4 {
                // Boxes are 20 high, so the centres must be 20 + FLOW_GAP apart.
                let gap = net.node_pos(child + 1).y - net.node_pos(child).y - 20.0;
                assert!(
                    gap >= FLOW_GAP - 1.0,
                    "dy {dy}: node {child} ends {gap} above its parent"
                );
            }
        }
    }

    #[test]
    fn a_reversal_left_at_rest_is_kept() {
        let (l, mut net) = chain_net();
        let params = NetParams::default();
        // Put node 2 above its child 1 without the graph adapting, then adapt around it.
        let up = l.nodes[0].y - l.nodes[2].y - 40.0;
        drag(&mut net, &[2], Point::new(200.0, up), &free());
        settle(&mut net, &free());
        assert!(net.is_reversed(1));
        drag(&mut net, &[4], Point::new(30.0, 10.0), &params);
        settle(&mut net, &params);
        assert!(
            net.node_pos(2).y < net.node_pos(1).y,
            "the reversal was put back in order"
        );
    }

    #[test]
    fn reversed_edges_turn_round_their_nodes() {
        let (l, mut net) = chain_net();
        // Move node 2 above its child 1 (edge 1: 1 -> 2), well to the side.
        let up = l.nodes[0].y - l.nodes[2].y - 40.0;
        drag(&mut net, &[2], Point::new(200.0, up), &free());
        settle(&mut net, &free());
        assert!(net.is_reversed(1) && net.is_rerouted(1));
        let pts: Vec<Point> = net.edge_points(1).collect();
        let (child, parent) = (net.node_pos(1), net.node_pos(2));
        let n = pts.len();
        // Boxes are 20 high: it leaves below the child and arrives from above the parent.
        assert!(close(
            pts[1],
            Point::new(child.x, child.y + 10.0 + route::TURN)
        ));
        assert!(close(
            pts[n - 2],
            Point::new(parent.x, parent.y - 10.0 - route::TURN)
        ));
    }

    #[test]
    fn moved_nodes_keep_giving_way() {
        let (_, mut net) = chain_net();
        let params = NetParams::default();
        drag(&mut net, &[2], Point::new(200.0, 0.0), &params);
        settle(&mut net, &params);
        let dropped = net.node_pos(2);
        // Dragging a neighbour moves the node that was dropped before.
        drag(&mut net, &[3], Point::new(150.0, 0.0), &params);
        settle(&mut net, &params);
        assert!(
            net.node_pos(2).x > dropped.x + 20.0,
            "{:?} -> {:?}",
            dropped,
            net.node_pos(2)
        );
    }

    #[test]
    fn free_moves_only_the_dragged_nodes_and_stretches_their_edges() {
        // 0 -> 1 -> 2 -> 3, and a long edge 0 -> 3 with two bend points.
        let input = LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 4],
            times: vec![4, 3, 2, 1],
            edges: vec![edge(0, 1), edge(1, 2), edge(2, 3), edge(0, 3)],
            priority: Vec::new(),
        };
        let (l, mut net) = net_for(&input);
        let long = 3;
        assert_eq!(l.edges[long].len(), 4, "the long edge has two bend points");
        let by = Point::new(300.0, 0.0);
        drag(&mut net, &[0], by, &free());
        settle(&mut net, &free());
        assert!(close(net.node_pos(0), add(l.nodes[0], by)));
        for i in 1..4 {
            assert_eq!(net.node_pos(i), l.nodes[i], "node {i} stays");
        }
        // The bend points share the move, less the nearer they are to the parent. (The edge is
        // drawn along a route of its own now; its bend points still follow, for when its nodes
        // come back into line.)
        let pts: Vec<Point> = net.chains[long]
            .iter()
            .map(|&p| net.pos(p as usize))
            .collect();
        let shift = |k: usize| pts[k].x - l.edges[long][k].x;
        assert!(shift(1) > shift(2) && shift(2) > 0.0 && shift(1) < 300.0);
        assert!(net.is_moved(0) && net.is_rerouted(long));
    }

    #[test]
    fn free_moves_keep_their_offsets_when_adapting_again() {
        let (l, mut net) = chain_net();
        drag(&mut net, &[0], Point::new(300.0, 0.0), &free());
        settle(&mut net, &free());
        // Switching back to Adapt changes nothing by itself.
        let params = NetParams::default();
        for _ in 0..60 {
            net.step(1.0 / 60.0, &params);
        }
        assert!(close(
            net.node_pos(0),
            Point::new(l.nodes[0].x + 300.0, l.nodes[0].y)
        ));
        // Dragging its neighbour down pulls it along, but keeps it 300 to the right.
        drag(&mut net, &[1], Point::new(0.0, 60.0), &params);
        settle(&mut net, &params);
        let p = net.node_pos(0);
        assert!(p.y > l.nodes[0].y + 10.0, "follows: {p:?}");
        assert!(
            (p.x - l.nodes[0].x - 300.0).abs() < 5.0,
            "keeps its offset: {p:?}"
        );
    }

    /// True if no segment of edge `e` runs into the box of a node other than its own.
    fn edge_is_clear(net: &Net, e: usize) -> bool {
        let pts: Vec<Point> = net.edge_points(e).collect();
        let chain = &net.chains[e];
        let ends = [chain[0] as usize, chain[chain.len() - 1] as usize];
        (0..net.node_count())
            .filter(|i| !ends.contains(i))
            .all(|i| {
                let (c, h) = (net.node_pos(i), net.half[i]);
                pts.windows(2)
                    .all(|w| route::entry(w[0], w[1], sub(c, h), add(c, h)).is_none())
            })
    }

    #[test]
    fn squeezed_edges_lose_their_bends() {
        // 0 -> 1 -> 2 -> 3, and a long edge 0 -> 3 with two bend points.
        let input = LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 4],
            times: vec![4, 3, 2, 1],
            edges: vec![edge(0, 1), edge(1, 2), edge(2, 3), edge(0, 3)],
            priority: Vec::new(),
        };
        let (l, mut net) = net_for(&input);
        let long = 3;
        assert!(
            (0..4).all(|e| !net.is_rerouted(e)),
            "the layout's routes at first"
        );
        assert_eq!(net.edge_points(long).count(), 4);
        // Put 3 beside 0.
        let by = sub(Point::new(l.nodes[0].x + 150.0, l.nodes[0].y), l.nodes[3]);
        drag(&mut net, &[3], by, &free());
        settle(&mut net, &free());
        assert!(net.is_rerouted(long));
        assert_eq!(net.edge_points(long).count(), 2, "straight across");
        for e in 0..4 {
            assert!(edge_is_clear(&net, e), "edge {e} runs through a node");
        }
        // Back to the layout, back to its routes.
        net.reset();
        settle(&mut net, &free());
        assert!((0..4).all(|e| !net.is_rerouted(e)));
    }

    #[test]
    fn only_edges_at_moved_nodes_reroute() {
        let (_, mut net) = chain_net();
        let params = NetParams::default();
        drag(&mut net, &[2], Point::new(60.0, 0.0), &params);
        settle(&mut net, &params);
        // Edges 1 -> 2 and 2 -> 3 are at the dragged node; 0 -> 1 and 3 -> 4 only gave way.
        let rerouted: Vec<bool> = (0..4).map(|e| net.is_rerouted(e)).collect();
        assert_eq!(rerouted, [false, true, true, false]);
    }

    #[test]
    fn stretched_edges_go_around_nodes_in_between() {
        // Two chains side by side: 0 -> 1 and 2 -> 3.
        let input = LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 4],
            times: vec![4, 2, 3, 1],
            edges: vec![edge(0, 1), edge(2, 3)],
            priority: Vec::new(),
        };
        let (l, mut net) = net_for(&input);
        let (left, right) = if l.nodes[0].x < l.nodes[2].x {
            (0, 2)
        } else {
            (2, 0)
        };
        // Pull the right chain's tip far to the left, past the left chain: its edge would cut
        // through the left chain's nodes.
        let by = Point::new(l.nodes[left].x - l.nodes[right].x - 300.0, 60.0);
        drag(&mut net, &[right], by, &free());
        settle(&mut net, &free());
        let e = if right == 0 { 0 } else { 1 };
        assert!(net.is_rerouted(e));
        assert!(edge_is_clear(&net, e));
    }

    #[test]
    fn edges_make_way_for_a_node_dropped_on_them() {
        // 0 -> 1 in one column, and 2 -> 3 in another.
        let input = LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 4],
            times: vec![4, 2, 3, 1],
            edges: vec![edge(0, 1), edge(2, 3)],
            priority: Vec::new(),
        };
        let (l, mut net) = net_for(&input);
        assert!(!net.is_rerouted(0));
        // Drop 3 halfway along the edge from 0 to 1.
        let middle = Point::new(
            (l.nodes[0].x + l.nodes[1].x) / 2.0,
            (l.nodes[0].y + l.nodes[1].y) / 2.0,
        );
        drag(&mut net, &[3], sub(middle, l.nodes[3]), &free());
        settle(&mut net, &free());
        assert!(net.is_rerouted(0), "0 -> 1 is covered by 3");
        eprintln!(
            "nodes {:?}",
            (0..4)
                .map(|i| (net.node_pos(i), net.half[i]))
                .collect::<Vec<_>>()
        );
        eprintln!("edge {:?}", net.edge_points(0).collect::<Vec<_>>());
        assert!(edge_is_clear(&net, 0));
    }

    #[test]
    fn edges_left_behind_by_a_node_take_the_layout_route_again() {
        // 0 -> 1 in one column, and 2 far off to the side.
        let input = LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 3],
            times: vec![3, 2, 1],
            edges: vec![edge(0, 1)],
            priority: Vec::new(),
        };
        let opts = LayoutOptions {
            node_gap: 300.0,
            ..LayoutOptions::default()
        };
        let l = layout::layout(&input, &opts);
        let mut net = Net::new(&l, &input.sizes);
        assert!((l.nodes[2].x - l.nodes[1].x).abs() > 300.0);
        // Drop 2 on 1, the end of 0 -> 1, so that the edge re-routes.
        drag(&mut net, &[2], sub(l.nodes[1], l.nodes[2]), &free());
        settle(&mut net, &free());
        assert!(net.is_rerouted(0), "0 -> 1 is covered by 2");
        // On the way back 2 leaves the edge's surroundings between two frames. The edge must
        // still notice that it is clear.
        net.reset();
        settle(&mut net, &free());
        assert!(close(net.node_pos(2), l.nodes[2]));
        assert!(!net.is_rerouted(0), "0 -> 1 takes the layout's route again");
    }

    #[test]
    fn edges_rerouted_away_from_their_layout_route_take_it_again() {
        // 0 -> 1 -> 2 -> 3 in one column, 0 -> 3 alongside it, and 4 beside 3.
        let input = LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 5],
            times: vec![5, 4, 3, 2, 1],
            edges: vec![
                edge(0, 1),
                edge(1, 2),
                edge(2, 3),
                LayoutEdge {
                    first_parent: false,
                    ..edge(0, 3)
                },
            ],
            priority: Vec::new(),
        };
        let (l, mut net) = net_for(&input);
        // Drop 4 on the bend of 0 -> 3 beside 2.
        let bend = l.edges[3][2];
        assert!((bend.y - l.nodes[2].y).abs() < 1.0 && bend.x > l.nodes[2].x);
        drag(&mut net, &[4], sub(bend, l.nodes[4]), &free());
        settle(&mut net, &free());
        assert!(net.is_rerouted(3), "0 -> 3 is covered by 4");
        // On the way back 4 pushes the route of 0 -> 3 round the other side of the column,
        // away from its layout route, which 4 still covers.
        net.reset();
        settle(&mut net, &free());
        assert!(close(net.node_pos(4), l.nodes[4]));
        assert!(!net.is_rerouted(3), "0 -> 3 takes the layout's route again");
    }

    /// 0 -> 3 -> 4, and merges 1, 2 and 5 of 4 and 3 above 3: their edges into 4 are
    /// bundled into one trunk (edges 2, 3 and 4).
    fn bundle_net() -> (Layout, Net) {
        let merge = |child, parent| LayoutEdge {
            first_parent: false,
            ..edge(child, parent)
        };
        let input = LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 6],
            times: vec![10, 10, 10, 5, 1, 10],
            edges: vec![
                edge(0, 3),
                edge(3, 4),
                edge(1, 4),
                edge(2, 4),
                edge(5, 4),
                merge(1, 3),
                merge(2, 3),
                merge(5, 3),
            ],
            priority: Vec::new(),
        };
        let opts = LayoutOptions {
            concentrate_edges: true,
            ..LayoutOptions::default()
        };
        let l = layout::layout(&input, &opts);
        let net = Net::new(&l, &input.sizes);
        (l, net)
    }

    /// The last segment of each of `edges`: where they join into their parent.
    fn last_segments(net: &Net, edges: &[usize]) -> Vec<Vec<Point>> {
        edges
            .iter()
            .map(|&e| {
                let n = net.edge_points(e).len();
                net.edge_points(e).skip(n - 2).collect()
            })
            .collect()
    }

    /// True if `edges` end in one trunk: their last segments coincide, and start away from
    /// their children.
    fn bundled(net: &Net, edges: &[usize]) -> bool {
        let last = last_segments(net, edges);
        let children: Vec<Point> = edges
            .iter()
            .map(|&e| net.node_pos(net.chains[e][0] as usize))
            .collect();
        last.iter().all(|s| *s == last[0]) && !children.contains(&last[0][0])
    }

    const BUNDLED: [usize; 3] = [2, 3, 4];

    #[test]
    fn rerouted_edges_join_their_trunk() {
        let (_, mut net) = bundle_net();
        assert!(bundled(&net, &BUNDLED), "a trunk to begin with");
        for params in [free(), NetParams::default()] {
            // Nudge one child: its edge re-routes, and still joins the others.
            drag(&mut net, &[1], Point::new(6.0, 4.0), &params);
            settle(&mut net, &params);
            assert!(net.is_rerouted(2));
            assert!(bundled(&net, &BUNDLED), "{params:?}");
            // Move all the children: still one trunk.
            drag(&mut net, &[1, 2, 5], Point::new(-20.0, 10.0), &params);
            settle(&mut net, &params);
            assert!(BUNDLED.iter().all(|&e| net.is_rerouted(e)));
            assert!(bundled(&net, &BUNDLED), "{params:?}");
            net.reset();
            settle(&mut net, &params);
            assert!(BUNDLED.iter().all(|&e| !net.is_rerouted(e)));
        }
    }

    #[test]
    fn a_moved_parent_keeps_its_edges_bundled() {
        let (_, mut net) = bundle_net();
        for params in [free(), NetParams::default()] {
            drag(&mut net, &[4], Point::new(80.0, 30.0), &params);
            settle(&mut net, &params);
            assert!(
                bundled(&net, &BUNDLED),
                "{params:?}: {:?}",
                last_segments(&net, &BUNDLED)
            );
            for e in 0..8 {
                assert!(edge_is_clear(&net, e), "edge {e} runs through a node");
            }
            net.reset();
            settle(&mut net, &params);
        }
    }

    #[test]
    fn a_child_moved_past_the_trunk_takes_a_route_of_its_own() {
        let (l, mut net) = bundle_net();
        // Put 5 beside 3, level with the trunk's only bend point.
        let by = sub(Point::new(l.nodes[3].x + 150.0, l.nodes[3].y), l.nodes[5]);
        drag(&mut net, &[5], by, &free());
        settle(&mut net, &free());
        assert!(bundled(&net, &[2, 3]));
        let last = last_segments(&net, &[4, 2]);
        assert_eq!(last[0][0], net.node_pos(5), "straight from 5 to 4");
        assert_ne!(last[0], last[1]);
    }

    #[test]
    fn carried_nodes_move_along_unmarked() {
        let (l, mut net) = chain_net();
        let by = Point::new(120.0, 0.0);
        net.grab(1, &[1], &[0, 1], false);
        net.drag_to(add(l.nodes[1], by));
        net.step(1.0 / 60.0, &free());
        net.release(&free());
        settle(&mut net, &free());
        for i in [0, 1] {
            assert!(
                close(net.node_pos(i), add(l.nodes[i], by)),
                "node {i} moves"
            );
        }
        assert!(
            net.is_moved(1) && !net.is_moved(0),
            "only the grabbed node is marked"
        );
        assert!(net.is_displaced(0));
    }

    #[test]
    fn selections_move_together() {
        let (l, mut net) = chain_net();
        let by = Point::new(-150.0, 30.0);
        drag(&mut net, &[1, 3], by, &free());
        settle(&mut net, &free());
        for i in [1, 3] {
            assert!(close(net.node_pos(i), add(l.nodes[i], by)));
        }
        for i in [0, 2, 4] {
            assert_eq!(net.node_pos(i), l.nodes[i]);
        }
    }

    #[test]
    fn nodes_push_each_other_away() {
        // Two separate chains side by side: 0 -> 1 and 2 -> 3.
        let input = LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 4],
            times: vec![4, 2, 3, 1],
            edges: vec![edge(0, 1), edge(2, 3)],
            priority: Vec::new(),
        };
        let (l, _) = net_for(&input);
        assert_eq!(l.nodes[0].y, l.nodes[2].y, "tips share a row");
        let (left, right) = if l.nodes[0].x < l.nodes[2].x {
            (0, 2)
        } else {
            (2, 0)
        };
        // Bring the left tip within 10 of the right one.
        let gap = l.nodes[right].x - l.nodes[left].x - 60.0;
        let by = Point::new(gap - 10.0, 0.0);
        let pushed = |push: f32| {
            let (_, mut net) = net_for(&input);
            let params = NetParams {
                push,
                ..NetParams::default()
            };
            drag(&mut net, &[left], by, &params);
            settle(&mut net, &params);
            net.node_pos(right).x - l.nodes[right].x
        };
        let (with, without) = (pushed(0.5), pushed(0.0));
        assert!(
            with > 5.0 && with > without + 3.0,
            "pushed aside: {with} vs {without}"
        );
    }

    #[test]
    fn undo_and_redo() {
        let (l, mut net) = chain_net();
        let params = NetParams::default();
        assert!(!net.can_undo());
        drag(&mut net, &[2], Point::new(200.0, 0.0), &params);
        settle(&mut net, &params);
        let shape: Vec<Point> = (0..5).map(|i| net.node_pos(i)).collect();
        assert!(net.undo());
        settle(&mut net, &params);
        for i in 0..5 {
            assert!(close(net.node_pos(i), l.nodes[i]), "node {i} back");
        }
        assert!(!net.is_moved(2) && !net.any_displaced());
        assert!(net.redo());
        settle(&mut net, &params);
        for (i, &p) in shape.iter().enumerate() {
            assert!(close(net.node_pos(i), p), "node {i} redone");
        }
        assert!(net.is_moved(2));
        // A reset is undoable too.
        net.reset();
        settle(&mut net, &params);
        assert!(close(net.node_pos(2), l.nodes[2]));
        assert!(net.undo());
        settle(&mut net, &params);
        assert!(close(net.node_pos(2), shape[2]));
    }

    #[test]
    fn undo_with_nothing_to_undo_keeps_the_drag() {
        let (l, mut net) = chain_net();
        net.grab(2, &[2], &[], true);
        assert!(!net.undo() && !net.redo());
        assert_eq!(net.grabbed(), Some(2));
        net.drag_to(add(l.nodes[2], Point::new(50.0, 0.0)));
        net.step(1.0 / 60.0, &NetParams::default());
        assert!(net.node_pos(2).x > l.nodes[2].x + 49.0);
    }

    #[test]
    fn grabbing_without_moving_changes_nothing() {
        let input = LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 6],
            times: vec![1, 6, 5, 4, 3, 2],
            edges: (1..6).map(|t| edge(t, 0)).collect(),
            priority: Vec::new(),
        };
        // Nodes closer together than the overlap margin, as the spacing settings allow.
        let options = LayoutOptions {
            node_gap: 3.0,
            ..LayoutOptions::default()
        };
        let l = layout::layout(&input, &options);
        let mut net = Net::new(&l, &input.sizes);
        let params = NetParams::default();
        let hold = |net: &mut Net, node: usize| {
            net.grab(node, &[node], &[], true);
            for _ in 0..20 {
                net.step(1.0 / 60.0, &params);
            }
            net.release(&params);
        };
        // While everything rests.
        hold(&mut net, 3);
        settle(&mut net, &params);
        assert_eq!(net.rest_places().count(), 0);
        assert!(!net.can_undo());
        // While a drop is still settling.
        drag(&mut net, &[3], Point::new(0.0, 40.0), &params);
        net.step(1.0 / 60.0, &params);
        let rest: Vec<RestPlace> = net.rest_places().collect();
        for node in [3, 1, 4] {
            hold(&mut net, node);
        }
        settle(&mut net, &params);
        assert_eq!(net.rest_places().collect::<Vec<_>>(), rest);
        assert!(
            net.undo() && !net.can_undo(),
            "only the real drag was recorded"
        );
    }

    #[test]
    fn a_grab_wakes_only_what_it_presses_on() {
        // Many short chains side by side, rows closer than the magnet range.
        let chains = 3_000u32;
        let input = LayoutInput {
            sizes: vec![Point::new(40.0, 20.0); chains as usize * 3],
            times: (0..chains as i64 * 3).map(|i| 3 - i % 3).collect(),
            edges: (0..chains)
                .flat_map(|c| [edge(3 * c, 3 * c + 1), edge(3 * c + 1, 3 * c + 2)])
                .collect(),
            priority: Vec::new(),
        };
        let (_, mut net) = net_for(&input);
        net.grab(1, &[1], &[], true);
        for _ in 0..100 {
            net.step(1.0 / 60.0, &NetParams::default());
        }
        assert!(
            net.active_count() <= ACTIVE_BUDGET + 1,
            "{} particles awake",
            net.active_count()
        );
    }

    #[test]
    fn return_one_node_to_the_layout() {
        let (l, mut net) = chain_net();
        drag(&mut net, &[1, 2], Point::new(100.0, 0.0), &free());
        settle(&mut net, &free());
        net.return_to_layout(&[2]);
        settle(&mut net, &free());
        assert!(close(net.node_pos(2), l.nodes[2]) && !net.is_moved(2));
        assert!(close(
            net.node_pos(1),
            Point::new(l.nodes[1].x + 100.0, l.nodes[1].y)
        ));
        assert!(net.is_moved(1));
    }

    #[test]
    fn rest_places_round_trip() {
        let (l, mut net) = chain_net();
        drag(&mut net, &[3], Point::new(40.0, 0.0), &NetParams::default());
        settle(&mut net, &NetParams::default());
        let saved: Vec<RestPlace> = net.rest_places().collect();
        assert!(saved.iter().any(|s| s.node == 3 && s.moved));
        let (_, mut fresh) = chain_net();
        fresh.restore(saved, &NetParams::default());
        assert!(!fresh.is_awake(), "restores at once");
        for i in 0..5 {
            assert!(close(fresh.node_pos(i), net.node_pos(i)), "node {i}");
        }
        assert!(fresh.is_moved(3) && !fresh.is_moved(2));
        assert!(
            fresh.node_pos(2).x > l.nodes[2].x,
            "neighbours as they were"
        );
    }

    /// Saves `net`'s rest offsets and restores them into a net for `input`, which has the same
    /// nodes as `net` (at the same indices) and new ones after them.
    fn reload(net: &Net, input: &LayoutInput) -> (Layout, Net) {
        let (l, mut fresh) = net_for(input);
        fresh.restore(net.rest_places(), &NetParams::default());
        (l, fresh)
    }

    #[test]
    fn new_commits_on_a_moved_node_keep_their_place_beside_it() {
        // 0 -> 1 -> 2 -> 3; then new commits 5 -> 4 -> 1, a branch growing out of 1.
        let old = LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 4],
            times: vec![6, 5, 2, 1],
            edges: (0..3).map(|i| edge(i, i + 1)).collect(),
            priority: Vec::new(),
        };
        let mut new = old.clone();
        new.sizes.extend([Point::new(60.0, 20.0); 2]);
        new.times.extend([3, 4]);
        new.edges.extend([edge(4, 1), edge(5, 4)]);
        let (_, mut net) = net_for(&old);
        drag(&mut net, &[1], Point::new(300.0, 0.0), &free());
        settle(&mut net, &free());

        let (l, fresh) = reload(&net, &new);
        let from_1 = |i: usize| sub(fresh.node_pos(i), fresh.node_pos(1));
        for i in [4, 5] {
            assert!(
                close(from_1(i), sub(l.nodes[i], l.nodes[1])),
                "node {i} keeps its place beside 1: {:?}",
                from_1(i)
            );
            assert!(!fresh.is_moved(i));
        }
        assert!(
            close(fresh.node_pos(0), l.nodes[0]),
            "an old child that stayed put stays put"
        );
    }

    #[test]
    fn a_new_commit_is_pushed_clear_above_its_parents() {
        // 0 -> 1 -> 2 and 3 -> 2; then a new merge 4 of 0 and 3, after 3 was moved far up.
        let old = LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 4],
            times: vec![4, 3, 1, 2],
            edges: vec![edge(0, 1), edge(1, 2), edge(3, 2)],
            priority: Vec::new(),
        };
        let mut new = old.clone();
        new.sizes.push(Point::new(60.0, 20.0));
        new.times.push(5);
        new.edges.extend([
            edge(4, 0),
            LayoutEdge {
                first_parent: false,
                ..edge(4, 3)
            },
        ]);
        let (_, mut net) = net_for(&old);
        let up = scale(net.flow, -400.0);
        drag(&mut net, &[3], up, &free());
        settle(&mut net, &free());

        let (l, fresh) = reload(&net, &new);
        let flow = fresh.flow;
        let above = |child: usize, parent: usize| {
            dot(sub(fresh.node_pos(parent), fresh.node_pos(child)), flow)
        };
        assert!(above(4, 3) >= 20.0 + FLOW_GAP - 0.5, "{}", above(4, 3));
        assert!(above(4, 0) > 0.0);
        let across = sub(fresh.node_pos(4), l.nodes[4]);
        assert!(
            dot(across, Point::new(flow.y, flow.x)).abs() < 0.5,
            "only pushed up"
        );
        assert!(close(fresh.node_pos(0), l.nodes[0]), "old nodes stay");
    }

    /// Two chains side by side: 0 -> 1 and 2 -> 3.
    fn two_chains() -> LayoutInput {
        LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 4],
            times: vec![4, 2, 3, 1],
            edges: vec![edge(0, 1), edge(2, 3)],
            priority: Vec::new(),
        }
    }

    #[test]
    fn a_node_put_back_onto_another_pushes_it_aside() {
        let (l, mut net) = net_for(&two_chains());
        // Node 0 was moved by hand to where a changed layout now puts node 2.
        let onto = sub(l.nodes[2], l.nodes[0]);
        let place = RestPlace {
            node: 0,
            offset: onto,
            moved: true,
            beside: None,
        };
        net.restore([place], &NetParams::default());
        assert!(!net.is_awake(), "settles at once");
        assert!(close(net.node_pos(0), l.nodes[2]), "the moved node stays");
        assert!(!close(net.node_pos(2), l.nodes[2]), "the other gives way");
        assert_eq!(net.tidiness().overlaps, 0);
        let flow = net.flow;
        assert!(
            dot(sub(net.node_pos(3), net.node_pos(2)), flow) > 0.0,
            "and stays above its parent"
        );
        assert!(net.is_displaced(2) && !net.is_moved(2));
    }

    #[test]
    fn a_node_goes_back_beside_its_first_parent() {
        let apart = Point::new(0.0, -200.0);
        let offset = Point::new(5.0, 0.0);
        let place = |beside| RestPlace {
            node: 0,
            offset,
            moved: true,
            beside: Some(beside),
        };
        let (_, mut net) = net_for(&two_chains());
        net.restore([place((1, apart))], &NetParams::default());
        assert!(close(sub(net.node_pos(0), net.node_pos(1)), apart));
        // Beside a node that is no longer its first parent: at its offset instead.
        let (l, mut net) = net_for(&two_chains());
        net.restore([place((3, apart))], &NetParams::default());
        assert!(close(net.node_pos(0), add(l.nodes[0], offset)));
        // A node that rested in the layout stays in the layout.
        let (l, mut net) = net_for(&two_chains());
        let unplaced = RestPlace {
            offset: Point::default(),
            moved: false,
            ..place((1, apart))
        };
        net.restore([unplaced], &NetParams::default());
        assert!(close(net.node_pos(0), l.nodes[0]));
    }

    #[test]
    fn a_child_goes_back_beside_its_parent_after_the_parent() {
        // 0 -> 1 -> 2 -> 3 -> 4, with 1 and 0 both resting away; the child is listed first.
        let (_, mut net) = chain_net();
        let (to_1, to_0) = (Point::new(200.0, -30.0), Point::new(-120.0, -60.0));
        let place = |node: usize, beside: usize, apart: Point| RestPlace {
            node,
            offset: Point::new(1.0, 0.0),
            moved: true,
            beside: Some((beside, apart)),
        };
        net.restore(
            [place(0, 1, to_0), place(1, 2, to_1)],
            &NetParams::default(),
        );
        assert!(close(sub(net.node_pos(1), net.node_pos(2)), to_1));
        assert!(close(sub(net.node_pos(0), net.node_pos(1)), to_0));
    }

    #[test]
    fn a_moved_branch_tip_stays_put_when_a_merge_comes_in() {
        // #190: 0 (main) and 1 (a branch) grow out of 2; the branch tip is moved aside by
        // hand. Then main is merged into the branch: a new commit 3 with parents 1 and 0.
        let old = LayoutInput {
            sizes: vec![Point::new(120.0, 20.0); 3],
            times: vec![3, 2, 1],
            edges: vec![edge(0, 2), edge(1, 2)],
            priority: Vec::new(),
        };
        let mut new = old.clone();
        new.sizes.push(Point::new(120.0, 20.0));
        new.times.push(4);
        new.edges.extend([
            edge(3, 1),
            LayoutEdge {
                first_parent: false,
                ..edge(3, 0)
            },
        ]);
        let (_, mut net) = net_for(&old);
        drag(&mut net, &[1], Point::new(300.0, -40.0), &free());
        settle(&mut net, &free());
        let beside = sub(net.node_pos(1), net.node_pos(2));

        let (_, fresh) = reload(&net, &new);
        assert!(
            close(sub(fresh.node_pos(1), fresh.node_pos(2)), beside),
            "the tip keeps its place beside its parent"
        );
        assert!(fresh.is_moved(1) && !fresh.is_moved(3));
        let flow = fresh.flow;
        assert!(dot(sub(fresh.node_pos(1), fresh.node_pos(3)), flow) > 0.0);
        let tidiness = fresh.tidiness();
        assert_eq!(tidiness.overlaps, 0);
        assert_eq!(tidiness.edges_through_boxes, 0);
        assert_eq!(tidiness.edges_detouring, 0);
    }

    #[test]
    fn only_the_neighbourhood_of_a_grab_is_simulated() {
        // A chain far longer than the active budget.
        let n = ACTIVE_BUDGET as u32 * 2;
        let input = LayoutInput {
            sizes: vec![Point::new(20.0, 10.0); n as usize],
            times: (0..n as i64).rev().collect(),
            edges: (0..n - 1).map(|i| edge(i, i + 1)).collect(),
            priority: Vec::new(),
        };
        let (l, mut net) = net_for(&input);
        net.grab(0, &[0], &[], true);
        assert!(net.active_count() <= ACTIVE_BUDGET + 1);
        net.drag_to(Point::new(l.nodes[0].x + 100.0, l.nodes[0].y));
        net.step(1.0 / 60.0, &NetParams::default());
        assert_eq!(
            net.node_pos(n as usize - 1),
            l.nodes[n as usize - 1],
            "far end untouched"
        );
        net.release(&NetParams::default());
        settle(&mut net, &NetParams::default());
        assert_eq!(net.active_count(), 0, "everything goes back to sleep");
    }

    #[test]
    fn pushing_wakes_resting_nodes_in_big_graphs() {
        // A row of more tips than are simulated at once, under one root.
        let tips = ACTIVE_BUDGET as u32 + 2_000;
        let input = LayoutInput {
            sizes: vec![Point::new(40.0, 20.0); tips as usize + 1],
            times: (0..=tips as i64).rev().collect(),
            edges: (0..tips).map(|t| edge(t, tips)).collect(),
            priority: Vec::new(),
        };
        let (l, mut net) = net_for(&input);
        let row = |i: usize| l.nodes[i].x;
        let first = (0..tips as usize)
            .min_by(|&a, &b| row(a).total_cmp(&row(b)))
            .unwrap();
        let params = NetParams::default();
        net.grab(first, &[first], &[], true);
        let asleep = (0..tips as usize).filter(|&i| !net.is_active[i]).count();
        assert!(asleep > 0, "part of the row is not simulated");
        // Sweep through the whole row in big jumps.
        let span = (0..tips as usize).map(row).fold(0.0f32, f32::max) - row(first);
        for f in 1..=40 {
            let x = row(first) + span * f as f32 / 40.0;
            net.drag_to(Point::new(x, l.nodes[first].y + 3.0));
            net.step(1.0 / 60.0, &params);
        }
        net.release(&params);
        settle(&mut net, &params);
        for i in 0..=tips as usize {
            let p = net.node_pos(i);
            assert!(p.x.is_finite() && p.y.is_finite());
        }
    }

    #[test]
    fn neighbours_in_a_layer_keep_their_order() {
        // Root 0 with three tips side by side in one layer.
        let input = LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 4],
            times: vec![1, 4, 3, 2],
            edges: (1..4).map(|t| edge(t, 0)).collect(),
            priority: Vec::new(),
        };
        let (l, _) = net_for(&input);
        let order = |net: &Net| {
            let mut tips = vec![1, 2, 3];
            tips.sort_by(|&a, &b| net.node_pos(a).x.total_cmp(&net.node_pos(b).x));
            tips
        };
        for params in [NetParams::default(), free()] {
            let (_, mut net) = net_for(&input);
            let before = order(&net);
            // Drag the leftmost tip far past the others and drop it there, then reset.
            let left = before[0];
            let by = Point::new(l.nodes[before[2]].x - l.nodes[left].x + 200.0, 0.0);
            drag(&mut net, &[left], by, &params);
            settle(&mut net, &params);
            if params.model.adapts() {
                assert_eq!(order(&net), before, "pushed ahead like beads on a string");
            }
            net.reset();
            settle(&mut net, &params);
            assert_eq!(order(&net), before);
            for i in 0..4 {
                assert!(close(net.node_pos(i), l.nodes[i]), "node {i} back home");
            }
        }
    }

    #[test]
    fn old_settings_still_load() {
        assert_eq!(
            serde_json_like("Net"),
            DragModel::Adapt,
            "the spider web became Adapt"
        );
        assert_eq!(serde_json_like("Rigid"), DragModel::Free);
    }

    /// Deserialises a unit variant by name without pulling in a format crate.
    fn serde_json_like(name: &str) -> DragModel {
        use serde::de::IntoDeserializer;
        use serde::de::value::{Error, StrDeserializer};
        let d: StrDeserializer<'_, Error> = name.into_deserializer();
        DragModel::deserialize(d).expect("known variant")
    }
}
