//! Which side of the trunk every node and edge goes.
//!
//! The trunk is the first-parent line of the first priority node (the default branch). Every
//! other node belongs to a *side branch*: a connected group of nodes off the trunk, with the
//! edges that join it to the trunk. A whole side branch goes on one side, so it never crosses
//! the trunk. Ordering then keeps `left | trunk | right` in every layer, and coordinate
//! assignment pins the trunk to one straight line.

use super::{LayoutInput, Trunk};

pub const LEFT: i8 = -1;
pub const TRUNK: i8 = 0;
pub const RIGHT: i8 = 1;

/// Side of every node and every edge: [`LEFT`], [`TRUNK`] or [`RIGHT`].
#[derive(Clone, Debug, PartialEq)]
pub struct Sides {
    pub nodes: Vec<i8>,
    pub edges: Vec<i8>,
}

/// Room taken in the layers, for balancing the sides.
pub struct Room<'a> {
    pub layers: &'a [u32],
    pub breadth: &'a [f32],
    pub node_gap: f32,
    pub edge_gap: f32,
    /// Edges into the same parent share their bend points.
    pub concentrate: bool,
}

/// Sides for `variant`, or `None` for [`Trunk::Leftmost`] or without a priority node.
pub fn sides(input: &LayoutInput, variant: Trunk, room: &Room) -> Option<Sides> {
    if variant == Trunk::Leftmost {
        return None;
    }
    let &anchor = input.priority.first()?;
    let n = input.sizes.len();
    let mut nodes = vec![RIGHT; n];
    let mut edges = vec![RIGHT; input.edges.len()];

    // The trunk: first parents from the anchor down. Where the graph dropped a merge's first
    // parent as redundant (it is an ancestor of another parent), the line goes on through the
    // edge that is left.
    let mut next = vec![usize::MAX; n];
    for (k, e) in input.edges.iter().enumerate() {
        let slot = &mut next[e.child as usize];
        if *slot == usize::MAX || (e.first_parent && !input.edges[*slot].first_parent) {
            *slot = k;
        }
    }
    let mut node = anchor as usize;
    loop {
        nodes[node] = TRUNK;
        let k = next[node];
        if k == usize::MAX {
            break;
        }
        edges[k] = TRUNK;
        node = input.edges[k].parent as usize;
        if nodes[node] == TRUNK {
            break;
        }
    }

    // Side branches: nodes off the trunk joined by edges off the trunk. An edge to the trunk
    // goes with its other end; one between two trunk nodes, but not along the trunk, is a side
    // branch of its own.
    let mut uf = UnionFind::new(n + input.edges.len());
    for (k, e) in input.edges.iter().enumerate() {
        if edges[k] == TRUNK {
            continue;
        }
        let (c, p) = (e.child as usize, e.parent as usize);
        match (nodes[c] == TRUNK, nodes[p] == TRUNK) {
            (false, false) => {
                uf.union(c, p);
                uf.union(n + k, c);
            }
            (false, true) => uf.union(n + k, c),
            (true, false) => uf.union(n + k, p),
            (true, true) => {}
        }
    }
    // Every side branch's nodes and edges, by its root.
    let mut group_of = vec![u32::MAX; n + input.edges.len()];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let members = (0..n).filter(|&i| nodes[i] != TRUNK).chain(
        (0..input.edges.len())
            .filter(|&k| edges[k] != TRUNK)
            .map(|k| n + k),
    );
    for m in members {
        let root = uf.find(m);
        if group_of[root] == u32::MAX {
            group_of[root] = groups.len() as u32;
            groups.push(Vec::new());
        }
        groups[group_of[root] as usize].push(m);
    }

    let go_left = match variant {
        Trunk::Leftmost => unreachable!(),
        Trunk::Centred => balance(input, room, &groups, n),
        Trunk::Alternating => alternate(input, room, &groups, n),
    };
    for (members, left) in groups.iter().zip(go_left) {
        if left {
            for &m in members {
                if m < n {
                    nodes[m] = LEFT;
                } else {
                    edges[m - n] = LEFT;
                }
            }
        }
    }
    Some(Sides { nodes, edges })
}

