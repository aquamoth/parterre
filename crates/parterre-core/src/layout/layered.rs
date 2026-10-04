//! A proper layered graph: every edge spans exactly one layer, long edges being split into
//! chains of dummy items.

use super::LayoutInput;

/// Upper bound on dummy items; see [`LayeredGraph::build`].
const MAX_DUMMIES: u64 = 3_000_000;

/// A node or an edge dummy placed in one layer. Its neighbours are
/// [`LayeredGraph::up`] and [`LayeredGraph::down`].
#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub layer: u32,
    /// Extent along the layer.
    pub breadth: f32,
    /// True for a bend point of a long edge rather than a real node.
    pub dummy: bool,
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

/// Neighbours of every item in one direction, with edge weights.
type Adjacency = Flat<(u32, f32)>;

#[derive(Clone, Debug)]
pub struct LayeredGraph {
    /// Items `0..node_count` are the real nodes, in input order; dummies follow.
    pub items: Vec<Item>,
    pub node_count: usize,
    /// Items of each layer in left-to-right order.
    pub layers: Vec<Vec<u32>>,
    /// Position of every item within its layer (inverse of `layers`).
    pub pos: Vec<u32>,
    /// For every input edge, the dummy items it passes through, from child to parent.
    pub chains: Vec<Vec<u32>>,
    /// PROTOTYPE (centred trunk): side of the trunk of every item, -1, 0 (trunk) or 1; empty
    /// for no trunk.
    pub side: Vec<i8>,
    /// Neighbours in the layer above (newer) and below (older).
    up: Adjacency,
    down: Adjacency,
}

/// One segment of an edge, between items in adjacent layers, with its weight.
#[derive(Clone, Copy, Debug)]
struct Segment {
    upper: u32,
    lower: u32,
    weight: f32,
}

/// Weight of an edge segment in coordinate assignment: long edges (dummy-to-dummy) are kept
/// straight most eagerly, then first-parent lines, then everything else.
fn segment_weight(first_parent: bool, from_dummy: bool, to_dummy: bool) -> f32 {
    let base = if first_parent { 2.0 } else { 1.0 };
    match (from_dummy, to_dummy) {
        (true, true) => base * 4.0,
        (true, false) | (false, true) => base * 2.0,
        (false, false) => base,
    }
}

impl LayeredGraph {
    /// Builds the layered graph. With `concentrate`, edges into the same parent share their
    /// dummy items wherever they pass through the same layer, so parallel edges merge into one
    /// trunk (graphviz's "edge concentration").
    pub fn build(
        input: &LayoutInput,
        layers: &[u32],
        breadth: &[f32],
        edge_gap: f32,
        concentrate: bool,
    ) -> Self {
        let n = input.sizes.len();
        let mut items: Vec<Item> = (0..n)
            .map(|i| Item {
                layer: layers[i],
                breadth: breadth[i],
                dummy: false,
            })
            .collect();
        let mut chains = Vec::with_capacity(input.edges.len());
        let mut segments: Vec<Segment> = Vec::new();
        // Safety valve for pathological inputs: if routing every edge through every layer would
        // need more than MAX_DUMMIES bend points, the longest edges get none and are drawn as
        // direct lines.
        let spans: Vec<u32> = input
            .edges
            .iter()
            .map(|e| layers[e.parent as usize].saturating_sub(layers[e.child as usize]))
            .collect();
        let max_span = {
            let total: u64 = spans.iter().map(|&s| s.saturating_sub(1) as u64).sum();
            if total <= MAX_DUMMIES {
                u32::MAX
            } else {
                let mut sorted = spans.clone();
                sorted.sort_unstable();
                let mut budget = MAX_DUMMIES;
                let mut limit = 1;
                for &s in &sorted {
                    let cost = s.saturating_sub(1) as u64;
                    if cost > budget {
                        break;
                    }
                    budget -= cost;
                    limit = s;
                }
                limit
            }
        };
        // (parent, layer) -> shared dummy, when concentrating.
        let mut shared: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();

        for e in &input.edges {
            let (c, p) = (e.child as usize, e.parent as usize);
            let mut chain = Vec::new();
            let mut prev = c as u32;
            if layers[p].saturating_sub(layers[c]) > max_span {
                // Too long to route: no bend points and no pull on the coordinates.
                chains.push(chain);
                continue;
            }
            for layer in layers[c] + 1..layers[p] {
                let existing = if concentrate {
                    shared.get(&(p as u32, layer)).copied()
                } else {
                    None
                };
                let d = match existing {
                    Some(d) => d,
                    None => {
                        let d = items.len() as u32;
                        items.push(Item {
                            layer,
                            breadth: edge_gap,
                            dummy: true,
                        });
                        if concentrate {
                            shared.insert((p as u32, layer), d);
                        }
                        d
                    }
                };
                segments.push(segment(&items, prev, d, e.first_parent));
                chain.push(d);
                prev = d;
            }
            if layers[p] > layers[c] {
                segments.push(segment(&items, prev, p as u32, e.first_parent));
            }
            chains.push(chain);
        }

        let layer_count = layers.iter().map(|&l| l as usize + 1).max().unwrap_or(0);
        let mut by_layer: Vec<Vec<u32>> = vec![Vec::new(); layer_count];
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
            side: Vec::new(),
            up,
            down,
        };
        g.update_positions();
        g
    }

    /// Neighbours of `item` in the layer above (newer), with edge weights.
    pub fn up(&self, item: usize) -> &[(u32, f32)] {
        self.up.get(item)
    }

    /// Neighbours of `item` in the layer below (older), with edge weights.
    pub fn down(&self, item: usize) -> &[(u32, f32)] {
        self.down.get(item)
    }

    /// Recomputes `pos` from `layers`.
    pub fn update_positions(&mut self) {
        for layer in &self.layers {
            for (i, &item) in layer.iter().enumerate() {
                self.pos[item as usize] = i as u32;
            }
        }
    }
}

fn segment(items: &[Item], upper: u32, lower: u32, first_parent: bool) -> Segment {
    Segment {
        upper,
        lower,
        weight: segment_weight(
            first_parent,
            items[upper as usize].dummy,
            items[lower as usize].dummy,
        ),
    }
}

/// Concentrated edges share segments, and parallel edges their only one: keeps the first of
/// every repeated (upper, lower) pair, with the heaviest weight, and drops the rest.
fn merge_repeats(segments: &mut Vec<Segment>, item_count: usize) {
    // Segment indices grouped by upper item, in order.
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
                let w = segments[k as usize].weight;
                let kept = &mut segments[first as usize];
                if w > kept.weight {
                    kept.weight = w;
                }
                keep[k as usize] = false;
            }
        }
    }
    let mut keep = keep.into_iter();
    segments.retain(|_| keep.next() == Some(true));
}

/// Neighbour lists from segments: `ends` gives (item, neighbour) of each. Every list keeps
/// the segments' order.
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

/// Stable counting sort of the entries that `each` emits (twice: once to count them, once to
/// place them) by item.
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
