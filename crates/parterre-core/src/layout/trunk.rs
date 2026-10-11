//! The trunk is the first-parent line of the first priority node (the default branch). Every
//! other node belongs to a side branch: a connected group of nodes off the trunk, with the
//! edges that join it to the trunk. A whole side branch goes on one side, so it never crosses
//! the trunk. Ordering then keeps `left | trunk | right` in every layer, and coordinate
//! assignment pins the trunk to one straight line.

use std::collections::HashSet;

use super::{LayoutInput, LayoutOptions, Trunk, layer_count};

/// Ordered as the items of a layer are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    Left,
    Trunk,
    Right,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sides {
    pub nodes: Vec<Side>,
    pub edges: Vec<Side>,
}

/// `None` for [`Trunk::Leftmost`] or without a priority node.
pub fn sides(
    input: &LayoutInput,
    layers: &[u32],
    breadth: &[f32],
    options: &LayoutOptions,
) -> Option<Sides> {
    let place = match options.trunk {
        Trunk::Leftmost => return None,
        Trunk::Centred => Layering::balance,
        Trunk::Alternating => Layering::alternate,
    };
    let &anchor = input.priority.first()?;
    let n = input.sizes.len();
    let mut nodes = vec![Side::Right; n];
    let mut edges = vec![Side::Right; input.edges.len()];

    // The trunk: first parents from the anchor down. Where the graph dropped a merge's first
    // parent as redundant (it is an ancestor of another parent), the line goes on through the
    // edge that is left.
    let mut next: Vec<Option<usize>> = vec![None; n];
    for (k, e) in input.edges.iter().enumerate() {
        let slot = &mut next[e.child as usize];
        if slot.is_none_or(|s| e.first_parent && !input.edges[s].first_parent) {
            *slot = Some(k);
        }
    }
    let mut node = anchor as usize;
    loop {
        nodes[node] = Side::Trunk;
        let Some(k) = next[node] else {
            break;
        };
        edges[k] = Side::Trunk;
        node = input.edges[k].parent as usize;
        if nodes[node] == Side::Trunk {
            break;
        }
    }

    // Side branches: nodes off the trunk joined by edges off the trunk. An edge to the trunk
    // goes with its other end; one between two trunk nodes, but not along the trunk, is a side
    // branch of its own. Members `0..n` are nodes, `n + k` is edge `k`.
    let mut uf = UnionFind::new(n + input.edges.len());
    for (k, e) in input.edges.iter().enumerate() {
        if edges[k] == Side::Trunk {
            continue;
        }
        let (c, p) = (e.child as usize, e.parent as usize);
        match (nodes[c] == Side::Trunk, nodes[p] == Side::Trunk) {
            (false, false) => {
                uf.union(c, p);
                uf.union(n + k, c);
            }
            (false, true) => uf.union(n + k, c),
            (true, false) => uf.union(n + k, p),
            (true, true) => {}
        }
    }
    let mut group_of = vec![u32::MAX; n + input.edges.len()];
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let members = (0..n).filter(|&i| nodes[i] != Side::Trunk).chain(
        (0..input.edges.len())
            .filter(|&k| edges[k] != Side::Trunk)
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

    let layering = Layering {
        input,
        layers,
        breadth,
        options,
    };
    for (members, left) in groups.iter().zip(place(&layering, &groups)) {
        if !left {
            continue;
        }
        for &m in members {
            if m < n {
                nodes[m] = Side::Left;
            } else {
                edges[m - n] = Side::Left;
            }
        }
    }
    Some(Sides { nodes, edges })
}

struct Layering<'a> {
    input: &'a LayoutInput,
    layers: &'a [u32],
    breadth: &'a [f32],
    options: &'a LayoutOptions,
}

impl Layering<'_> {
    /// Biggest side branches first, each on the side where its busiest layer stays narrowest;
    /// ties go right.
    fn balance(&self, groups: &[Vec<usize>]) -> Vec<bool> {
        let layer_count = layer_count(self.layers);
        let mut left_load = vec![0.0f32; layer_count];
        let mut right_load = vec![0.0f32; layer_count];
        let footprints: Vec<Vec<(u32, f32)>> = groups
            .iter()
            .map(|members| self.footprint(members))
            .collect();
        let totals: Vec<f32> = footprints
            .iter()
            .map(|f| f.iter().map(|&(_, w)| w).sum::<f32>())
            .collect();
        let mut order: Vec<usize> = (0..groups.len()).collect();
        order.sort_by(|&a, &b| totals[b].total_cmp(&totals[a]).then(a.cmp(&b)));
        let mut left = vec![false; groups.len()];
        for g in order {
            let peak = |load: &[f32]| {
                footprints[g]
                    .iter()
                    .map(|&(l, w)| load[l as usize] + w)
                    .fold(0.0f32, f32::max)
            };
            left[g] = peak(&left_load) < peak(&right_load);
            let load = if left[g] {
                &mut left_load
            } else {
                &mut right_load
            };
            for &(l, w) in &footprints[g] {
                load[l as usize] += w;
            }
        }
        left
    }

    /// Side branches take turns, right first, from the newest fork down.
    fn alternate(&self, groups: &[Vec<usize>]) -> Vec<bool> {
        let n = self.input.sizes.len();
        // A side branch forks off where its oldest end is.
        let fork = |members: &Vec<usize>| {
            members
                .iter()
                .map(|&m| {
                    if m < n {
                        self.layers[m]
                    } else {
                        self.layers[self.input.edges[m - n].parent as usize]
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

    /// The width a side branch takes in each layer it reaches, by layer: its nodes and the
    /// bend points of its edges.
    fn footprint(&self, members: &[usize]) -> Vec<(u32, f32)> {
        let n = self.input.sizes.len();
        let mut out = Vec::new();
        let mut bundles = HashSet::new();
        for &m in members {
            if m < n {
                out.push((self.layers[m], self.breadth[m] + self.options.node_gap));
                continue;
            }
            let e = self.input.edges[m - n];
            let (from, to) = (
                self.layers[e.child as usize],
                self.layers[e.parent as usize],
            );
            for l in from.saturating_add(1)..to {
                if !self.options.concentrate_edges || bundles.insert((e.parent, l)) {
                    out.push((l, self.options.edge_gap));
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