/// Biggest side branches first, each on the side where its busiest layer stays narrowest;
/// ties go right.
fn balance(input: &LayoutInput, room: &Room, groups: &[Vec<usize>], n: usize) -> Vec<bool> {
    let layer_count = room
        .layers
        .iter()
        .map(|&l| l as usize + 1)
        .max()
        .unwrap_or(0);
    let mut load = [vec![0.0f32; layer_count], vec![0.0f32; layer_count]];
    let footprints: Vec<Vec<(u32, f32)>> = groups
        .iter()
        .map(|members| footprint(input, room, members, n))
        .collect();
    let mut order: Vec<usize> = (0..groups.len()).collect();
    let total = |g: usize| footprints[g].iter().map(|&(_, w)| w).sum::<f32>();
    let totals: Vec<f32> = order.iter().map(|&g| total(g)).collect();
    order.sort_by(|&a, &b| totals[b].total_cmp(&totals[a]).then(a.cmp(&b)));
    let mut left = vec![false; groups.len()];
    for g in order {
        let peak = |side: usize| {
            footprints[g]
                .iter()
                .map(|&(l, w)| load[side][l as usize] + w)
                .fold(0.0f32, f32::max)
        };
        let side = if peak(0) < peak(1) { 0 } else { 1 };
        for &(l, w) in &footprints[g] {
            load[side][l as usize] += w;
        }
        left[g] = side == 0;
    }
    left
}

/// Side branches take turns, right first, from the newest fork down.
fn alternate(input: &LayoutInput, room: &Room, groups: &[Vec<usize>], n: usize) -> Vec<bool> {
    // A side branch forks off where its oldest end is.
    let fork = |members: &Vec<usize>| {
        members
            .iter()
            .map(|&m| {
                if m < n {
                    room.layers[m]
                } else {
                    room.layers[input.edges[m - n].parent as usize]
                }
            })
            .max()
            .unwrap_or(0)
    };
    let mut order: Vec<(u32, usize)> = groups.iter().map(fork).zip(0..).collect();
    order.sort_unstable();
    let mut left = vec![false; groups.len()];
    for (turn, &(_, g)) in order.iter().enumerate() {
        left[g] = turn % 2 == 1;
    }
    left
}

/// Width a side branch takes in each layer it reaches, merged by layer: its nodes and the bend
/// points of its edges.
fn footprint(input: &LayoutInput, room: &Room, members: &[usize], n: usize) -> Vec<(u32, f32)> {
    let mut out = Vec::new();
    let mut bundles = std::collections::HashSet::new();
    for &m in members {
        if m < n {
            out.push((room.layers[m], room.breadth[m] + room.node_gap));
            continue;
        }
        let e = input.edges[m - n];
        let (from, to) = (
            room.layers[e.child as usize],
            room.layers[e.parent as usize],
        );
        for l in from.saturating_add(1)..to {
            if !room.concentrate || bundles.insert((e.parent, l)) {
                out.push((l, room.edge_gap));
            }
        }
    }
    out.sort_unstable_by_key(|&(l, _)| l);
    out.dedup_by(|b, a| {
        let same = a.0 == b.0;
        if same {
            a.1 += b.1;
        }
        same
    });
    out
}

struct UnionFind(Vec<u32>);

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind((0..n as u32).collect())
    }

    fn find(&mut self, mut i: usize) -> usize {
        while self.0[i] as usize != i {
            let up = self.0[self.0[i] as usize];
            self.0[i] = up;
            i = up as usize;
        }
        i
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        // The smaller index becomes the root, so groups come out in a stable order.
        if a < b {
            self.0[b] = a as u32;
        } else if b < a {
            self.0[a] = b as u32;
        }
    }
}
