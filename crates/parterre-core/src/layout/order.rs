//! Crossing minimisation. Starts from a depth-first order that keeps first-parent lines
//! together, then applies layer-by-layer median sweeps (Eades & Wormald; Gansner et al.) with
//! transposition, keeping the best ordering seen by an exact crossing count (Barth, Jünger &
//! Mutzel). As OGDF's `SugiyamaLayout` does, several runs are made, all but the first from a
//! randomly perturbed copy of the depth-first order, and the best is kept.

use super::LayoutInput;
use super::layered::LayeredGraph;
use super::trunk::Side;

/// Per run, each a sweep down and one up.
const MAX_SWEEPS: usize = 24;
const MAX_SWEEPS_WITHOUT_IMPROVEMENT: usize = 4;
const MAX_TRANSPOSE_PASSES: usize = 8;

#[derive(Clone, Copy)]
enum Neighbours {
    Above,
    Below,
}

impl Neighbours {
    fn of(self, g: &LayeredGraph, item: usize) -> &[(u32, f32)] {
        match self {
            Neighbours::Above => g.up(item),
            Neighbours::Below => g.down(item),
        }
    }
}

/// Returns the crossings left.
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
    let mut best = Vec::new();
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
            reorder_layer(g, l, Neighbours::Above, sweep, &mut scratch);
        }
        for l in (0..g.layers.len() - 1).rev() {
            reorder_layer(g, l, Neighbours::Below, sweep, &mut scratch);
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
            if stale >= MAX_SWEEPS_WITHOUT_IMPROVEMENT {
                break;
            }
        }
    }
    g.layers = best;
    g.update_positions();
    best_crossings
}

/// Swaps random neighbouring items in every layer, keeping most of the depth-first structure.
fn perturb(g: &mut LayeredGraph, rng: &mut XorShift) {
    for layer in &mut g.layers {
        let n = layer.len();
        if n < 2 {
            continue;
        }
        for _ in 0..n.div_ceil(3) {
            let i = (rng.next() % (n as u64 - 1)) as usize;
            if same_side(&g.side, layer[i], layer[i + 1]) {
                layer.swap(i, i + 1);
            }
        }
    }
    g.update_positions();
}

/// Items never swap across the trunk.
fn same_side(side: &[Side], a: u32, b: u32) -> bool {
    side.is_empty() || side[a as usize] == side[b as usize]
}

/// Gansner et al.'s transpose heuristic: swaps adjacent items wherever that reduces crossings
/// with both neighbouring layers, until no swap helps.
fn transpose_all(g: &mut LayeredGraph) {
    let mut around_a = Vec::new();
    let mut around_b = Vec::new();
    for _ in 0..MAX_TRANSPOSE_PASSES {
        let mut improved = false;
        for l in 0..g.layers.len() {
            for i in 0..g.layers[l].len().saturating_sub(1) {
                let (a, b) = (g.layers[l][i], g.layers[l][i + 1]);
                if !same_side(&g.side, a, b) {
                    continue;
                }
                let (a, b) = (a as usize, b as usize);
                let mut crossings_kept = 0;
                let mut crossings_swapped = 0;
                for neighbours in [Neighbours::Above, Neighbours::Below] {
                    neighbour_positions(g, a, neighbours, &mut around_a);
                    neighbour_positions(g, b, neighbours, &mut around_b);
                    crossings_kept += pair_crossings(&around_a, &around_b);
                    crossings_swapped += pair_crossings(&around_b, &around_a);
                }
                if crossings_swapped < crossings_kept {
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

fn neighbour_positions(g: &LayeredGraph, item: usize, neighbours: Neighbours, out: &mut Vec<u32>) {
    out.clear();
    out.extend(
        neighbours
            .of(g, item)
            .iter()
            .map(|&(nb, _)| g.pos[nb as usize]),
    );
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

/// Deterministic: layouts must be reproducible.
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
    let mut seq = vec![u32::MAX; g.items.len()];
    let mut next = 0u32;

    let mut starts: Vec<u32> = input.priority.clone();
    let mut by_age: Vec<u32> = (0..g.node_count as u32).collect();
    by_age.sort_by_key(|&v| {
        (
            g.items[v as usize].layer,
            std::cmp::Reverse(input.time(v as usize)),
        )
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
        // Left | trunk | right, the left side mirrored so that the first-visited branches sit
        // next to the trunk on both sides.
        let side = &g.side;
        for layer in &mut g.layers {
            layer.sort_by_key(|&i| {
                let s = side[i as usize];
                let k = seq[i as usize] as i64;
                (s, if s == Side::Left { -k } else { k })
            });
        }
    }
    g.update_positions();
}

#[derive(Default)]
struct Scratch {
    values: Vec<f32>,
    keys: Vec<Option<f32>>,
    movable: Vec<(f32, usize, u32)>,
}

/// Orders layer `l` by the median position of each item's `neighbours`. Items without any
/// keep their slot.
fn reorder_layer(
    g: &mut LayeredGraph,
    l: usize,
    neighbours: Neighbours,
    sweep: usize,
    s: &mut Scratch,
) {
    if g.layers[l].len() < 2 {
        return;
    }
    s.keys.clear();
    for &i in &g.layers[l] {
        s.values.clear();
        s.values.extend(
            neighbours
                .of(g, i as usize)
                .iter()
                .map(|&(nb, _)| g.pos[nb as usize] as f32),
        );
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
    // Items never move across the trunk.
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

fn total_crossings(g: &LayeredGraph) -> u64 {
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

/// Crossings among edges between two layers, given as (upper pos, lower pos): inversions of
/// the lower positions, counted with a Fenwick tree.
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
        assert_eq!(bilayer_crossings(&mut [(0, 1), (1, 0)], 2), 1);
        // Shared endpoints do not cross.
        assert_eq!(bilayer_crossings(&mut [(0, 0), (0, 1), (1, 1)], 2), 0);
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
