//! Moving nodes by hand, with the graph as a net of weak springs.
//!
//! Every node and edge bend point is a particle with a rest position, at first its place in the
//! layout. Springs along the edges keep the offsets between rest positions, so a dragged node
//! pulls its neighbours along, and each particle is weakly anchored to its rest position, which
//! limits how far a pull spreads. Nodes closer than they rest push each other away, up to a
//! short range, like weak magnets; nodes side by side in a row keep their order, like beads on
//! a string, unless one is lifted out of the row; and the weft, springs between neighbours in a
//! layer, pushes edges aside the same way. While the graph adapts, every edge keeps running
//! along the flow, so children stay above their parents unless the user left them otherwise. A
//! drop makes the new shape the rest shape, so nothing drifts back.
//!
//! The state is kept as displacements from the layout, which stay small and therefore exact
//! where layout coordinates run into the millions. Each frame of an adaptive drag has two
//! parts:
//!
//! 1. **Shape.** The targets minimise `Σ k_s |d_b − d_a − (h_b − h_a)|² + Σ k_a |d_i − h_i|²`
//!    (plus the magnets) over displacements `d`, where `h` are the rest displacements and the
//!    dragged nodes are held fixed. They are found by Gauss-Seidel relaxation, warm-started from
//!    the previous frame. In a chain the displacement decays by a factor λ per hop when
//!    `k_a / k_s = (1 − λ)² / λ`, which is how [`NetParams::pull`] sets `k_a`.
//! 2. **Motion.** Every particle follows its target through a damped spring, overshooting as
//!    much as [`NetParams::wobble`] says.
//!
//! Only particles near the dragged nodes are simulated (breadth-first, up to a budget), so that
//! dragging stays smooth in graphs with a million bend points.
//!
//! Edges keep the layout's route until it stops fitting, and are then routed afresh around the
//! nodes ([`crate::route`]). A routed edge joins its bundle's trunk, and a trunk is routed
//! afresh as a whole.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::layout::{Layout, Point};
use crate::route::{self, Obstacles};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DragModel {
    #[default]
    #[serde(alias = "Net", alias = "Strings")]
    Adapt,
    #[serde(alias = "Rigid")]
    Free,
    /// The caller passes the nodes growing out of the dragged ones
    /// ([`crate::revgraph::RevGraph::subtree`]) to [`Net::grab`] as carried.
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

    pub fn adapts(self) -> bool {
        self == DragModel::Adapt
    }
}

/// `pull`, `push` and `wobble` run from 0 to 1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetParams {
    pub model: DragModel,
    pub pull: f32,
    pub push: f32,
    pub wobble: f32,
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

/// A pull has decayed to nothing well within this many particles.
const GRAB_WAKE_BUDGET: usize = 8_000;
const PUSH_WAKE_BUDGET: usize = 32;
const SETTLE_WAKE_BUDGET: usize = 1_000;
const SWEEPS_PER_FRAME: usize = 12;
const SWEEPS_ON_DROP: usize = 48;
const SETTLE_FRAMES: usize = 4;
const MAX_SEPARATE_PASSES: usize = 64;
const MAX_STALLED_SEPARATE_PASSES: usize = 4;
const FOLLOW_SUBSTEPS: usize = 4;
const FOLLOW_FREQUENCY_HZ: f32 = 3.0;
const OVERLAP_MARGIN: f32 = 6.0;
const NO_BUNDLE: u32 = u32::MAX;
const NO_NODE: u32 = u32::MAX;
/// Gap kept along the flow between the boxes at the ends of an edge segment while the graph
/// adapts, so that an edge visibly leaves its child towards its parent. More than twice
/// [`route::CLEARANCE`], so that routes still fit between rows.
pub const FLOW_GAP: f32 = 24.0;
const ALONG_FLOW: u8 = 1;
const AGAINST_FLOW: u8 = 2;
/// Edge springs have stiffness 1.
const MAX_MAGNET_STIFFNESS: f32 = 1.5;
const MAX_MAGNET_RANGE: f32 = 64.0;
const NEAR_GRID_CELL: f32 = 512.0;
/// Near pairs are collected once per frame, and come closer during its sweeps.
const NEAR_PAIRS_SLACK: f32 = 24.0;
const REST_EPSILON: f32 = 0.05;
const REARRANGED_STRETCH_LIMIT: f32 = 1.0;
const STRETCH_LIMIT: f32 = 40.0;
const ROUTE_UPDATE_STEP: f32 = 0.25;
const GRAZE_TOLERANCE: f32 = 2.0;
const MAX_UNDO_STEPS: usize = 200;
/// About 20 bytes each.
const MAX_UNDO_PARTICLE_CHANGES: usize = 1_000_000;
const MAX_SLEEP_SPEED: f32 = 1.0;
const MAX_SLEEP_DISTANCE: f32 = 0.1;
const DETOUR_FACTOR: f32 = 1.5;
const DETOUR_SLACK: f32 = 60.0;

#[derive(Clone, Copy, Debug)]
struct Spring {
    a: u32,
    b: u32,
    layout_offset: Point,
    stiffness: f32,
    along_edge: bool,
}

/// Spring ids per particle, flat: particle `i`'s are `ids[start[i]..start[i + 1]]`. A `Vec` per
/// particle would cost 24 bytes and an allocation each, for millions of particles.
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

#[derive(Clone, Debug)]
struct Route {
    bends: Vec<Point>,
    /// An index into the bundle's trunk, whose bend points the route runs through from there.
    joins_trunk_at: Option<u32>,
}

#[derive(Clone, Debug)]
enum Trunk {
    Layout,
    Routed(Vec<Point>),
    /// The parent has been moved before the trunk: every edge takes a route of its own.
    Broken,
}

#[derive(Clone, Debug)]
struct Grab {
    anchor: u32,
    anchor_start: Point,
    held_starts: Vec<(u32, Point)>,
    marked_on_drop: Vec<u32>,
    stretched_edges: Vec<u32>,
    stretched_bends: Vec<u32>,
    delta: Point,
    adapts: bool,
}

/// One undoable step: (particle, before, after) for rest positions and moved-by-hand marks.
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

#[derive(Clone, Copy)]
enum ChangeSide {
    Before,
    After,
}

impl ChangeSide {
    fn pick<T>(self, before: T, after: T) -> T {
        match self {
            ChangeSide::Before => before,
            ChangeSide::After => after,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Magnet {
    pair: u32,
    kept_gap: f32,
}

/// One frame's stiffness of the anchors and magnets, and how near the weft lets edges come.
struct Forces {
    k_anchor: f32,
    k_magnet: f32,
    weft_reach: f32,
}

#[derive(Clone, Debug)]
pub struct Net {
    node_count: usize,
    // Per particle. All but `layout_pos` and `half_size` are displacements from `layout_pos`.
    layout_pos: Vec<Point>,
    half_size: Vec<Point>,
    home: Vec<Point>,
    disp: Vec<Point>,
    velocity: Vec<Point>,
    target: Vec<Point>,
    frame_start_target: Vec<Point>,

    displaced_count: usize,
    flows_vertically: bool,
    /// The unit vector from newer to older commits.
    flow: Point,
    moved_by_hand: Vec<bool>,
    springs: Vec<Spring>,
    springs_at: SpringsAt,
    /// Per edge: its particles, child first, parent last.
    chains: Vec<Vec<u32>>,
    node_edges: Vec<Vec<u32>>,
    first_parent: Vec<u32>,
    /// Indexed by bend particle minus `node_count`.
    edges_through_bend: Vec<u32>,
    grab: Option<Grab>,
    is_held: Vec<bool>,
    awake: bool,
    wobbly: bool,
    active: Vec<u32>,
    is_active: Vec<bool>,
    active_generation: u64,
    segments: FlowSegments,
    /// [`ALONG_FLOW`] and [`AGAINST_FLOW`]: the ways each particle can't move without moving a
    /// held one, as of the last flow-order pass that looked.
    wedged: Vec<u8>,
    /// [`ALONG_FLOW`] and [`AGAINST_FLOW`]: the ways [`Net::separate_nodes`] pushed each
    /// particle since the last flow-order pass.
    separated: Vec<u8>,
    grid: Grid,
    magnets: Vec<Vec<Magnet>>,
    nodes_with_magnets: Vec<u32>,
    undo: Vec<Change>,
    redo: Vec<Change>,
    /// Per bundle, the bend points its edges share on their way into their parent, in order.
    /// Each edge's layout route ends with part of its trunk.
    trunks: Vec<Vec<u32>>,
    bundle_edges: Vec<Vec<u32>>,
    bundle_of: Vec<u32>,
    trunk_cache: HashMap<u32, Trunk>,
    own_routes: Vec<Option<Route>>,
    turned: Vec<bool>,
    obstacles: Option<Obstacles>,
    disp_in_obstacles: Vec<Point>,
    is_settling: bool,
}

impl Net {
    pub fn new(layout: &Layout, node_sizes: &[Point]) -> Net {
        let n = layout.nodes.len();
        let mut layout_pos = layout.nodes.clone();
        let mut half_size: Vec<Point> = node_sizes
            .iter()
            .map(|s| Point::new(s.x / 2.0, s.y / 2.0))
            .collect();
        let mut layer_of: Vec<u32> = layout.layers.clone();
        let mut chains = Vec::with_capacity(layout.edges.len());
        let mut node_edges = vec![Vec::new(); n];
        let mut first_parent = vec![NO_NODE; n];
        let mut edges_through_bend: Vec<u32> = Vec::new();
        let mut springs = Vec::new();
        let mut seen_segments = HashSet::new();
        // Bundled edges share bend points, and so particles.
        let mut particle_of_bend: HashMap<u32, u32> = HashMap::new();

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
                let particle = match id.and_then(|id| particle_of_bend.get(&id)) {
                    Some(&particle) => particle,
                    None => {
                        let particle = layout_pos.len() as u32;
                        layout_pos.push(*p);
                        half_size.push(Point::default());
                        edges_through_bend.push(0);
                        layer_of.push(layout.layers[child as usize] + 1 + k as u32);
                        if let Some(id) = id {
                            particle_of_bend.insert(id, particle);
                        }
                        particle
                    }
                };
                chain.push(particle);
            }
            chain.push(parent);
            // Count each edge once per bend point, even if it passes it twice.
            for p in sorted_unique(chain[1..chain.len() - 1].iter().copied()) {
                edges_through_bend[p as usize - n] += 1;
            }
            for w in chain.windows(2) {
                if seen_segments.insert((w[0], w[1])) {
                    springs.push(Spring {
                        a: w[0],
                        b: w[1],
                        layout_offset: layout_pos[w[1] as usize] - layout_pos[w[0] as usize],
                        stiffness: 1.0,
                        along_edge: true,
                    });
                }
            }
            chains.push(chain);
        }

        // The weft: springs between neighbours within a layer.
        let layer_count = layer_of.iter().map(|&l| l as usize + 1).max().unwrap_or(0);
        let mut by_layer: Vec<Vec<u32>> = vec![Vec::new(); layer_count];
        for (i, &l) in layer_of.iter().enumerate() {
            by_layer[l as usize].push(i as u32);
        }
        let flows_vertically = layout.direction.is_vertical();
        let along = |p: Point| if flows_vertically { p.x } else { p.y };
        for layer in &mut by_layer {
            layer.sort_by(|&a, &b| {
                along(layout_pos[a as usize]).total_cmp(&along(layout_pos[b as usize]))
            });
            for w in layer.windows(2) {
                springs.push(Spring {
                    a: w[0],
                    b: w[1],
                    layout_offset: layout_pos[w[1] as usize] - layout_pos[w[0] as usize],
                    stiffness: 0.5,
                    along_edge: false,
                });
            }
        }

        // Bundles: edges whose last bend points are shared (all edges through a bend point run
        // into the same parent). The longest shared run is the trunk.
        let mut trunks: Vec<Vec<u32>> = Vec::new();
        let mut bundle_of = vec![NO_BUNDLE; chains.len()];
        let mut bundle_at: HashMap<u32, u32> = HashMap::new();
        fn shared_run<'a>(chain: &'a [u32], edges_through_bend: &[u32], n: usize) -> &'a [u32] {
            let bends = &chain[1..chain.len() - 1];
            let k = bends
                .iter()
                .rev()
                .take_while(|&&p| edges_through_bend[p as usize - n] > 1)
                .count();
            &bends[bends.len() - k..]
        }
        for (e, chain) in chains.iter().enumerate() {
            let run = shared_run(chain, &edges_through_bend, n);
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
            bundle_of[e] = b;
        }
        let mut bundle_edges = vec![Vec::new(); trunks.len()];
        for (e, chain) in chains.iter().enumerate() {
            let b = bundle_of[e];
            if b != NO_BUNDLE {
                if trunks[b as usize].ends_with(shared_run(chain, &edges_through_bend, n)) {
                    bundle_edges[b as usize].push(e as u32);
                } else {
                    bundle_of[e] = NO_BUNDLE;
                }
            }
        }

