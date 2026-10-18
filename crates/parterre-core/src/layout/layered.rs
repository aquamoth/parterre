use std::collections::HashMap;

use super::trunk::{Side, Sides};
use super::{LayoutInput, LayoutOptions, layer_count};

const MAX_DUMMIES: u64 = 3_000_000;

#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub layer: u32,
    pub breadth: f32,
    pub is_dummy: bool,
}

/// A list per item, stored flat: item `i`'s is `list[start[i]..start[i + 1]]`. All-commits
/// views of big repositories have millions of dummies; a pair of `Vec`s per item cost 48 bytes
/// plus two heap blocks each, which the allocator mostly kept after the layout.
#[derive(Clone, Debug)]
struct Flat<T> {
    start: Vec<u32>,
    list: Vec<T>,
}

impl<T> Flat<T> {
    fn get(&self, item: usize) -> &[T] {
        &self.list[self.start[item] as usize..self.start[item + 1] as usize]
    }
}

type Adjacency = Flat<(u32, f32)>;

#[derive(Clone, Debug)]
pub struct LayeredGraph {
    /// Items `0..node_count` are the nodes, in input order; dummies follow.
    pub items: Vec<Item>,
    pub node_count: usize,
    pub layers: Vec<Vec<u32>>,
    /// Every item's index in its layer: the inverse of `layers`.
    pub pos: Vec<u32>,
    /// For every input edge, its dummies from child to parent.
    pub chains: Vec<Vec<u32>>,
    /// Every item's side of the trunk; empty without a trunk.
    pub side: Vec<Side>,
    up: Adjacency,
    down: Adjacency,
}

#[derive(Clone, Copy, Debug)]
struct Segment {
    upper: u32,
    lower: u32,
    weight: f32,
}

/// Coordinate assignment keeps long edges (dummy to dummy) straight most eagerly, then
/// first-parent lines, then everything else.
fn segment_weight(first_parent: bool, from_dummy: bool, to_dummy: bool) -> f32 {
    let base = if first_parent { 2.0 } else { 1.0 };
    match (from_dummy, to_dummy) {
        (true, true) => base * 4.0,
        (true, false) | (false, true) => base * 2.0,
        (false, false) => base,
    }
}

impl LayeredGraph {
    /// With [`LayoutOptions::concentrate_edges`], edges into the same parent share their dummy
    /// in every layer they pass through, one per side of the trunk (graphviz's edge
    /// concentration).
    pub fn build(
        input: &LayoutInput,
        layers: &[u32],
        breadth: &[f32],
        options: &LayoutOptions,
        sides: Option<&Sides>,
    ) -> Self {
        let n = input.sizes.len();
        let mut items: Vec<Item> = (0..n)
            .map(|i| Item {
                layer: layers[i],
                breadth: breadth[i],
                is_dummy: false,
            })
            .collect();
        let mut chains = Vec::with_capacity(input.edges.len());
        let mut segments: Vec<Segment> = Vec::new();
        let spans: Vec<u32> = input
            .edges
            .iter()
            .map(|e| layers[e.parent as usize].saturating_sub(layers[e.child as usize]))
            .collect();
        let max_span = longest_routed_span(&spans);
        let mut shared: HashMap<(u32, u32, Side), u32> = HashMap::new();
        let mut side = sides.map_or_else(Vec::new, |s| s.nodes.clone());

        for (k, e) in input.edges.iter().enumerate() {
            if spans[k] > max_span {
                // No bend points, and no pull on the coordinates either.
                chains.push(Vec::new());
                continue;
            }
            let edge_side = sides.map_or(Side::Trunk, |s| s.edges[k]);
            let (c, p) = (e.child as usize, e.parent as usize);
            let mut chain = Vec::new();
            let mut prev = e.child;
            for layer in layers[c] + 1..layers[p] {
                let key = (e.parent, layer, edge_side);
                let d = match shared.get(&key) {
                    Some(&d) => d,
                    None => {
                        let d = items.len() as u32;
                        items.push(Item {
                            layer,
                            breadth: options.edge_gap,
                            is_dummy: true,
                        });
                        if sides.is_some() {
                            side.push(edge_side);
                        }
                        if options.concentrate_edges {
                            shared.insert(key, d);
                        }
                        d
                    }
                };
                segments.push(segment(&items, prev, d, e.first_parent));
                chain.push(d);
                prev = d;
            }
            if spans[k] > 0 {
                segments.push(segment(&items, prev, e.parent, e.first_parent));
            }
            chains.push(chain);
        }

        let mut by_layer: Vec<Vec<u32>> = vec![Vec::new(); layer_count(layers)];
        for (i, item) in items.iter().enumerate() {
            by_layer[item.layer as usize].push(i as u32);
        }
        merge_repeats(&mut segments, items.len());
        let down = compress(items.len(), &segments, |s| (s.upper, s.lower));
        let up = compress(items.len(), &segments, |s| (s.lower, s.upper));
        drop(segments);
        let mut g = LayeredGraph {
            pos: vec![0; items.len()],
            items,
            node_count: n,
            layers: by_layer,
            chains,
            side,
            up,
            down,
        };
        g.update_positions();
        g
    }

