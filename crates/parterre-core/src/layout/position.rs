//! Coordinate assignment along layers. Minimises `Σ w·|u(a) − u(b)|` over all edge segments,
//! subject to each layer keeping its order with minimum separations: the objective of OGDF's
//! `OptimalHierarchyLayout`. Solved by block coordinate descent: each layer in turn is placed
//! optimally given its neighbours, a weighted isotonic regression solved exactly by
//! pool-adjacent-violators. The L1 objective is approached by iteratively reweighted least
//! squares, which makes edges exactly straight wherever the constraints allow.

use super::LayeredGraph;
use super::trunk::Side;

const SWEEPS: usize = 40;
const HUGE_GRAPH_ITEMS: usize = 200_000;
const HUGE_GRAPH_SWEEPS: usize = 12;
/// Edges shorter than this count as straight when reweighting.
const STRAIGHT: f32 = 1.0;
/// The pull of an item towards its previous position, relative to its edge weights.
const INERTIA: f32 = 0.02;
const TRUNK_PIN_WEIGHT: f32 = 1.0e6;

/// The `u` of every item's centre.
pub fn assign(g: &LayeredGraph, node_gap: f32) -> Vec<f32> {
    let mut u = vec![0.0f32; g.items.len()];
    for layer in &g.layers {
        let mut x = 0.0;
        for (k, &i) in layer.iter().enumerate() {
            if k > 0 {
                x += separation(g, layer[k - 1], i, node_gap);
            }
            u[i as usize] = x;
        }
    }

    let pinned = |i: u32| g.side.get(i as usize) == Some(&Side::Trunk);
    let mut targets = Vec::new();
    let mut weights = Vec::new();
    let mut seps = Vec::new();
    let mut solver = Isotonic::default();
    let sweeps = if g.items.len() > HUGE_GRAPH_ITEMS {
        HUGE_GRAPH_SWEEPS
    } else {
        SWEEPS
    };
    for sweep in 0..sweeps {
        // L2 for the first sweeps to settle, then reweight towards L1.
        let l1 = sweep >= sweeps / 4;
        for k in 0..g.layers.len() {
            let l = if sweep.is_multiple_of(2) {
                k
            } else {
                g.layers.len() - 1 - k
            };
            let layer = &g.layers[l];
            if let [single] = layer[..] {
                if pinned(single) {
                    u[single as usize] = 0.0;
                    continue;
                }
                // Nothing to separate: straight to the weighted target.
                let (sum_w, sum_wx) = pull(g, &u, single as usize, l1);
                if sum_w > 0.0 {
                    u[single as usize] = sum_wx / sum_w;
                }
                continue;
            }
            targets.clear();
            weights.clear();
            seps.clear();
            for &i in layer {
                let x = u[i as usize];
                let (sum_w, sum_wx) = pull(g, &u, i as usize, l1);
                let inertia = INERTIA * sum_w.max(1.0);
                if pinned(i) {
                    targets.push(0.0);
                    weights.push(TRUNK_PIN_WEIGHT);
                } else {
                    targets.push((sum_wx + inertia * x) / (sum_w + inertia));
                    weights.push(sum_w + inertia);
                }
            }
            seps.extend(
                layer
                    .windows(2)
                    .map(|pair| separation(g, pair[0], pair[1], node_gap)),
            );
            let placed = solver.solve(&targets, &weights, &seps);
            for (&i, &x) in layer.iter().zip(placed) {
                u[i as usize] = x;
            }
        }
    }
    u
}

/// `Σw` and `Σw·u` over the neighbours of `item`, the weights reweighted towards L1 if `l1`.
fn pull(g: &LayeredGraph, u: &[f32], item: usize, l1: bool) -> (f32, f32) {
    let x = u[item];
    let (mut sum_w, mut sum_wx) = (0.0, 0.0);
    for &(nb, w) in g.up(item).iter().chain(g.down(item)) {
        let nx = u[nb as usize];
        let w = if l1 {
            w / (x - nx).abs().max(STRAIGHT)
        } else {
            w
        };
        sum_w += w;
        sum_wx += w * nx;
    }
    (sum_w, sum_wx)
}

/// The least centre-to-centre distance between adjacent items `a` (left) and `b`.
fn separation(g: &LayeredGraph, a: u32, b: u32, node_gap: f32) -> f32 {
    let (ia, ib) = (&g.items[a as usize], &g.items[b as usize]);
    let gap = match (ia.is_dummy, ib.is_dummy) {
        (true, true) => 0.0,
        (false, false) => node_gap,
        _ => node_gap * 0.5,
    };
    (ia.breadth + ib.breadth) / 2.0 + gap
}

/// Weighted least-squares placement `x` minimising `Σ w_i (x_i − t_i)²` subject to
/// `x_{i+1} − x_i ≥ sep_i`, by pool-adjacent-violators on `y_i = x_i − Σ_{j<i} sep_j`.
/// Keeps its buffers between calls.
#[derive(Default)]
struct Isotonic {
    offset: Vec<f64>,
    blocks: Vec<Block>,
    x: Vec<f32>,
}

#[derive(Clone, Copy)]
struct Block {
    sum_wy: f64,
    sum_w: f64,
    len: usize,
}

impl Block {
    fn mean(self) -> f64 {
        self.sum_wy / self.sum_w
    }
}

impl Isotonic {
    fn solve(&mut self, targets: &[f32], weights: &[f32], seps: &[f32]) -> &[f32] {
        let n = targets.len();
        self.offset.clear();
        let mut acc = 0.0f64;
        for i in 0..n {
            if i > 0 {
                acc += seps[i - 1] as f64;
            }
            self.offset.push(acc);
        }
        self.blocks.clear();
        for ((&t, &w), &offset) in targets.iter().zip(weights).zip(&self.offset) {
            let w = w.max(1e-6) as f64;
            let y = t as f64 - offset;
            self.blocks.push(Block {
                sum_wy: w * y,
                sum_w: w,
                len: 1,
            });
            while let [.., a, b] = self.blocks[..] {
                if a.mean() <= b.mean() {
                    break;
                }
                self.blocks.truncate(self.blocks.len() - 2);
                self.blocks.push(Block {
                    sum_wy: a.sum_wy + b.sum_wy,
                    sum_w: a.sum_w + b.sum_w,
                    len: a.len + b.len,
                });
            }
        }
        self.x.clear();
        for block in &self.blocks {
            let y = block.mean();
            for _ in 0..block.len {
                let i = self.x.len();
                self.x.push((y + self.offset[i]) as f32);
            }
        }
        &self.x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isotonic(targets: &[f32], weights: &[f32], seps: &[f32]) -> Vec<f32> {
        Isotonic::default().solve(targets, weights, seps).to_vec()
    }

    #[test]
    fn isotonic_keeps_feasible_targets() {
        let x = isotonic(&[0.0, 10.0, 30.0], &[1.0; 3], &[5.0, 5.0]);
        assert_eq!(x, vec![0.0, 10.0, 30.0]);
    }

    #[test]
    fn isotonic_pools_violators_around_weighted_mean() {
        // Both want 0 but must be 10 apart: centred on 0.
        let x = isotonic(&[0.0, 0.0], &[1.0, 1.0], &[10.0]);
        assert_eq!(x, vec![-5.0, 5.0]);
        // The heavier item stays closer to its target.
        let x = isotonic(&[0.0, 0.0], &[3.0, 1.0], &[8.0]);
        assert_eq!(x, vec![-2.0, 6.0]);
    }
}