        let count = layout_pos.len();
        let springs_at = SpringsAt::new(count, &springs);
        Net {
            node_count: n,
            layout_pos,
            half_size,
            home: vec![Point::default(); count],
            disp: vec![Point::default(); count],
            velocity: vec![Point::default(); count],
            target: vec![Point::default(); count],
            frame_start_target: vec![Point::default(); count],
            displaced_count: 0,
            flows_vertically,
            flow: layout.direction.flow(),
            moved_by_hand: vec![false; n],
            springs,
            springs_at,
            chains,
            node_edges,
            first_parent,
            edges_through_bend,
            grab: None,
            is_held: vec![false; count],
            awake: false,
            wobbly: false,
            active: Vec::new(),
            is_active: vec![false; count],
            active_generation: 0,
            segments: FlowSegments::default(),
            wedged: vec![0; count],
            separated: vec![0; count],
            grid: Grid::default(),
            magnets: vec![Vec::new(); n],
            nodes_with_magnets: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            trunks,
            bundle_edges,
            bundle_of,
            trunk_cache: HashMap::new(),
            own_routes: vec![None; layout.edges.len()],
            turned: vec![false; layout.edges.len()],
            obstacles: None,
            disp_in_obstacles: vec![Point::default(); n],
            is_settling: false,
        }
    }

    fn pos(&self, i: usize) -> Point {
        self.layout_pos[i] + self.disp[i]
    }

    pub fn node_pos(&self, node: usize) -> Point {
        self.pos(node)
    }

    /// (child, parent) of edge `e`.
    fn ends(&self, e: usize) -> (usize, usize) {
        let chain = &self.chains[e];
        (chain[0] as usize, chain[chain.len() - 1] as usize)
    }

    fn bends(&self, e: usize) -> &[u32] {
        let chain = &self.chains[e];
        &chain[1..chain.len() - 1]
    }

    /// Child centre, bend points, parent centre.
    pub fn edge_points(&self, e: usize) -> impl ExactSizeIterator<Item = Point> + '_ {
        let pts: Vec<Point> = match &self.own_routes[e] {
            Some(route) => {
                let (c, p) = self.ends(e);
                let trunk = match route.joins_trunk_at {
                    Some(j) => &self.trunks[self.bundle_of[e] as usize][j as usize..],
                    None => &[],
                };
                std::iter::once(self.pos(c))
                    .chain(route.bends.iter().copied())
                    .chain(trunk.iter().map(|&t| self.pos(t as usize)))
                    .chain(std::iter::once(self.pos(p)))
                    .collect()
            }
            None => self.chains[e]
                .iter()
                .map(|&p| self.pos(p as usize))
                .collect(),
        };
        pts.into_iter()
    }

    pub fn tidiness(&self) -> Tidiness {
        let n = self.node_count;
        let mut obstacles = Obstacles::new(n);
        for i in 0..n {
            obstacles.place(i, self.pos(i), self.half_size[i]);
        }
        let mut out = Tidiness {
            overlaps: self.overlapping_pairs(|i| self.pos(i)).len(),
            ..Tidiness::default()
        };
        for e in 0..self.chains.len() {
            let (child, parent) = self.ends(e);
            if child == parent {
                continue;
            }
            // As drawn: from the child's side facing its parents to the parent's other side.
            let mut pts: Vec<Point> = self.edge_points(e).collect();
            let last = pts.len() - 1;
            pts[0] = self.exit(child);
            pts[last] = self.entry(parent);
            let mut through = false;
            for w in pts.windows(2) {
                obstacles.near_segment(w[0], w[1], |i| {
                    let i = i as usize;
                    if through || i == child || i == parent {
                        return;
                    }
                    // Cutting a corner, as layout routes may, doesn't count.
                    through = self.segment_enters(w[0], w[1], i, OVERLAP_MARGIN);
                });
            }
            out.edges_through_boxes += usize::from(through);
            // Reversed edges run round their nodes by design.
            if !self.is_reversed(e) {
                let back: f32 = pts
                    .windows(2)
                    .map(|w| (-(w[1] - w[0]).dot(self.flow)).max(0.0))
                    .sum();
                out.edges_doubling_back += usize::from(back > route::TURN_LENGTH);
                let length: f32 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
                let direct = (pts[last] - pts[0]).length();
                out.edges_detouring += usize::from(length > DETOUR_FACTOR * direct + DETOUR_SLACK);
            }
        }
        out
    }

    /// True if the segment from `p` to `q` enters node `i`'s box shrunk by `inset`.
    fn segment_enters(&self, p: Point, q: Point, i: usize, inset: f32) -> bool {
        let (c, half) = (self.pos(i), self.half_size[i]);
        let h = half - Point::new(inset, inset);
        route::box_entry(p, q, c - h, c + h).is_some()
    }

    pub fn is_rerouted(&self, e: usize) -> bool {
        self.own_routes[e].is_some()
    }

    /// True if edge `e` has a route made for a reversed edge: its first bend point lies
    /// straight on from the child along the flow, its last straight before the parent, and it
    /// runs round both in between.
    pub fn has_turned_route(&self, e: usize) -> bool {
        self.turned[e]
    }

    /// True if edge `e`'s parent has been moved before its child, against the flow.
    fn is_reversed(&self, e: usize) -> bool {
        let (c, p) = self.ends(e);
        (self.pos(p) - self.pos(c)).dot(self.flow) < 0.0
    }

    pub fn is_displaced(&self, node: usize) -> bool {
        self.home[node] != Point::default()
    }

    pub fn any_displaced(&self) -> bool {
        self.displaced_count > 0
    }

    fn set_home(&mut self, p: usize, h: Point) {
        let zero = Point::default();
        match (self.home[p] == zero, h == zero) {
            (true, false) => self.displaced_count += 1,
            (false, true) => self.displaced_count -= 1,
            _ => {}
        }
        self.home[p] = h;
    }

    pub fn is_awake(&self) -> bool {
        self.awake || self.grab.is_some()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Starts dragging `anchor`, the node under the pointer, with `nodes` and `carried`, all
    /// rigidly together. Once dropped, `anchor` and `nodes` count as moved by hand.
    pub fn grab(&mut self, anchor: usize, nodes: &[usize], carried: &[usize], model: DragModel) {
        self.cancel_grab();
        let n = self.node_count;
        let marked_on_drop = sorted_unique(
            nodes
                .iter()
                .chain([&anchor])
                .filter(|&&node| node < n)
                .map(|&node| node as u32),
        );
        let mut held: Vec<u32> = Vec::new();
        let carried = carried.iter().filter(|&&c| c < n).map(|&c| c as u32);
        for node in marked_on_drop.iter().copied().chain(carried) {
            if !self.is_held[node as usize] {
                self.is_held[node as usize] = true;
                held.push(node);
            }
        }
        let edges = self.edges_at(held.iter().copied());
        // Bend points whose edges all run between held nodes are held too.
        let mut internal_bends: Vec<u32> = edges
            .iter()
            .filter(|&&e| {
                let (a, b) = self.ends(e as usize);
                self.is_held[a] && self.is_held[b]
            })
            .flat_map(|&e| sorted_unique(self.bends(e as usize).iter().copied()))
            .collect();
        internal_bends.sort_unstable();
        for same_bend in internal_bends.chunk_by(|a, b| a == b) {
            let p = same_bend[0];
            if same_bend.len() == self.edges_through_bend[p as usize - n] as usize {
                self.is_held[p as usize] = true;
                held.push(p);
            }
        }
        let stretched_edges: Vec<u32> = edges
            .into_iter()
            .filter(|&e| {
                self.bends(e as usize)
                    .iter()
                    .any(|&p| !self.is_held[p as usize])
            })
            .collect();
        let stretched_bends = sorted_unique(
            stretched_edges
                .iter()
                .flat_map(|&e| self.bends(e as usize).iter().copied())
                .filter(|&p| !self.is_held[p as usize]),
        );

        for &p in &held {
            self.activate(p as usize);
        }
        let adapts = model.adapts();
        if adapts {
            self.wake_around(&held, GRAB_WAKE_BUDGET);
        } else {
            for &p in &stretched_bends {
                self.activate(p as usize);
            }
        }
        self.wobbly = adapts;
        self.awake = true;
        self.grab = Some(Grab {
            anchor: anchor as u32,
            anchor_start: self.disp[anchor],
            held_starts: held.iter().map(|&p| (p, self.disp[p as usize])).collect(),
            marked_on_drop,
            stretched_edges,
            stretched_bends,
            delta: Point::default(),
            adapts,
        });
    }

    /// Moves the anchor's centre to `target`, and the other dragged nodes with it.
    pub fn drag_to(&mut self, target: Point) {
        if let Some(g) = &mut self.grab {
            let at = target - self.layout_pos[g.anchor as usize];
            g.delta = at - g.anchor_start;
            self.awake = true;
        }
    }

    /// Drops the dragged nodes, and the shape the net has taken becomes its rest shape.
    /// Undoable.
    pub fn release(&mut self, params: &NetParams) {
        let Some(grab) = &self.grab else {
            return;
        };
        if grab.delta.length() <= REST_EPSILON {
            self.cancel_grab();
            return;
        }
        self.shape(params, SWEEPS_ON_DROP);
        let grab = self.grab.take().expect("grab exists");
        let mut change = Change {
            homes: self.rest_at_targets(),
            marks: Vec::new(),
        };
        self.let_go(&grab);
        for &node in &grab.marked_on_drop {
            if !self.moved_by_hand[node as usize] {
                self.moved_by_hand[node as usize] = true;
                change.marks.push((node, false, true));
            }
        }
        self.record(change);
        self.awake = true;
    }

    /// Makes every woken particle rest where it is heading, and returns the rest positions that
    /// changed: (particle, before, after).
    fn rest_at_targets(&mut self) -> Vec<(u32, Point, Point)> {
        let mut changed = Vec::new();
        for k in 0..self.active.len() {
            let p = self.active[k] as usize;
            let t = self.target[p];
            if (t - self.home[p]).length() > REST_EPSILON {
                changed.push((p as u32, self.home[p], t));
                self.set_home(p, t);
            }
            self.target[p] = self.home[p];
        }
        changed
    }

    fn let_go(&mut self, grab: &Grab) {
        for &(p, _) in &grab.held_starts {
            self.is_held[p as usize] = false;
        }
    }

    fn cancel_grab(&mut self) {
        if let Some(grab) = self.grab.take() {
            self.let_go(&grab);
            self.awake = true;
        }
    }

    fn record(&mut self, change: Change) {
        if change.is_empty() {
            return;
        }
        self.undo.push(change);
        self.redo.clear();
        // Forget the oldest steps beyond the limits, but always keep the latest.
        let size = |c: &Change| c.homes.len() + c.marks.len();
        let mut total: usize = self.undo.iter().map(size).sum();
        let mut drop = 0;
        while self.undo.len() - drop > 1
            && (self.undo.len() - drop > MAX_UNDO_STEPS || total > MAX_UNDO_PARTICLE_CHANGES)
        {
            total -= size(&self.undo[drop]);
            drop += 1;
        }
        self.undo.drain(..drop);
    }

    /// Puts the state before or after `change` in place; the particles concerned move there
    /// smoothly.
    fn apply(&mut self, change: &Change, side: ChangeSide) {
        for &(p, before, after) in &change.homes {
            self.set_home(p as usize, side.pick(before, after));
            self.activate(p as usize);
        }
        for &(node, before, after) in &change.marks {
            self.moved_by_hand[node as usize] = side.pick(before, after);
        }
        self.wobbly = false;
        self.awake = true;
    }

    /// Reverts the last drop, reset or return to the layout, ending any drag; false if there
    /// is none.
    pub fn undo(&mut self) -> bool {
        let Some(change) = self.undo.pop() else {
            return false;
        };
        self.cancel_grab();
        self.apply(&change, ChangeSide::Before);
        self.redo.push(change);
        true
    }

    /// Repeats the last change undone, ending any drag; false if there is none.
    pub fn redo(&mut self) -> bool {
        let Some(change) = self.redo.pop() else {
            return false;
        };
        self.cancel_grab();
        self.apply(&change, ChangeSide::After);
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
        for (node, &moved) in self.moved_by_hand.iter().enumerate() {
            if moved {
                change.marks.push((node as u32, true, false));
            }
        }
        self.apply(&change, ChangeSide::After);
        self.record(change);
    }

    /// Sends `nodes` back to their layout positions, stretching the edges at them; nothing
    /// else moves. Undoable.
    pub fn return_to_layout(&mut self, nodes: &[usize]) {
        self.cancel_grab();
        let shifts: BTreeMap<u32, Point> = nodes
            .iter()
            .filter(|&&node| node < self.node_count)
            .map(|&node| (node as u32, -self.home[node]))
            .collect();
        let mut change = Change {
            homes: self.shifted_homes(&shifts),
            marks: Vec::new(),
        };
        for &node in shifts.keys() {
            if self.moved_by_hand[node as usize] {
                change.marks.push((node, true, false));
            }
        }
        self.apply(&change, ChangeSide::After);
        self.record(change);
    }

    /// Where the nodes rest that rest away from the layout or were moved by hand, for saving;
    /// and the children of those resting away, so that [`Net::restore`] can tell them from new
    /// commits.
    pub fn rest_places(&self) -> impl Iterator<Item = RestPlace> + '_ {
        let rest = |i: usize| self.layout_pos[i] + self.home[i];
        (0..self.node_count)
            .filter(|&i| {
                self.moved_by_hand[i]
                    || self.is_displaced(i)
                    || self.parents(i).any(|p| self.is_displaced(p as usize))
            })
            .map(move |i| {
                let first = self.first_parent[i];
                RestPlace {
                    node: i,
                    offset: self.home[i],
                    by_hand: self.moved_by_hand[i],
                    beside: (first != NO_NODE)
                        .then(|| (first as usize, rest(i) - rest(first as usize))),
                }
            })
    }

    fn parents(&self, node: usize) -> impl Iterator<Item = u32> + '_ {
        let node = node as u32;
        self.edge_ends(node)
            .filter_map(move |(child, parent)| (child == node).then_some(parent))
    }

    fn children(&self, node: usize) -> impl Iterator<Item = u32> + '_ {
        let node = node as u32;
        self.edge_ends(node)
            .filter_map(move |(child, parent)| (parent == node).then_some(child))
    }

    /// (child, parent) of every edge at `node`, leaving out loops.
    fn edge_ends(&self, node: u32) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.node_edges[node as usize].iter().filter_map(|&e| {
            let (child, parent) = self.ends(e as usize);
            (parent != child).then_some((child as u32, parent as u32))
        })
    }

    /// The edges at `nodes`, sorted, each once.
    fn edges_at(&self, nodes: impl IntoIterator<Item = u32>) -> Vec<u32> {
        sorted_unique(
            nodes
                .into_iter()
                .flat_map(|node| self.node_edges[node as usize].iter().copied()),
        )
    }

    /// Puts saved nodes back at once: beside their first parent as they were, if it still is
    /// their first parent, else at their offset from the layout. Unsaved children of nodes
    /// resting away are new commits: they keep their place beside their first parent, moved on
    /// against the flow if that leaves them too close to a parent. The layout may have changed
    /// since the places were saved, so the graph then settles where nodes overlap. For a fresh
    /// net; not undoable.
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
        let along_flow = |i: usize| self.layout_pos[i].dot(self.flow);
        saved.sort_by(|a, b| {
            along_flow(b.node)
                .total_cmp(&along_flow(a.node))
                .then(a.node.cmp(&b.node))
        });
        let mut shifts: BTreeMap<u32, Point> = BTreeMap::new();
        for s in &saved {
            let i = s.node;
            self.moved_by_hand[i] = s.by_hand;
            let rested_away = s.offset != Point::default();
            let rest = match s.beside {
                Some((parent, apart))
                    if rested_away
                        && parent < n
                        && self.first_parent[i] == parent as u32
                        && finite(apart) =>
                {
                    let shift = shifts.get(&(parent as u32)).copied().unwrap_or_default();
                    self.layout_pos[parent] + self.home[parent] + shift + apart
                }
                _ => self.layout_pos[i] + s.offset,
            };
            shifts.insert(i as u32, rest - self.layout_pos[i] - self.home[i]);
        }
        self.place_new_nodes(&mut shifts);
        if shifts.values().all(|&d| d == Point::default()) {
            return;
        }
        for (p, _, after) in self.shifted_homes(&shifts) {
            let p = p as usize;
            self.set_home(p, after);
            self.disp[p] = after;
            self.target[p] = after;
            self.velocity[p] = Point::default();
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
        let held_starts: Vec<(u32, Point)> = (0..self.node_count as u32)
            .filter(|&i| self.moved_by_hand[i as usize] && self.is_displaced(i as usize))
            .map(|i| (i, self.disp[i as usize]))
            .collect();
        for &(p, _) in &held_starts {
            self.is_held[p as usize] = true;
        }
        self.wake_around(&overlapping, SETTLE_WAKE_BUDGET);
        self.is_settling = true;
        self.grab = Some(Grab {
            anchor,
            anchor_start: self.disp[anchor as usize],
            held_starts,
            marked_on_drop: Vec::new(),
            stretched_edges: Vec::new(),
            stretched_bends: Vec::new(),
            delta: Point::default(),
            adapts: true,
        });
        let params = NetParams {
            model: DragModel::Adapt,
            avoid_overlap: true,
            ..params.clone()
        };
        // Nodes pushed aside may come up against others: look again each frame, as a drag does.
        for _ in 0..SETTLE_FRAMES {
            self.shape(&params, SWEEPS_PER_FRAME);
        }
        let n = self.node_count;
        let start: Vec<(u32, Point)> = self
            .active
            .iter()
            .filter(|&&i| (i as usize) < n)
            .map(|&i| (i, self.target[i as usize]))
            .collect();
        self.separate_alone();
        self.clear_along_layers();
        self.bend_edges_along(start);
        self.is_settling = false;
        let grab = self.grab.take().expect("settling");
        self.let_go(&grab);
        self.rest_at_targets();
        self.deactivate_all();
        self.awake = false;
    }

    /// Separates overlapping nodes, keeping children above their parents, for as long as that
    /// clears overlaps: a row of nodes pushed along it clears one pair per sweep while the
    /// springs pull it back between sweeps. If some are left, as where a node is wedged between
    /// nodes moved by hand, those give way too.
    fn separate_alone(&mut self) {
        let (mut least, mut stalled) = (f32::INFINITY, 0);
        for _ in 0..MAX_SEPARATE_PASSES {
            let near = self.near_pairs(OVERLAP_MARGIN);
            let overlap: f32 = near
                .iter()
                .map(|pair| (-self.gap(pair, &self.target, pair.margin()).0).max(0.0))
                .sum();
            if overlap < 0.99 * least {
                (least, stalled) = (overlap, 0);
            } else {
                stalled += 1;
            }
            if overlap <= REST_EPSILON {
                break;
            }
            if stalled >= MAX_STALLED_SEPARATE_PASSES {
                let grab = self.grab.as_ref().expect("settling");
                if !grab
                    .held_starts
                    .iter()
                    .any(|&(p, _)| self.is_held[p as usize])
                {
                    break;
                }
                for &(p, _) in &grab.held_starts {
                    self.is_held[p as usize] = false;
                }
                (least, stalled) = (overlap, 0);
            }
            let mut target = std::mem::take(&mut self.target);
            self.separate_nodes(&mut target, &near);
            let segments = self.flow_segments();
            self.keep_flow_order(&segments, true, &mut target);
            self.target = target;
        }
    }

    /// Bends the edges at the nodes that moved from where they were heading at `start` along
    /// with them.
    fn bend_edges_along(&mut self, start: Vec<(u32, Point)>) {
        let shifts: BTreeMap<u32, Point> = start
            .into_iter()
            .map(|(i, was)| (i, self.target[i as usize] - was))
            .filter(|&(_, d)| d.length() > REST_EPSILON)
            .collect();
        let edges = self.edges_at(shifts.keys().copied());
        let bend_shifts = self.bend_shifts(&edges, |i| shifts.get(&i).copied().unwrap_or_default());
        for (p, shift) in bend_shifts {
            let p = p as usize;
            self.activate(p);
            self.target[p] += shift;
        }
    }

    /// Clears the overlaps settling leaves, where they and the order along the flow can't both
    /// be cleared, by pushing nodes along their layer, which never turns an edge around: of
    /// each overlapping pair, the one farther along moves on.
    fn clear_along_layers(&mut self) {
        let (along, _) = self.axes();
        for _ in 0..self.node_count {
            let near = self.near_pairs(OVERLAP_MARGIN);
            let mut pushed = false;
            for pair in &near {
                let (a, b) = (pair.a as usize, pair.b as usize);
                let place = |i: usize| self.layout_pos[i] + self.target[i];
                let d = place(b) - place(a);
                let ext = self.half_size[a] + self.half_size[b];
                if !boxes_overlap(d, ext) {
                    continue;
                }
                let apart = d.dot(along);
                let last = if apart < 0.0 { a } else { b };
                let short = ext.dot(along) + OVERLAP_MARGIN - apart.abs();
                self.activate(last);
                self.target[last] += along * short;
                pushed = true;
            }
            if !pushed {
                break;
            }
        }
    }

    /// Nodes whose boxes overlap at rest, where at least one of the two rests away from the
    /// layout (the layout itself leaves none).
    fn overlapping(&self) -> Vec<u32> {
        let pairs = self.overlapping_pairs(|i| self.layout_pos[i] + self.home[i]);
        sorted_unique(
            pairs
                .into_iter()
                .filter(|&(a, b)| self.is_displaced(a as usize) || self.is_displaced(b as usize))
                .flat_map(|(a, b)| [a, b]),
        )
    }

    /// Pairs of nodes whose boxes, centred at `centre`, overlap by more than a graze.
    fn overlapping_pairs(&self, centre: impl Fn(usize) -> Point) -> Vec<(u32, u32)> {
        let boxes: Vec<(u32, Point, Point)> = (0..self.node_count)
            .map(|i| (i as u32, centre(i), self.half_size[i]))
            .collect();
        let mut grid = Grid::default();
        grid.rebuild(&boxes);
        let mut pairs = grid.candidate_pairs();
        pairs.retain(|&(a, b)| {
            let (a, b) = (a as usize, b as usize);
            boxes_overlap(centre(b) - centre(a), self.half_size[a] + self.half_size[b])
        });
        pairs
    }

    /// Adds to `shifts`, which holds how far the saved nodes move, the new commits among the
    /// others (see [`Net::restore`]).
    fn place_new_nodes(&self, shifts: &mut BTreeMap<u32, Point>) {
        let rest = |shifts: &BTreeMap<u32, Point>, p: u32| {
            self.home[p as usize] + shifts.get(&p).copied().unwrap_or_default()
        };
        // The new commits: children of saved nodes that rest away, and their children in turn.
        let mut new_commits: Vec<u32> = Vec::new();
        let mut seen: HashSet<u32> = HashSet::new();
        let mut queue: Vec<u32> = shifts
            .keys()
            .copied()
            .filter(|&p| rest(shifts, p) != Point::default())
            .collect();
        while let Some(p) = queue.pop() {
            for c in self.children(p as usize) {
                if !shifts.contains_key(&c) && seen.insert(c) {
                    new_commits.push(c);
                    queue.push(c);
                }
            }
        }
        // Parents first: they lie farther along the flow.
        let along_flow = |p: u32| self.layout_pos[p as usize].dot(self.flow);
        new_commits.sort_unstable_by(|&a, &b| along_flow(b).total_cmp(&along_flow(a)));
        for c in new_commits {
            let ci = c as usize;
            let first = self.first_parent[ci];
            let mut d = if first == NO_NODE {
                self.home[ci]
            } else {
                rest(shifts, first)
            };
            for p in self.parents(ci) {
                let pi = p as usize;
                let apart = (self.layout_pos[pi] - self.layout_pos[ci]).dot(self.flow);
                if apart <= 0.0 {
                    continue;
                }
                let needed = self.kept_flow_gap(ci, pi, apart);
                let have = apart + (rest(shifts, p) - d).dot(self.flow);
                if have < needed {
                    d -= self.flow * (needed - have);
                }
            }
            shifts.insert(c, d - self.home[ci]);
        }
    }

    /// Rest positions after shifting nodes by `shifts`, with the bend points of their edges
    /// shifted in proportion: (particle, before, after).
    fn shifted_homes(&self, shifts: &BTreeMap<u32, Point>) -> Vec<(u32, Point, Point)> {
        let mut out: Vec<(u32, Point, Point)> = shifts
            .iter()
            .filter(|&(_, &d)| d != Point::default())
            .map(|(&node, &d)| {
                let h = self.home[node as usize];
                (node, h, h + d)
            })
            .collect();
        let edges = self.edges_at(shifts.keys().copied());
        let shift_of = |node| shifts.get(&node).copied().unwrap_or_default();
        for (p, s) in self.bend_shifts(&edges, shift_of) {
            if s != Point::default() {
                let h = self.home[p as usize];
                out.push((p, h, h + s));
            }
        }
        out
    }

    /// How far the bend points of `edges` move when their end nodes move by `shift_of`, sorted
    /// by bend point: each edge's bend points share the movement of its ends in proportion to
    /// their distance along the edge. A bend point shared by several bundled edges gets the
    /// average over all of them.
    fn bend_shifts(&self, edges: &[u32], shift_of: impl Fn(u32) -> Point) -> Vec<(u32, Point)> {
        let n = self.node_count;
        let mut shares: Vec<(u32, Point)> = Vec::new();
        let rest = |p: u32| (self.layout_pos[p as usize], self.home[p as usize]);
        let mut rest_distance = Vec::new();
        for &e in edges {
            let chain = &self.chains[e as usize];
            let last = chain.len() - 1;
            let (sa, sb) = (shift_of(chain[0]), shift_of(chain[last]));
            if last < 2 || (sa == Point::default() && sb == Point::default()) {
                continue;
            }
            rest_distance.clear();
            rest_distance.push(0.0f32);
            for w in chain.windows(2) {
                let ((oa, ha), (ob, hb)) = (rest(w[0]), rest(w[1]));
                let step = (ob - oa + (hb - ha)).length();
                rest_distance.push(rest_distance[rest_distance.len() - 1] + step);
            }
            let total = rest_distance[last];
            for (k, &p) in chain.iter().enumerate().take(last).skip(1) {
                let t = if total > 0.0 {
                    rest_distance[k] / total
                } else {
                    k as f32 / last as f32
                };
                let s = sa * (1.0 - t) + sb * t;
                let share = s * (1.0 / self.edges_through_bend[p as usize - n] as f32);
                shares.push((p, share));
            }
        }
        shares.sort_by_key(|&(p, _)| p);
        shares.dedup_by(|(p, share), (kept, sum)| {
            let same = p == kept;
            if same {
                *sum += *share;
            }
            same
        });
        shares
    }

    fn activate(&mut self, p: usize) {
        let t = self.target[p];
        self.activate_heading(p, t);
    }

    /// Wakes particle `p`, heading for `target`: for while `self.target` is taken out to relax.
    fn activate_heading(&mut self, p: usize, target: Point) {
        if !self.is_active[p] {
            self.is_active[p] = true;
            self.frame_start_target[p] = target;
            self.active.push(p as u32);
            self.active_generation += 1;
        }
    }

    /// Wakes the particles nearest to `from`, breadth-first over springs, up to `budget` more
    /// than are awake already.
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
            for &s in self.springs_at.get(q) {
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
            self.velocity[p] = Point::default();
            self.disp[p] = self.target[p];
            self.wedged[p] = 0;
        }
        self.active.clear();
        self.active_generation += 1;
    }

    /// Advances the simulation by `dt` seconds; true while anything still moves.
    pub fn step(&mut self, dt: f32, params: &NetParams) -> bool {
        if !self.is_awake() {
            return false;
        }
        let dt = dt.clamp(1.0 / 240.0, 1.0 / 30.0);
        self.shape(params, SWEEPS_PER_FRAME);

        // Follow the targets through damped springs, by semi-implicit Euler. Held particles, and
        // the stretched edges of a drag that doesn't adapt, follow exactly.
        if let Some(g) = &self.grab
            && !g.adapts
        {
            for &p in &g.stretched_bends {
                self.disp[p as usize] = self.target[p as usize];
                self.velocity[p as usize] = Point::default();
            }
        }
        let zeta = if self.wobbly {
            1.0 - 0.75 * params.wobble.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let omega = std::f32::consts::TAU * FOLLOW_FREQUENCY_HZ;
        let h = dt / FOLLOW_SUBSTEPS as f32;
        let mut max_speed = 0.0f32;
        let mut max_distance = 0.0f32;
        for &i in &self.active {
            let i = i as usize;
            if self.is_held[i] {
                self.disp[i] = self.target[i];
                self.velocity[i] = Point::default();
                continue;
            }
            for _ in 0..FOLLOW_SUBSTEPS {
                let pull = (self.target[i] - self.disp[i]) * (omega * omega);
                let drag = self.velocity[i] * (2.0 * zeta * omega);
                self.velocity[i] += (pull - drag) * h;
                self.disp[i] += self.velocity[i] * h;
            }
            max_speed = max_speed.max(self.velocity[i].length());
            max_distance = max_distance.max((self.target[i] - self.disp[i]).length());
        }
        self.awake = max_speed > MAX_SLEEP_SPEED || max_distance > MAX_SLEEP_DISTANCE;
        let settled = !self.awake && self.grab.is_none();
        let moved: Vec<u32> = self
            .active
            .iter()
            .copied()
            .filter(|&i| {
                let i = i as usize;
                i < self.node_count
                    && (self.disp[i] - self.disp_in_obstacles[i]).length() > ROUTE_UPDATE_STEP
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

    fn obstacles(&mut self) -> &mut Obstacles {
        if self.obstacles.is_none() {
            let mut o = Obstacles::new(self.node_count);
            for i in 0..self.node_count {
                o.place(i, self.pos(i), self.half_size[i]);
                self.disp_in_obstacles[i] = self.disp[i];
            }
            self.obstacles = Some(o);
        }
        self.obstacles.as_mut().expect("just set up")
    }

    /// Brings routes up to date after the `moved` nodes have moved: those of their edges, and
    /// those of routed edges passing where they were or are now. An edge a node has just left
    /// may have been routed round it, and has to be looked at again even if the node jumped
    /// clear of it in one frame.
    fn update_routes(&mut self, moved: &[u32]) {
        if moved.is_empty() {
            return;
        }
        self.trunk_cache.clear();
        let mut area: Option<(Point, Point)> = None;
        for &node in moved {
            let i = node as usize;
            let (centre, half_size) = (self.pos(i), self.half_size[i]);
            let obstacles = self.obstacles();
            let was = obstacles.box_of(i);
            obstacles.place(i, centre, half_size);
            let now = obstacles.box_of(i);
            self.disp_in_obstacles[i] = self.disp[i];
            area = bounding_box(area.into_iter().chain(was).chain(now));
        }
        let mut edges: Vec<u32> = moved
            .iter()
            .flat_map(|&node| self.node_edges[node as usize].iter().copied())
            .collect();
        if let Some((lo, hi)) = area {
            for e in 0..self.own_routes.len() {
                if self.own_routes[e].is_some() {
                    let (a, b) = self.span(e);
                    if a.x <= hi.x && b.x >= lo.x && a.y <= hi.y && b.y >= lo.y {
                        edges.push(e as u32);
                    }
                }
            }
        }
        self.refresh_routes(edges);
    }

    /// Refreshes the routes of `edges`, and of all edges bundled with them, which have to agree
    /// on how their trunk runs.
    fn refresh_routes(&mut self, mut edges: Vec<u32>) {
        let bundles: Vec<u32> = edges
            .iter()
            .map(|&e| self.bundle_of[e as usize])
            .filter(|&b| b != NO_BUNDLE)
            .collect();
        for b in bundles {
            edges.extend_from_slice(&self.bundle_edges[b as usize]);
        }
        for e in sorted_unique(edges) {
            self.refresh_route(e as usize);
        }
    }

    /// Bounding box of edge `e` as drawn, and of its layout route, which [`Net::blocked`]
    /// looks at and a route of its own may leave far behind.
    fn span(&self, e: usize) -> (Point, Point) {
        let layout_route = self.chains[e].iter().map(|&p| self.pos(p as usize));
        let pts = self.edge_points(e).chain(layout_route);
        bounding_box(pts.map(|p| (p, p))).expect("edges have two ends")
    }

    /// Routes edge `e` afresh if its layout route no longer fits: its nodes have moved
    /// relative to each other, a moved node covers it, or its trunk has been routed afresh.
    fn refresh_route(&mut self, e: usize) {
        let (c, p) = self.ends(e);
        let stretched = self.stretched(c, p, self.is_rearranged(c) || self.is_rearranged(p));
        let bundle = self.bundle_of[e];
        let trunk = (bundle != NO_BUNDLE).then(|| self.trunk(bundle));
        let trunk_routed = matches!(trunk, Some(Trunk::Routed(_)));
        if !(stretched || trunk_routed || self.blocked(e)) {
            self.turned[e] = false;
            self.own_routes[e] = None;
            return;
        }
        let a = self.exit(c);
        let b = self.entry(p);
        let flows_vertically = self.flows_vertically;
        self.turned[e] = self.is_reversed(e);
        let route = if self.turned[e] {
            // Keep the usual sides, and run round both boxes from just past the child to just
            // before the parent.
            let a = a + self.flow * route::TURN_LENGTH;
            let b = b - self.flow * route::TURN_LENGTH;
            let ends = [route::NO_END; 2];
            let mut pts = vec![a];
            pts.extend(route::route(self.obstacles(), flows_vertically, a, b, ends));
            pts.push(b);
            Route {
                bends: pts,
                joins_trunk_at: None,
            }
        } else {
            // A bundled edge joins its trunk at the first point far enough on from the child.
            let ahead = |q: Point| (q - a).dot(self.flow) >= FLOW_GAP;
            let ends = [c as u32, route::NO_END];
            match trunk {
                Some(Trunk::Layout) => {
                    let join = self.trunks[bundle as usize]
                        .iter()
                        .map(|&t| self.pos(t as usize))
                        .enumerate()
                        .find(|&(_, q)| ahead(q));
                    join.map(|(j, at)| Route {
                        bends: route::route(self.obstacles(), flows_vertically, a, at, ends),
                        joins_trunk_at: Some(j as u32),
                    })
                }
                Some(Trunk::Routed(pts)) => pts.iter().position(|&q| ahead(q)).map(|j| {
                    let mut bends =
                        route::route(self.obstacles(), flows_vertically, a, pts[j], ends);
                    bends.extend_from_slice(&pts[j..]);
                    Route {
                        bends,
                        joins_trunk_at: None,
                    }
                }),
                Some(Trunk::Broken) | None => None,
            }
            .unwrap_or_else(|| Route {
                bends: route::route(
                    self.obstacles(),
                    flows_vertically,
                    a,
                    b,
                    [c as u32, p as u32],
                ),
                joins_trunk_at: None,
            })
        };
        self.own_routes[e] = Some(route);
    }

    /// True if particles `a` and `b` have moved too far relative to each other for the
    /// layout's route between them: less far if one of them is `rearranged`.
    fn stretched(&self, a: usize, b: usize, rearranged: bool) -> bool {
        let limit = if rearranged {
            REARRANGED_STRETCH_LIMIT
        } else {
            STRETCH_LIMIT
        };
        (self.disp[b] - self.disp[a]).length() > limit
    }

    /// Where edges leave node `i`: its side facing along the flow.
    fn exit(&self, i: usize) -> Point {
        self.pos(i) + self.flow * self.half_depth(i)
    }

    /// Where edges enter node `i`: its side facing against the flow.
    fn entry(&self, i: usize) -> Point {
        self.pos(i) - self.flow * self.half_depth(i)
    }

    fn half_depth(&self, i: usize) -> f32 {
        self.flow.x.abs() * self.half_size[i].x + self.flow.y.abs() * self.half_size[i].y
    }

    /// How the edges of bundle `b` run into their parent, worked out once per routing pass:
    /// along the layout's trunk, unless the parent has moved relative to it or a moved node
    /// covers it.
    fn trunk(&mut self, b: u32) -> Trunk {
        if let Some(t) = self.trunk_cache.get(&b) {
            return t.clone();
        }
        let trunk = &self.trunks[b as usize];
        let (_, p) = self.ends(self.bundle_edges[b as usize][0] as usize);
        let (first, last) = (trunk[0] as usize, trunk[trunk.len() - 1] as usize);
        let start = self.pos(first);
        let end = self.entry(p);
        let state = if (end - start).dot(self.flow) < FLOW_GAP {
            Trunk::Broken
        } else {
            let pts: Vec<Point> = trunk
                .iter()
                .map(|&t| self.pos(t as usize))
                .chain(std::iter::once(self.pos(p)))
                .collect();
            if self.stretched(last, p, self.is_rearranged(p))
                || self.covered(&pts, [route::NO_END, p as u32])
            {
                let flows_vertically = self.flows_vertically;
                let ends = [route::NO_END, p as u32];
                let mut pts = vec![start];
                pts.extend(route::route(
                    self.obstacles(),
                    flows_vertically,
                    start,
                    end,
                    ends,
                ));
                Trunk::Routed(pts)
            } else {
                Trunk::Layout
            }
        };
        self.trunk_cache.insert(b, state.clone());
        state
    }

    fn is_rearranged(&self, i: usize) -> bool {
        self.moved_by_hand[i] || self.is_held[i] || self.disp[i].length() > STRETCH_LIMIT
    }

    /// True if a rearranged node covers the layout route of edge `e`.
    fn blocked(&mut self, e: usize) -> bool {
        let (c, p) = self.ends(e);
        let pts: Vec<Point> = self.chains[e]
            .iter()
            .map(|&q| self.pos(q as usize))
            .collect();
        self.covered(&pts, [c as u32, p as u32])
    }

    /// True if a rearranged node other than `ends` covers the line through `pts`.
    fn covered(&mut self, pts: &[Point], ends: [u32; 2]) -> bool {
        let Some(mut obstacles) = self.obstacles.take() else {
            return false;
        };
        let mut hit = false;
        for w in pts.windows(2) {
            obstacles.near_segment(w[0], w[1], |i| {
                if hit || ends.contains(&i) {
                    return;
                }
                let i = i as usize;
                if !self.is_rearranged(i) || self.disp[i].length() <= ROUTE_UPDATE_STEP {
                    return;
                }
                hit = self.segment_enters(w[0], w[1], i, GRAZE_TOLERANCE);
            });
            if hit {
                break;
            }
        }
        self.obstacles = Some(obstacles);
        hit
    }

    /// Routes afresh every edge that a moved node now covers. Only once the net is at rest:
    /// while dragging, only the edges at moving nodes are looked at.
    fn route_blocked_edges(&mut self) {
        if self.obstacles.is_none() || !(0..self.node_count).any(|i| self.is_rearranged(i)) {
            return;
        }
        self.trunk_cache.clear();
        let blocked: Vec<u32> = (0..self.own_routes.len())
            .filter(|&e| self.own_routes[e].is_none() && self.blocked(e))
            .map(|e| e as u32)
            .collect();
        self.refresh_routes(blocked);
    }

    /// Sets where every woken particle is heading: the *shape* of the module docs.
    fn shape(&mut self, params: &NetParams, sweeps: usize) {
        match self.grab.as_ref().map(|g| g.adapts) {
            None => {
                for &i in &self.active {
                    self.target[i as usize] = self.home[i as usize];
                }
            }
            Some(false) => self.stretch_to_grab(),
            Some(true) => self.relax(params, sweeps),
        }
    }

    /// The shape for a drag that doesn't adapt: everything else heads for (or stays at) its
    /// rest position, and the edges at the dragged nodes stretch evenly.
    fn stretch_to_grab(&mut self) {
        let grab = self.grab.as_ref().expect("dragging");
        for &i in &self.active {
            let i = i as usize;
            if !self.is_held[i] {
                self.target[i] = self.home[i];
            }
        }
        for &(p, start) in &grab.held_starts {
            self.target[p as usize] = start + grab.delta;
        }
        let shifts = self.bend_shifts(&grab.stretched_edges, |node| {
            self.target[node as usize] - self.home[node as usize]
        });
        for (p, s) in shifts {
            if !self.is_held[p as usize] {
                self.target[p as usize] = self.home[p as usize] + s;
            }
        }
    }

    /// The shape for an adaptive drag (see the module docs).
    fn relax(&mut self, params: &NetParams, sweeps: usize) {
        let grab = self.grab.as_ref().expect("dragging");
        // Where everything was heading at the start of the frame decides on which side of each
        // other things stay.
        for &i in &self.active {
            self.frame_start_target[i as usize] = self.target[i as usize];
        }
        for &(p, start) in &grab.held_starts {
            self.target[p as usize] = start + grab.delta;
        }
        let lambda = 0.3 + 0.6 * params.pull.clamp(0.0, 1.0);
        let push = params.push.clamp(0.0, 1.0);
        let magnet_range = MAX_MAGNET_RANGE * push;
        let forces = Forces {
            k_anchor: (1.0 - lambda) * (1.0 - lambda) / lambda,
            k_magnet: MAX_MAGNET_STIFFNESS * push,
            weft_reach: magnet_range + OVERLAP_MARGIN,
        };
        let near = if push > 0.0 || params.avoid_overlap {
            self.near_pairs(magnet_range + NEAR_PAIRS_SLACK)
        } else {
            Vec::new()
        };
        self.collect_magnets(&near, magnet_range, forces.k_magnet > 0.0);

        let mut segments = std::mem::take(&mut self.segments);
        if segments.active_generation != Some(self.active_generation) {
            segments = self.flow_segments();
        }
        let mut target = std::mem::take(&mut self.target);
        for sweep in 0..sweeps {
            for &i in &self.active {
                let i = i as usize;
                if !self.is_held[i] {
                    target[i] = self.relaxed(i, &target, &near, &forces);
                }
            }
            if params.avoid_overlap && sweep % 4 == 3 {
                self.separate_nodes(&mut target, &near);
            }
            // Every other sweep is enough, as long as it includes the last one and those
            // right before and after separating (which needs to know what is wedged).
            if sweep % 2 == 1 {
                let find_wedged = params.avoid_overlap && sweep % 4 == 1;
                self.keep_flow_order(&segments, find_wedged, &mut target);
            }
        }
        self.target = target;
        self.segments = segments;
        for &node in &self.nodes_with_magnets {
            self.magnets[node as usize].clear();
        }
        self.nodes_with_magnets.clear();
    }

    /// Where particle `i` heads given where the others do: the mean of where its anchor, its
    /// springs and its magnets want it, weighted by their stiffness.
    fn relaxed(&self, i: usize, target: &[Point], near: &[Near], forces: &Forces) -> Point {
        let n = self.node_count;
        // Bend points hold on to their rest less, so edges bend before nodes move.
        let k_home = if i < n {
            forces.k_anchor
        } else {
            forces.k_anchor * 0.5
        };
        let mut num = self.home[i] * k_home;
        let mut den = k_home;
        for &si in self.springs_at.get(i) {
            let s = &self.springs[si as usize];
            if let Some(want) = self.spring_want(s, i, target, forces.weft_reach) {
                num += want * s.stiffness;
                den += s.stiffness;
            }
        }
        if i < n {
            for magnet in &self.magnets[i] {
                let pair = &near[magnet.pair as usize];
                let (gap, away) = self.gap(pair, target, 0.0);
                if gap < magnet.kept_gap {
                    let away = if pair.b as usize == i { away } else { -away };
                    let want = target[i] + away * (magnet.kept_gap - gap);
                    num += want * forces.k_magnet;
                    den += forces.k_magnet;
                }
            }
        }
        num * (1.0 / den)
    }

    /// Where spring `s` wants its end `i`, if it pulls at all.
    fn spring_want(
        &self,
        s: &Spring,
        i: usize,
        target: &[Point],
        weft_reach: f32,
    ) -> Option<Point> {
        let n = self.node_count;
        let (a, b) = (s.a as usize, s.b as usize);
        let rest = self.home[b] - self.home[a];
        if s.along_edge {
            return Some(if i == a {
                target[b] - rest
            } else {
                target[a] + rest
            });
        }
        // The weft keeps edges from being pushed into each other or through nodes (the magnets
        // keep nodes apart). It only resists coming closer than at rest, and only once near.
        if a < n && b < n {
            return None;
        }
        let apart = s.layout_offset + rest;
        let dist = apart.length();
        if dist == 0.0 {
            return None;
        }
        let mut u = apart * (1.0 / dist);
        let ext = self.half_size[a] + self.half_size[b];
        let reach = u.x.abs() * ext.x + u.y.abs() * ext.y + weft_reach;
        // Keep them on the side they were on; once something has passed, it stays passed.
        let was = |p: usize| self.was_heading(p, target);
        let passed = (s.layout_offset + (was(b) - was(a))).dot(u) < 0.0;
        let keep = if passed {
            u = -u;
            reach
        } else {
            dist.min(reach)
        };
        let now = (s.layout_offset + (target[b] - target[a])).dot(u);
        if now >= keep {
            return None;
        }
        let push = u * (keep - now);
        Some(if i == a {
            target[a] - push
        } else {
            target[b] + push
        })
    }

    /// Where particle `p` was heading at the start of the frame, if awake, given `target`.
    fn was_heading(&self, p: usize, target: &[Point]) -> Point {
        if self.is_active[p] {
            self.frame_start_target[p]
        } else {
            target[p]
        }
    }

    /// Pairs of nodes whose boxes are less than `pad` apart, at least one of them away from
    /// its rest position (two resting nodes can't press on each other), and how they sat at
    /// the start of the frame.
    fn near_pairs(&mut self, pad: f32) -> Vec<Near> {
        let n = self.node_count;
        let place = |i: usize| self.layout_pos[i] + self.target[i];
        let moving = |i: usize| {
            self.is_held[i]
                || (self.target[i] - self.home[i]).length() > REST_EPSILON
                || self.is_settling && self.home[i] != Point::default()
        };
        // Only nodes near a moving one: in the coarse cells that the moving boxes, grown by
        // `pad`, touch. One box around them all would take in most of a big graph when they are
        // far apart.
        let cells_of = |p: Point, h: Point| {
            let cell = |v: f32| (v / NEAR_GRID_CELL).floor() as i32;
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
                let grown = self.half_size[i] + Point::new(pad, pad);
                area.extend(cells_of(place(i), grown));
            }
        }
        if area.is_empty() {
            return Vec::new();
        }
        let items: Vec<(u32, Point, Point)> = (0..n)
            .filter(|&i| cells_of(place(i), self.half_size[i]).any(|c| area.contains(&c)))
            .map(|i| {
                let grown = self.half_size[i] + Point::new(pad / 2.0, pad / 2.0);
                (i as u32, place(i), grown)
            })
            .collect();
        self.grid.rebuild(&items);
        let pairs = self.grid.candidate_pairs();
        let (along, across) = self.axes();
        pairs
            .into_iter()
            .filter_map(|(a, b)| {
                let (a, b) = (a as usize, b as usize);
                if !moving(a) && !moving(b) {
                    return None;
                }
                let d = self.layout_pos[b] - self.layout_pos[a] + (self.target[b] - self.target[a]);
                let ext = self.half_size[a] + self.half_size[b];
                if d.x.abs() >= ext.x + pad || d.y.abs() >= ext.y + pad {
                    return None;
                }
                // Side by side in a row: they keep their order along it.
                let was = |p: usize| self.was_heading(p, &self.target);
                let d = self.layout_pos[b] - self.layout_pos[a] + (was(b) - was(a));
                let in_row = d.dot(across).abs() < ext.dot(across);
                let side = match (in_row, d.dot(along) < 0.0) {
                    (false, _) => 0.0,
                    (true, true) => -1.0,
                    (true, false) => 1.0,
                };
                let rest = self.layout_pos[b] - self.layout_pos[a] + (self.home[b] - self.home[a]);
                // Settling clears overlaps left at rest too: they weren't left so by hand.
                let rest_gap = box_gap(rest, ext).0;
                Some(Near {
                    a: a as u32,
                    b: b as u32,
                    side,
                    rest_gap: if self.is_settling {
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
        if self.flows_vertically {
            (Point::new(1.0, 0.0), Point::new(0.0, 1.0))
        } else {
            (Point::new(0.0, 1.0), Point::new(1.0, 0.0))
        }
    }

    /// Gap between the boxes of a pair of nodes, grown by `margin`, at displacements `t`; and
    /// the unit direction in which moving `b` widens it. Nodes side by side in a row measure it
    /// along the row, in their order.
    fn gap(&self, pair: &Near, t: &[Point], margin: f32) -> (f32, Point) {
        let (a, b) = (pair.a as usize, pair.b as usize);
        let d = self.layout_pos[b] - self.layout_pos[a] + (t[b] - t[a]);
        let ext = self.half_size[a] + self.half_size[b] + Point::new(margin, margin);
        if pair.side != 0.0 {
            let (along, _) = self.axes();
            let gap = d.dot(along) * pair.side - ext.dot(along);
            (gap, along * pair.side)
        } else {
            box_gap(d, ext)
        }
    }

    /// Sets up this frame's magnets between the near pairs, which keep the gap they have at
    /// rest, or `range` if that is smaller; and wakes the resting nodes pressed on.
    fn collect_magnets(&mut self, near: &[Near], range: f32, magnets_on: bool) {
        for (k, pair) in near.iter().enumerate() {
            let (a, b) = (pair.a as usize, pair.b as usize);
            if self.is_held[a] && self.is_held[b] {
                continue;
            }
            let kept_gap = pair.rest_gap.min(range);
            // Wake a resting node only once something actually presses on it.
            let now = self.gap(pair, &self.target, 0.0).0;
            let pressed = now < pair.margin() || magnets_on && now < kept_gap - REST_EPSILON;
            if pressed {
                for p in [a, b] {
                    if !self.is_active[p] {
                        self.wake_around(&[p as u32], PUSH_WAKE_BUDGET);
                    }
                }
            }
            if magnets_on {
                for p in [a, b] {
                    if self.magnets[p].is_empty() {
                        self.nodes_with_magnets.push(p as u32);
                    }
                    self.magnets[p].push(Magnet {
                        pair: k as u32,
                        kept_gap,
                    });
                }
            }
        }
    }

    /// Pushes overlapping node boxes apart, in `target`: nodes side by side in a row along the
    /// row, in their order, so that they never get pushed past each other; others along their
    /// axis of least overlap. A resting node that gets hit is woken.
    fn separate_nodes(&mut self, target: &mut [Point], near: &[Near]) {
        for pair in near {
            let (gap, away) = self.gap(pair, target, pair.margin());
            if gap >= 0.0 {
                continue;
            }
            let (a, b) = (pair.a as usize, pair.b as usize);
            self.activate_heading(a, target[a]);
            self.activate_heading(b, target[b]);
            let push = away * -gap;
            let (mut wa, mut wb) = (self.yields(a, -push), self.yields(b, push));
            if wa + wb == 0.0 && self.is_settling && self.is_held[a] && self.is_held[b] {
                // Two nodes moved by hand, put back onto each other: both give way.
                (wa, wb) = (1.0, 1.0);
            }
            if wa + wb == 0.0 {
                continue;
            }
            let (da, db) = (push * (-wa / (wa + wb)), push * (wb / (wa + wb)));
            for (i, d) in [(a, da), (b, db)] {
                target[i] += d;
                let along = d.dot(self.flow);
                if along > 0.0 {
                    self.separated[i] |= ALONG_FLOW;
                } else if along < 0.0 {
                    self.separated[i] |= AGAINST_FLOW;
                }
            }
        }
    }

    /// 1 if particle `i` may be pushed in direction `dir` to clear an overlap, else 0: held
    /// particles stay put, and so do those wedged against them that way, or the two would
    /// fight and the overlap would remain.
    fn yields(&self, i: usize, dir: Point) -> f32 {
        let along = dir.dot(self.flow);
        let wedged = self.wedged[i];
        let blocked = self.is_held[i]
            || (along > 0.0 && wedged & ALONG_FLOW != 0)
            || (along < 0.0 && wedged & AGAINST_FLOW != 0);
        if blocked { 0.0 } else { 1.0 }
    }

    /// The edge segments at the woken particles that [`Net::keep_flow_order`] looks after.
    fn flow_segments(&self) -> FlowSegments {
        let mut forward = Vec::new();
        for &i in &self.active {
            for &si in self.springs_at.get(i as usize) {
                let s = &self.springs[si as usize];
                // Each segment once: from its newer end, or its older end if the newer rests.
                let from_here = s.a == i || (s.b == i && !self.is_active[s.a as usize]);
                if from_here && s.along_edge && s.a != s.b && s.layout_offset.dot(self.flow) > 0.0 {
                    forward.push(si);
                }
            }
        }
        let sorted = |end: fn(&Spring) -> u32, sign: f32| {
            let mut keyed: Vec<(f32, u32)> = forward
                .iter()
                .map(|&si| {
                    let p = end(&self.springs[si as usize]) as usize;
                    (sign * self.layout_pos[p].dot(self.flow), si)
                })
                .collect();
            keyed.sort_unstable_by(|x, y| x.0.total_cmp(&y.0));
            keyed.into_iter().map(|(_, si)| si).collect()
        };
        FlowSegments {
            forward: sorted(|s| s.a, 1.0),
            backward: sorted(|s| s.b, -1.0),
            active_generation: Some(self.active_generation),
        }
    }

    /// Keeps every edge segment pointing along the flow, so children stay above their parents:
    /// the older end stays at least [`FLOW_GAP`] beyond the newer one (box borders, for nodes),
    /// or as far as at rest if that is less. Segments resting reversed (left so by the user)
    /// are left alone, held particles never give way, and a resting particle pushed is woken.
    ///
    /// One pass pushes older ends on and one pushes newer ends back, each in an order where
    /// every push is final, so together they resolve whole chains. A particle that
    /// [`Net::separate_nodes`] just pushed the other way is spared if possible, so that the
    /// other end gives way instead; where an overlap and the order can't both be cleared, the
    /// order wins. With `find_wedged`, [`Net::wedged`] then records which particles are wedged
    /// against a held one.
    fn keep_flow_order(
        &mut self,
        segments: &FlowSegments,
        find_wedged: bool,
        target: &mut [Point],
    ) {
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
        if !find_wedged {
            return;
        }
        // A particle can't move on along the flow if it is held, or if a taut segment ties it
        // to an older particle that can't either; likewise back.
        let held = |net: &Net, i: usize| {
            if net.is_held[i] {
                ALONG_FLOW | AGAINST_FLOW
            } else {
                0
            }
        };
        for &i in &self.active {
            self.wedged[i as usize] = held(self, i as usize);
        }
        for &si in &segments.forward {
            let s = self.springs[si as usize];
            for p in [s.a, s.b] {
                self.wedged[p as usize] = held(self, p as usize);
            }
        }
        let taut = |net: &Net, s: &Spring| net.slack(s, target).is_some_and(|x| x < 0.5);
        for &si in &segments.backward {
            let s = self.springs[si as usize];
            if self.wedged[s.b as usize] & ALONG_FLOW != 0 && taut(self, &s) {
                self.wedged[s.a as usize] |= ALONG_FLOW;
            }
        }
        for &si in &segments.forward {
            let s = self.springs[si as usize];
            if self.wedged[s.a as usize] & AGAINST_FLOW != 0 && taut(self, &s) {
                self.wedged[s.b as usize] |= AGAINST_FLOW;
            }
        }
    }

    /// How much farther along the flow than needed the older end of an edge segment is, or
    /// `None` if the segment isn't kept in order (see [`Net::keep_flow_order`]).
    fn slack(&self, s: &Spring, target: &[Point]) -> Option<f32> {
        let (a, b) = (s.a as usize, s.b as usize);
        let flow = self.flow;
        let rest = (s.layout_offset + (self.home[b] - self.home[a])).dot(flow);
        if !s.along_edge || a == b || s.layout_offset.dot(flow) <= 0.0 || rest <= 0.0 {
            return None;
        }
        let needed = self.kept_flow_gap(a, b, rest);
        let have = (self.layout_pos[b] + target[b] - (self.layout_pos[a] + target[a])).dot(flow);
        Some(have - needed)
    }

    /// How far along the flow the older end `b` of an edge segment stays beyond its newer end
    /// `a`, where they rest `rest_apart` apart.
    fn kept_flow_gap(&self, a: usize, b: usize, rest_apart: f32) -> f32 {
        (self.half_depth(a) + self.half_depth(b) + FLOW_GAP).min(rest_apart)
    }

    /// Puts segment `s` back in order by moving its `end` (`s.b` on along the flow, or `s.a`
    /// back), unless that end is held or, with `spare_separated`, was just pushed the other
    /// way by [`Net::separate_nodes`]. Returns true if the latter left it out of order.
    fn push_segment(
        &mut self,
        s: &Spring,
        end: usize,
        spare_separated: bool,
        target: &mut [Point],
    ) -> bool {
        if self.is_held[end] {
            return false;
        }
        let Some(slack) = self.slack(s, target).filter(|&x| x < 0.0) else {
            return false;
        };
        let forward = end == s.b as usize;
        let against = if forward { AGAINST_FLOW } else { ALONG_FLOW };
        if spare_separated && self.separated[end] & against != 0 {
            return true;
        }
        let was = target[end];
        let push = self.flow * (if forward { -slack } else { slack });
        target[end] = was + push;
        self.activate_heading(end, was);
        false
    }
}

/// Where a node rests, for saving and putting back (see [`Net::rest_places`] and
/// [`Net::restore`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RestPlace {
    pub node: usize,
    /// From the node's place in the layout.
    pub offset: Point,
    pub by_hand: bool,
    /// The node's first parent, and the node's offset from it: a layout of a changed graph may
    /// put the two elsewhere relative to each other.
    pub beside: Option<(usize, Point)>,
}

/// What makes a rearranged graph hard to read. Reversed edges, which run round their nodes by
/// design, count as neither doubling back nor detouring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tidiness {
    pub overlaps: usize,
    pub edges_through_boxes: usize,
    pub edges_doubling_back: usize,
    pub edges_detouring: usize,
}

/// Edge segments (springs) for [`Net::keep_flow_order`], sorted by where their newer end lies
/// along the flow, and by where their older end lies against it.
#[derive(Clone, Debug, Default)]
struct FlowSegments {
    forward: Vec<u32>,
    backward: Vec<u32>,
    active_generation: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
struct Near {
    a: u32,
    b: u32,
    /// ±1 if they sat side by side in a row at the start of the frame, `b` after or before `a`
    /// along it; 0 otherwise.
    side: f32,
    rest_gap: f32,
}

impl Near {
    /// The gap that overlap avoidance keeps: the margin, or less if they rest closer.
    fn margin(&self) -> f32 {
        self.rest_gap.min(OVERLAP_MARGIN)
    }
}

/// Gap between two boxes whose centres are `d` apart and whose half sizes add up to `ext`
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

/// True if two boxes whose centres are `d` apart and whose half sizes add up to `ext` overlap
/// by more than a graze.
fn boxes_overlap(d: Point, ext: Point) -> bool {
    d.x.abs() < ext.x - GRAZE_TOLERANCE && d.y.abs() < ext.y - GRAZE_TOLERANCE
}

/// The smallest box, as (min corner, max corner), around `boxes`.
fn bounding_box(boxes: impl IntoIterator<Item = (Point, Point)>) -> Option<(Point, Point)> {
    boxes.into_iter().reduce(|(lo, hi), (blo, bhi)| {
        (
            Point::new(lo.x.min(blo.x), lo.y.min(blo.y)),
            Point::new(hi.x.max(bhi.x), hi.y.max(bhi.y)),
        )
    })
}

fn sorted_unique(items: impl IntoIterator<Item = u32>) -> Vec<u32> {
    let mut items: Vec<u32> = items.into_iter().collect();
    items.sort_unstable();
    items.dedup();
    items
}

#[derive(Clone, Debug, Default)]
struct Grid {
    cell_size: f32,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl Grid {
    /// Indexes boxes given as (id, centre, half size).
    fn rebuild(&mut self, items: &[(u32, Point, Point)]) {
        self.cells.clear();
        let max_half = items
            .iter()
            .fold(0.0f32, |m, &(_, _, h)| m.max(h.x).max(h.y));
        self.cell_size = (2.0 * max_half).max(32.0);
        for &(i, p, h) in items {
            let (x0, y0) = self.cell_of(p.x - h.x, p.y - h.y);
            let (x1, y1) = self.cell_of(p.x + h.x, p.y + h.y);
            for x in x0..=x1 {
                for y in y0..=y1 {
                    self.cells.entry((x, y)).or_default().push(i);
                }
            }
        }
    }

    fn cell_of(&self, x: f32, y: f32) -> (i32, i32) {
        (
            (x / self.cell_size).floor() as i32,
            (y / self.cell_size).floor() as i32,
        )
    }

    /// Pairs of boxes sharing a cell, sorted, each once.
    fn candidate_pairs(&self) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for items in self.cells.values() {
            for (i, &a) in items.iter().enumerate() {
                for &b in &items[i + 1..] {
                    out.push((a.min(b), a.max(b)));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
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

    fn merge(child: u32, parent: u32) -> LayoutEdge {
        LayoutEdge {
            first_parent: false,
            ..edge(child, parent)
        }
    }

    fn chain_net() -> (Layout, Net) {
        net_for(&LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 5],
            times: vec![5, 4, 3, 2, 1],
            edges: (0..4).map(|i| edge(i, i + 1)).collect(),
            priority: Vec::new(),
        })
    }

    /// 0 -> 1 -> 2 -> 3, and a long edge 0 -> 3 (edge 3).
    fn chain_with_long_edge() -> LayoutInput {
        LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 4],
            times: vec![4, 3, 2, 1],
            edges: vec![edge(0, 1), edge(1, 2), edge(2, 3), edge(0, 3)],
            priority: Vec::new(),
        }
    }

    /// 0 -> 1 and 2 -> 3, side by side.
    fn two_chains() -> LayoutInput {
        LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 4],
            times: vec![4, 2, 3, 1],
            edges: vec![edge(0, 1), edge(2, 3)],
            priority: Vec::new(),
        }
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
        net.grab(nodes[0], nodes, &[], params.model);
        for f in 1..=30 {
            let t = f as f32 / 30.0;
            net.drag_to(start + by * t);
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
        assert!(net.moved_by_hand[2] && !net.moved_by_hand[1]);
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
        // Far above the newest node, and far below the oldest.
        for dy in [-400.0, 400.0] {
            let (_, mut net) = chain_net();
            drag(&mut net, &[2], Point::new(30.0, dy), &params);
            settle(&mut net, &params);
            for child in 0..4 {
                let box_height = 20.0;
                let gap = net.node_pos(child + 1).y - net.node_pos(child).y - box_height;
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
        // Node 2 above its child 1, left so without the graph adapting.
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
        // Node 2 above its child 1 (edge 1), well to the side.
        let up = l.nodes[0].y - l.nodes[2].y - 40.0;
        drag(&mut net, &[2], Point::new(200.0, up), &free());
        settle(&mut net, &free());
        assert!(net.is_reversed(1) && net.is_rerouted(1));
        let pts: Vec<Point> = net.edge_points(1).collect();
        let (child, parent) = (net.node_pos(1), net.node_pos(2));
        let n = pts.len();
        let half_height = 10.0;
        assert!(close(
            pts[1],
            Point::new(child.x, child.y + half_height + route::TURN_LENGTH)
        ));
        assert!(close(
            pts[n - 2],
            Point::new(parent.x, parent.y - half_height - route::TURN_LENGTH)
        ));
    }

    #[test]
    fn moved_nodes_keep_giving_way() {
        let (_, mut net) = chain_net();
        let params = NetParams::default();
        drag(&mut net, &[2], Point::new(200.0, 0.0), &params);
        settle(&mut net, &params);
        let dropped = net.node_pos(2);
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
        let input = chain_with_long_edge();
        let (l, mut net) = net_for(&input);
        let long = 3;
        assert_eq!(l.edges[long].len(), 4, "the long edge has two bend points");
        let by = Point::new(300.0, 0.0);
        drag(&mut net, &[0], by, &free());
        settle(&mut net, &free());
        assert!(close(net.node_pos(0), l.nodes[0] + by));
        for i in 1..4 {
            assert_eq!(net.node_pos(i), l.nodes[i], "node {i} stays");
        }
        // The edge now runs along a route of its own, but its bend points still follow, for
        // when its nodes come back into line.
        let pts: Vec<Point> = net.chains[long]
            .iter()
            .map(|&p| net.pos(p as usize))
            .collect();
        let shift = |k: usize| pts[k].x - l.edges[long][k].x;
        assert!(shift(1) > shift(2) && shift(2) > 0.0 && shift(1) < 300.0);
        assert!(net.moved_by_hand[0] && net.is_rerouted(long));
    }

    #[test]
    fn free_moves_keep_their_offsets_when_adapting_again() {
        let (l, mut net) = chain_net();
        drag(&mut net, &[0], Point::new(300.0, 0.0), &free());
        settle(&mut net, &free());
        let params = NetParams::default();
        for _ in 0..60 {
            net.step(1.0 / 60.0, &params);
        }
        assert!(close(
            net.node_pos(0),
            Point::new(l.nodes[0].x + 300.0, l.nodes[0].y)
        ));
        drag(&mut net, &[1], Point::new(0.0, 60.0), &params);
        settle(&mut net, &params);
        let p = net.node_pos(0);
        assert!(p.y > l.nodes[0].y + 10.0, "follows: {p:?}");
        assert!(
            (p.x - l.nodes[0].x - 300.0).abs() < 5.0,
            "keeps its offset: {p:?}"
        );
    }

    fn edge_is_clear(net: &Net, e: usize) -> bool {
        let pts: Vec<Point> = net.edge_points(e).collect();
        let (child, parent) = net.ends(e);
        (0..net.node_count)
            .filter(|&i| i != child && i != parent)
            .all(|i| {
                let (c, h) = (net.node_pos(i), net.half_size[i]);
                pts.windows(2)
                    .all(|w| route::box_entry(w[0], w[1], c - h, c + h).is_none())
            })
    }

    #[test]
    fn squeezed_edges_lose_their_bends() {
        let input = chain_with_long_edge();
        let (l, mut net) = net_for(&input);
        let long = 3;
        assert!(
            (0..4).all(|e| !net.is_rerouted(e)),
            "the layout's routes at first"
        );
        assert_eq!(net.edge_points(long).count(), 4);
        let beside_0 = Point::new(l.nodes[0].x + 150.0, l.nodes[0].y) - l.nodes[3];
        drag(&mut net, &[3], beside_0, &free());
        settle(&mut net, &free());
        assert!(net.is_rerouted(long));
        assert_eq!(net.edge_points(long).count(), 2, "straight across");
        for e in 0..4 {
            assert!(edge_is_clear(&net, e), "edge {e} runs through a node");
        }
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
        let rerouted: Vec<bool> = (0..4).map(|e| net.is_rerouted(e)).collect();
        assert_eq!(rerouted, [false, true, true, false]);
    }

    #[test]
    fn stretched_edges_go_around_nodes_in_between() {
        let (l, mut net) = net_for(&two_chains());
        let (left, right) = if l.nodes[0].x < l.nodes[2].x {
            (0, 2)
        } else {
            (2, 0)
        };
        // The right chain's tip far to the left, past the left chain: its edge would cut
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
        let (l, mut net) = net_for(&two_chains());
        assert!(!net.is_rerouted(0));
        let middle = Point::new(
            (l.nodes[0].x + l.nodes[1].x) / 2.0,
            (l.nodes[0].y + l.nodes[1].y) / 2.0,
        );
        drag(&mut net, &[3], middle - l.nodes[3], &free());
        settle(&mut net, &free());
        assert!(net.is_rerouted(0), "0 -> 1 is covered by 3");
        assert!(edge_is_clear(&net, 0));
    }

    #[test]
    fn edges_left_behind_by_a_node_take_the_layout_route_again() {
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
        drag(&mut net, &[2], l.nodes[1] - l.nodes[2], &free());
        settle(&mut net, &free());
        assert!(net.is_rerouted(0), "0 -> 1 is covered by 2");
        // On the way back 2 leaves the edge's surroundings between two frames.
        net.reset();
        settle(&mut net, &free());
        assert!(close(net.node_pos(2), l.nodes[2]));
        assert!(!net.is_rerouted(0), "0 -> 1 takes the layout's route again");
    }

    #[test]
    fn edges_rerouted_away_from_their_layout_route_take_it_again() {
        let input = LayoutInput {
            sizes: vec![Point::new(60.0, 20.0); 5],
            times: vec![5, 4, 3, 2, 1],
            edges: vec![edge(0, 1), edge(1, 2), edge(2, 3), merge(0, 3)],
            priority: Vec::new(),
        };
        let (l, mut net) = net_for(&input);
        let bend_beside_2 = l.edges[3][2];
        assert!((bend_beside_2.y - l.nodes[2].y).abs() < 1.0 && bend_beside_2.x > l.nodes[2].x);
        drag(&mut net, &[4], bend_beside_2 - l.nodes[4], &free());
        settle(&mut net, &free());
        assert!(net.is_rerouted(3), "0 -> 3 is covered by 4");
        // On the way back 4 pushes the route of 0 -> 3 round the other side of the column,
        // away from its layout route, which 4 still covers.
        net.reset();
        settle(&mut net, &free());
        assert!(close(net.node_pos(4), l.nodes[4]));
        assert!(!net.is_rerouted(3), "0 -> 3 takes the layout's route again");
    }

    /// 0 -> 3 -> 4, and merges 1, 2 and 5 of 4 and 3 above 3, whose edges into 4 are bundled
    /// into one trunk: [`BUNDLED`].
    fn bundle_net() -> (Layout, Net) {
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

    const BUNDLED: [usize; 3] = [2, 3, 4];

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
        let children: Vec<Point> = edges.iter().map(|&e| net.node_pos(net.ends(e).0)).collect();
        last.iter().all(|s| *s == last[0]) && !children.contains(&last[0][0])
    }

    #[test]
    fn rerouted_edges_join_their_trunk() {
        let (_, mut net) = bundle_net();
        assert!(bundled(&net, &BUNDLED), "a trunk to begin with");
        for params in [free(), NetParams::default()] {
            drag(&mut net, &[1], Point::new(6.0, 4.0), &params);
            settle(&mut net, &params);
            assert!(net.is_rerouted(2));
            assert!(bundled(&net, &BUNDLED), "{params:?}");
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
        // Level with the trunk's only bend point.
        let beside_3 = Point::new(l.nodes[3].x + 150.0, l.nodes[3].y) - l.nodes[5];
        drag(&mut net, &[5], beside_3, &free());
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
        net.grab(1, &[1], &[0, 1], DragModel::Free);
        net.drag_to(l.nodes[1] + by);
        net.step(1.0 / 60.0, &free());
        net.release(&free());
        settle(&mut net, &free());
        for i in [0, 1] {
            assert!(close(net.node_pos(i), l.nodes[i] + by), "node {i} moves");
        }
        assert!(
            net.moved_by_hand[1] && !net.moved_by_hand[0],
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
            assert!(close(net.node_pos(i), l.nodes[i] + by));
        }
        for i in [0, 2, 4] {
            assert_eq!(net.node_pos(i), l.nodes[i]);
        }
    }

    #[test]
    fn nodes_push_each_other_away() {
        let input = two_chains();
        let (l, _) = net_for(&input);
        assert_eq!(l.nodes[0].y, l.nodes[2].y, "tips share a row");
        let (left, right) = if l.nodes[0].x < l.nodes[2].x {
            (0, 2)
        } else {
            (2, 0)
        };
        // The left tip to within 10 of the right one.
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
        assert!(!net.moved_by_hand[2] && !net.any_displaced());
        assert!(net.redo());
        settle(&mut net, &params);
        for (i, &p) in shape.iter().enumerate() {
            assert!(close(net.node_pos(i), p), "node {i} redone");
        }
        assert!(net.moved_by_hand[2]);
        net.reset();
        settle(&mut net, &params);
        assert!(close(net.node_pos(2), l.nodes[2]));
        assert!(net.undo(), "a reset is undoable");
        settle(&mut net, &params);
        assert!(close(net.node_pos(2), shape[2]));
    }

    #[test]
    fn undo_with_nothing_to_undo_keeps_the_drag() {
        let (l, mut net) = chain_net();
        net.grab(2, &[2], &[], DragModel::Adapt);
        assert!(!net.undo() && !net.redo());
        assert_eq!(net.grab.as_ref().map(|g| g.anchor), Some(2));
        net.drag_to(l.nodes[2] + Point::new(50.0, 0.0));
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
            net.grab(node, &[node], &[], DragModel::Adapt);
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
        // Rows closer than the magnet range.
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
        net.grab(1, &[1], &[], DragModel::Adapt);
        for _ in 0..100 {
            net.step(1.0 / 60.0, &NetParams::default());
        }
        assert!(
            net.active.len() <= GRAB_WAKE_BUDGET + 1,
            "{} particles awake",
            net.active.len()
        );
    }

    #[test]
    fn return_one_node_to_the_layout() {
        let (l, mut net) = chain_net();
        drag(&mut net, &[1, 2], Point::new(100.0, 0.0), &free());
        settle(&mut net, &free());
        net.return_to_layout(&[2]);
        settle(&mut net, &free());
        assert!(close(net.node_pos(2), l.nodes[2]) && !net.moved_by_hand[2]);
        assert!(close(
            net.node_pos(1),
            Point::new(l.nodes[1].x + 100.0, l.nodes[1].y)
        ));
        assert!(net.moved_by_hand[1]);
    }

    #[test]
    fn rest_places_round_trip() {
        let (l, mut net) = chain_net();
        drag(&mut net, &[3], Point::new(40.0, 0.0), &NetParams::default());
        settle(&mut net, &NetParams::default());
        let saved: Vec<RestPlace> = net.rest_places().collect();
        assert!(saved.iter().any(|s| s.node == 3 && s.by_hand));
        let (_, mut fresh) = chain_net();
        fresh.restore(saved, &NetParams::default());
        assert!(!fresh.is_awake(), "restores at once");
        for i in 0..5 {
            assert!(close(fresh.node_pos(i), net.node_pos(i)), "node {i}");
        }
        assert!(fresh.moved_by_hand[3] && !fresh.moved_by_hand[2]);
        assert!(
            fresh.node_pos(2).x > l.nodes[2].x,
            "neighbours as they were"
        );
    }

    /// Saves `net`'s rest places and restores them into a net for `input`, which has the same
    /// nodes as `net`, at the same indices, and new ones after them.
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
        let from_1 = |i: usize| fresh.node_pos(i) - fresh.node_pos(1);
        for i in [4, 5] {
            assert!(
                close(from_1(i), l.nodes[i] - l.nodes[1]),
                "node {i} keeps its place beside 1: {:?}",
                from_1(i)
            );
            assert!(!fresh.moved_by_hand[i]);
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
        new.edges.extend([edge(4, 0), merge(4, 3)]);
        let (_, mut net) = net_for(&old);
        let up = net.flow * -400.0;
        drag(&mut net, &[3], up, &free());
        settle(&mut net, &free());

        let (l, fresh) = reload(&net, &new);
        let flow = fresh.flow;
        let above = |child: usize, parent: usize| {
            (fresh.node_pos(parent) - fresh.node_pos(child)).dot(flow)
        };
        assert!(above(4, 3) >= 20.0 + FLOW_GAP - 0.5, "{}", above(4, 3));
        assert!(above(4, 0) > 0.0);
        let across = fresh.node_pos(4) - l.nodes[4];
        assert!(
            across.dot(Point::new(flow.y, flow.x)).abs() < 0.5,
            "only pushed up"
        );
        assert!(close(fresh.node_pos(0), l.nodes[0]), "old nodes stay");
    }

    #[test]
    fn a_node_put_back_onto_another_pushes_it_aside() {
        let (l, mut net) = net_for(&two_chains());
        // Node 0 was moved by hand to where a changed layout now puts node 2.
        let onto = l.nodes[2] - l.nodes[0];
        let place = RestPlace {
            node: 0,
            offset: onto,
            by_hand: true,
            beside: None,
        };
        net.restore([place], &NetParams::default());
        assert!(!net.is_awake(), "settles at once");
        assert!(close(net.node_pos(0), l.nodes[2]), "the moved node stays");
        assert!(!close(net.node_pos(2), l.nodes[2]), "the other gives way");
        assert_eq!(net.tidiness().overlaps, 0);
        let flow = net.flow;
        assert!(
            (net.node_pos(3) - net.node_pos(2)).dot(flow) > 0.0,
            "and stays above its parent"
        );
        assert!(net.is_displaced(2) && !net.moved_by_hand[2]);
    }

    #[test]
    fn a_node_goes_back_beside_its_first_parent() {
        let apart = Point::new(0.0, -200.0);
        let offset = Point::new(5.0, 0.0);
        let place = |beside| RestPlace {
            node: 0,
            offset,
            by_hand: true,
            beside: Some(beside),
        };
        let (_, mut net) = net_for(&two_chains());
        net.restore([place((1, apart))], &NetParams::default());
        assert!(close(net.node_pos(0) - net.node_pos(1), apart));
        // Beside a node that is no longer its first parent: at its offset instead.
        let (l, mut net) = net_for(&two_chains());
        net.restore([place((3, apart))], &NetParams::default());
        assert!(close(net.node_pos(0), l.nodes[0] + offset));
        let (l, mut net) = net_for(&two_chains());
        let rested_in_layout = RestPlace {
            offset: Point::default(),
            by_hand: false,
            ..place((1, apart))
        };
        net.restore([rested_in_layout], &NetParams::default());
        assert!(close(net.node_pos(0), l.nodes[0]));
    }

    #[test]
    fn a_child_goes_back_beside_its_parent_after_the_parent() {
        // 1 and 0 both rest away, and the child is listed first.
        let (_, mut net) = chain_net();
        let (to_1, to_0) = (Point::new(200.0, -30.0), Point::new(-120.0, -60.0));
        let place = |node: usize, beside: usize, apart: Point| RestPlace {
            node,
            offset: Point::new(1.0, 0.0),
            by_hand: true,
            beside: Some((beside, apart)),
        };
        net.restore(
            [place(0, 1, to_0), place(1, 2, to_1)],
            &NetParams::default(),
        );
        assert!(close(net.node_pos(1) - net.node_pos(2), to_1));
        assert!(close(net.node_pos(0) - net.node_pos(1), to_0));
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
        new.edges.extend([edge(3, 1), merge(3, 0)]);
        let (_, mut net) = net_for(&old);
        drag(&mut net, &[1], Point::new(300.0, -40.0), &free());
        settle(&mut net, &free());
        let beside = net.node_pos(1) - net.node_pos(2);

        let (_, fresh) = reload(&net, &new);
        assert!(
            close(fresh.node_pos(1) - fresh.node_pos(2), beside),
            "the tip keeps its place beside its parent"
        );
        assert!(fresh.moved_by_hand[1] && !fresh.moved_by_hand[3]);
        let flow = fresh.flow;
        assert!((fresh.node_pos(1) - fresh.node_pos(3)).dot(flow) > 0.0);
        let tidiness = fresh.tidiness();
        assert_eq!(tidiness.overlaps, 0);
        assert_eq!(tidiness.edges_through_boxes, 0);
        assert_eq!(tidiness.edges_detouring, 0);
    }

    #[test]
    fn only_the_neighbourhood_of_a_grab_is_simulated() {
        let n = GRAB_WAKE_BUDGET as u32 * 2;
        let input = LayoutInput {
            sizes: vec![Point::new(20.0, 10.0); n as usize],
            times: (0..n as i64).rev().collect(),
            edges: (0..n - 1).map(|i| edge(i, i + 1)).collect(),
            priority: Vec::new(),
        };
        let (l, mut net) = net_for(&input);
        net.grab(0, &[0], &[], DragModel::Adapt);
        assert!(net.active.len() <= GRAB_WAKE_BUDGET + 1);
        net.drag_to(Point::new(l.nodes[0].x + 100.0, l.nodes[0].y));
        net.step(1.0 / 60.0, &NetParams::default());
        assert_eq!(
            net.node_pos(n as usize - 1),
            l.nodes[n as usize - 1],
            "far end untouched"
        );
        net.release(&NetParams::default());
        settle(&mut net, &NetParams::default());
        assert_eq!(net.active.len(), 0, "everything goes back to sleep");
    }

    #[test]
    fn pushing_wakes_resting_nodes_in_big_graphs() {
        // A row of more tips than are simulated at once, under one root.
        let tips = GRAB_WAKE_BUDGET as u32 + 2_000;
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
        net.grab(first, &[first], &[], DragModel::Adapt);
        let asleep = (0..tips as usize).filter(|&i| !net.is_active[i]).count();
        assert!(asleep > 0, "part of the row is not simulated");
        // Through the whole row in big jumps.
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
            // The leftmost tip far past the others.
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
        let model = |name: &str| serde_json::from_value::<DragModel>(name.into()).unwrap();
        assert_eq!(
            model("Net"),
            DragModel::Adapt,
            "the spider web became Adapt"
        );
        assert_eq!(model("Rigid"), DragModel::Free);
    }
}
