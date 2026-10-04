//! Crossing minimisation: ordering items within their layers.
//!
//! Starts from a depth-first order that keeps first-parent lines together, then applies
//! layer-by-layer median sweeps (Eades & Wormald; Gansner et al.) with transposition,
//! keeping the best ordering seen, as measured by an exact crossing count (Barth, Jünger &
//! Mutzel). Several runs from perturbed starting orders are made, as OGDF does.

use super::{LayeredGraph, LayoutInput};

/// Maximum number of down+up sweep pairs per run.
const MAX_SWEEPS: usize = 24;
/// Stop a run after this many sweeps without improvement.
const PATIENCE: usize = 4;
/// Transpose passes per sweep at most.
const MAX_TRANSPOSE_PASSES: usize = 8;

/// Orders every layer to reduce crossings and returns the number of crossings left.
///
/// Like OGDF's `SugiyamaLayout` (15 runs by default), several runs are made from different
/// starting orders and the best result is kept: the first run starts from the depth-first
/// order, later ones from randomly perturbed copies of it. Large graphs get fewer runs.
pub fn minimize_crossings(g: &mut LayeredGraph, input: &LayoutInput) -> u64 {
    initial_order(g, input);
    if g.layers.len() < 2 {
        return 0;
    }
    let items = g.items.len();
    let runs = match items {
        0..=2_000 => 12,
        2_001..=10_000 => 4,
        10_001..=40_000 => 2,
        _ => 1,
    };
    let transpose = items <= 40_000;

    let start = g.layers.clone();
    let mut best = g.layers.clone();
    let mut best_crossings = u64::MAX;
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    for run in 0..runs {
        if run > 0 {
            g.layers = start.clone();
            perturb(g, &mut rng);
        }
        let crossings = sweep_run(g, transpose);
        if crossings < best_crossings {
            best_crossings = crossings;
            best = g.layers.clone();
        }
        if best_crossings == 0 {
            break;
        }
    }
    g.layers = best;
    g.update_positions();
    best_crossings
}

/// One run of alternating median sweeps (with transposition), keeping the best ordering.
fn sweep_run(g: &mut LayeredGraph, transpose: bool) -> u64 {
    g.update_positions();
    let mut best = g.layers.clone();
    let mut best_crossings = total_crossings(g);
    let mut stale = 0;
    let mut scratch = Scratch::default();
    for sweep in 0..MAX_SWEEPS {
        if best_crossings == 0 {
            break;
        }
        for l in 1..g.layers.len() {
            reorder_layer(g, l, true, sweep, &mut scratch);
        }
        for l in (0..g.layers.len() - 1).rev() {
            reorder_layer(g, l, false, sweep, &mut scratch);
        }
        if transpose {
            transpose_all(g);
        }
        let crossings = total_crossings(g);
        if crossings < best_crossings {
            best_crossings = crossings;
            best = g.layers.clone();
            stale = 0;
        } else {
            stale += 1;
            if stale >= PATIENCE {
                break;
            }
        }
    }
    g.layers = best;
    g.update_positions();
    best_crossings
}

/// Randomly swaps some neighbouring items in every layer, so that a run explores a different
/// part of the search space while keeping most of the depth-first structure.
fn perturb(g: &mut LayeredGraph, rng: &mut XorShift) {
    for layer in &mut g.layers {
        let n = layer.len();
        if n < 2 {
            continue;
        }
        for _ in 0..n.div_ceil(3) {
            let i = (rng.next() % (n as u64 - 1)) as usize;
            if !g.side.is_empty() && g.side[layer[i] as usize] != g.side[layer[i + 1] as usize] {
                continue;
            }
            layer.swap(i, i + 1);
        }
    }
    g.update_positions();
}