    /// Neighbours in the newer layer, with their segments' weights.
    pub fn up(&self, item: usize) -> &[(u32, f32)] {
        self.up.get(item)
    }

    /// Neighbours in the older layer, with their segments' weights.
    pub fn down(&self, item: usize) -> &[(u32, f32)] {
        self.down.get(item)
    }

    pub fn update_positions(&mut self) {
        for layer in &self.layers {
            for (i, &item) in layer.iter().enumerate() {
                self.pos[item as usize] = i as u32;
            }
        }
    }
}

/// A safety valve for pathological inputs: if routing every edge through every layer took more
/// than [`MAX_DUMMIES`] bend points, the longest edges get none and are drawn as direct lines.
fn longest_routed_span(spans: &[u32]) -> u32 {
    let total: u64 = spans.iter().map(|&s| s.saturating_sub(1) as u64).sum();
    if total <= MAX_DUMMIES {
        return u32::MAX;
    }
    let mut sorted = spans.to_vec();
    sorted.sort_unstable();
    let mut budget = MAX_DUMMIES;
    let mut limit = 1;
    for s in sorted {
        let cost = s.saturating_sub(1) as u64;
        if cost > budget {
            break;
        }
        budget -= cost;
        limit = s;
    }
    limit
}

fn segment(items: &[Item], upper: u32, lower: u32, first_parent: bool) -> Segment {
    Segment {
        upper,
        lower,
        weight: segment_weight(
            first_parent,
            items[upper as usize].is_dummy,
            items[lower as usize].is_dummy,
        ),
    }
}

/// Concentrated edges share segments, and parallel edges their only one: keeps the first of
/// every repeated (upper, lower) pair, with the heaviest weight, and drops the rest.
fn merge_repeats(segments: &mut Vec<Segment>, item_count: usize) {
    let by_upper = compress_with(item_count, segments.len(), |emit| {
        for (k, s) in segments.iter().enumerate() {
            emit(s.upper, k as u32);
        }
    });
    let mut keep = vec![true; segments.len()];
    for item in 0..item_count {
        let group = by_upper.get(item);
        for (j, &k) in group.iter().enumerate() {
            let lower = segments[k as usize].lower;
            if let Some(&first) = group[..j]
                .iter()
                .find(|&&f| keep[f as usize] && segments[f as usize].lower == lower)
            {
                let weight = segments[k as usize].weight;
                let kept = &mut segments[first as usize];
                kept.weight = kept.weight.max(weight);
                keep[k as usize] = false;
            }
        }
    }
    let mut keep = keep.into_iter();
    segments.retain(|_| keep.next() == Some(true));
}

/// `ends` gives the (item, neighbour) of a segment. Every list keeps the segments' order.
fn compress(
    item_count: usize,
    segments: &[Segment],
    ends: impl Fn(&Segment) -> (u32, u32),
) -> Adjacency {
    compress_with(item_count, segments.len(), |emit| {
        for s in segments {
            let (item, neighbour) = ends(s);
            emit(item, (neighbour, s.weight));
        }
    })
}

/// A stable counting sort by item of the entries that `each` emits, which it does twice: once
/// to count them, once to place them.
fn compress_with<T: Copy + Default>(
    item_count: usize,
    len: usize,
    each: impl Fn(&mut dyn FnMut(u32, T)),
) -> Flat<T> {
    let mut start = vec![0u32; item_count + 1];
    each(&mut |item, _| start[item as usize + 1] += 1);
    for i in 0..item_count {
        start[i + 1] += start[i];
    }
    let mut next = start.clone();
    let mut list = vec![T::default(); len];
    each(&mut |item, value| {
        let slot = &mut next[item as usize];
        list[*slot as usize] = value;
        *slot += 1;
    });
    Flat { start, list }
}