/// Swaps adjacent items wherever that reduces crossings with both neighbouring layers, until
/// no swap helps (the "transpose" heuristic of Gansner et al.).
fn transpose_all(g: &mut LayeredGraph) {
    let mut up_a = Vec::new();
    let mut up_b = Vec::new();
    for _ in 0..MAX_TRANSPOSE_PASSES {
        let mut improved = false;
        for l in 0..g.layers.len() {
            for i in 0..g.layers[l].len().saturating_sub(1) {
                let (a, b) = (g.layers[l][i] as usize, g.layers[l][i + 1] as usize);
                if !g.side.is_empty() && g.side[a] != g.side[b] {
                    continue;
                }
                let mut keep = 0;
                let mut swap = 0;
                for up in [true, false] {
                    neighbour_positions(g, a, up, &mut up_a);
                    neighbour_positions(g, b, up, &mut up_b);
                    keep += pair_crossings(&up_a, &up_b);
                    swap += pair_crossings(&up_b, &up_a);
                }
                if swap < keep {
                    g.layers[l].swap(i, i + 1);
                    g.pos[a] = (i + 1) as u32;
                    g.pos[b] = i as u32;
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
}

fn neighbour_positions(g: &LayeredGraph, item: usize, up: bool, out: &mut Vec<u32>) {
    let list = if up { g.up(item) } else { g.down(item) };
    out.clear();
    out.extend(list.iter().map(|&(nb, _)| g.pos[nb as usize]));
    out.sort_unstable();
}

/// Crossings between the edges of a left item (neighbour positions `left`, sorted) and a right
/// item (`right`, sorted): pairs where the left item's neighbour lies right of the other's.
fn pair_crossings(left: &[u32], right: &[u32]) -> u64 {
    let mut count = 0;
    let mut j = 0;
    for &a in left {
        while j < right.len() && right[j] < a {
            j += 1;
        }
        count += j as u64;
    }
    count
}

/// Small deterministic PRNG (layouts must be reproducible).
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// Depth-first from the tips (priority nodes first), descending into first parents before
/// other parents, so that each first-parent line forms a contiguous left-to-right band.
fn initial_order(g: &mut LayeredGraph, input: &LayoutInput) {
    let n_items = g.items.len();
    let mut seq = vec![u32::MAX; n_items];
    let mut next = 0u32;

    let mut starts: Vec<u32> = input.priority.clone();
    let mut by_age: Vec<u32> = (0..g.node_count as u32).collect();
    by_age.sort_by_key(|&v| {
        let time = input.times.get(v as usize).copied().unwrap_or(0);
        (g.items[v as usize].layer, std::cmp::Reverse(time))
    });
    starts.extend(by_age);

    let mut stack: Vec<(u32, usize)> = Vec::new();
    for start in starts {
        if seq[start as usize] != u32::MAX {
            continue;
        }
        seq[start as usize] = next;
        next += 1;
        stack.push((start, 0));
        while let Some(top) = stack.last_mut() {
            let (item, i) = (top.0 as usize, top.1);
            if let Some(&(child, _)) = g.down(item).get(i) {
                top.1 += 1;
                if seq[child as usize] == u32::MAX {
                    seq[child as usize] = next;
                    next += 1;
                    stack.push((child, 0));
                }
            } else {
                stack.pop();
            }
        }
    }
    if g.side.is_empty() {
        for layer in &mut g.layers {
            layer.sort_by_key(|&i| seq[i as usize]);
        }
    } else {
        // PROTOTYPE (centred trunk): left | trunk | right, the left side mirrored so that the
        // first-visited branches sit next to the trunk on both sides.
        let side = &g.side;
        for layer in &mut g.layers {
            layer.sort_by_key(|&i| {
                let s = side[i as usize];
                let k = seq[i as usize] as i64;
                (s, if s < 0 { -k } else { k })
            });
        }
    }
    g.update_positions();
}

/// Reusable buffers for [`reorder_layer`].
#[derive(Default)]
struct Scratch {
    values: Vec<f32>,
    keys: Vec<Option<f32>>,
    movable: Vec<(f32, usize, u32)>,
}

/// Reorders layer `l` by the median position of each item's neighbours in the adjacent layer
/// (above when sweeping down). Items without such neighbours keep their slot.
fn reorder_layer(
    g: &mut LayeredGraph,
    l: usize,
    sweeping_down: bool,
    sweep: usize,
    s: &mut Scratch,
) {
    if g.layers[l].len() < 2 {
        return;
    }
    s.keys.clear();
    for &i in &g.layers[l] {
        let neighbours = if sweeping_down {
            g.up(i as usize)
        } else {
            g.down(i as usize)
        };
        s.values.clear();
        s.values
            .extend(neighbours.iter().map(|&(nb, _)| g.pos[nb as usize] as f32));
        s.keys.push(median(&mut s.values));
    }
    s.movable.clear();
    for (slot, (&item, key)) in g.layers[l].iter().zip(&s.keys).enumerate() {
        if let Some(k) = key {
            s.movable.push((*k, slot, item));
        }
    }
    // Alternate the tie-breaking direction between sweeps so equal medians can swap.
    if sweep.is_multiple_of(2) {
        s.movable
            .sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    } else {
        s.movable
            .sort_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)));
    }
    let mut movable = s.movable.iter();
    let layer = &mut g.layers[l];
    for (slot, key) in s.keys.iter().enumerate() {
        if key.is_some() {
            layer[slot] = movable.next().expect("one movable item per keyed slot").2;
        }
    }
    if !g.side.is_empty() {
        let side = &g.side;
        layer.sort_by_key(|&i| side[i as usize]);
    }
    for (i, &item) in layer.iter().enumerate() {
        g.pos[item as usize] = i as u32;
    }
}

/// Median with the Gansner et al. interpolation for even counts.
fn median(values: &mut [f32]) -> Option<f32> {
    let n = values.len();
    if n == 0 {
        return None;
    }
    values.sort_by(f32::total_cmp);
    let m = n / 2;
    Some(match n {
        _ if n % 2 == 1 => values[m],
        2 => (values[0] + values[1]) / 2.0,
        _ => {
            let left = values[m - 1] - values[0];
            let right = values[n - 1] - values[m];
            if left + right == 0.0 {
                (values[m - 1] + values[m]) / 2.0
            } else {
                (values[m - 1] * right + values[m] * left) / (left + right)
            }
        }
    })
}

/// Total number of edge crossings between all adjacent layer pairs.
pub fn total_crossings(g: &LayeredGraph) -> u64 {
    let mut edges: Vec<(u32, u32)> = Vec::new();
    let mut total = 0;
    for l in 0..g.layers.len().saturating_sub(1) {
        edges.clear();
        for &i in &g.layers[l] {
            for &(below, _) in g.down(i as usize) {
                edges.push((g.pos[i as usize], g.pos[below as usize]));
            }
        }
        total += bilayer_crossings(&mut edges, g.layers[l + 1].len());
    }
    total
}

/// Counts crossings among edges between two layers, given as (upper pos, lower pos), by
/// counting inversions of the lower positions with a Fenwick tree.
fn bilayer_crossings(edges: &mut [(u32, u32)], lower_len: usize) -> u64 {
    edges.sort_unstable();
    let mut tree = vec![0u64; lower_len + 1];
    let mut crossings = 0;
    for (seen, &(_, lower)) in edges.iter().enumerate() {
        // Edges seen so far with a lower endpoint strictly right of this one cross it.
        let mut not_greater = 0;
        let mut i = lower as usize + 1;
        while i > 0 {
            not_greater += tree[i];
            i &= i - 1;
        }
        crossings += seen as u64 - not_greater;
        let mut i = lower as usize + 1;
        while i <= lower_len {
            tree[i] += 1;
            i += i & i.wrapping_neg();
        }
    }
    crossings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_bilayer_crossings() {
        // Upper 0 -> lower 1, upper 1 -> lower 0: one crossing.
        assert_eq!(bilayer_crossings(&mut [(0, 1), (1, 0)], 2), 1);
        // Shared endpoints do not cross.
        assert_eq!(bilayer_crossings(&mut [(0, 0), (0, 1), (1, 1)], 2), 0);
        // Complete reversal of three edges: three crossings.
        assert_eq!(bilayer_crossings(&mut [(0, 2), (1, 1), (2, 0)], 3), 3);
    }

    #[test]
    fn pair_crossings_counts_inversions() {
        assert_eq!(pair_crossings(&[0], &[1]), 0);
        assert_eq!(pair_crossings(&[1], &[0]), 1);
        assert_eq!(pair_crossings(&[2, 3], &[0, 1, 2]), 5);
    }

    #[test]
    fn median_interpolates() {
        assert_eq!(median(&mut []), None);
        assert_eq!(median(&mut [3.0]), Some(3.0));
        assert_eq!(median(&mut [4.0, 2.0]), Some(3.0));
        assert_eq!(median(&mut [1.0, 5.0, 3.0]), Some(3.0));
    }
}
