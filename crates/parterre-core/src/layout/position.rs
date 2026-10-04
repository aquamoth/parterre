//! Coordinate assignment along layers.
//!
//! Minimises `Σ w·|u(a) − u(b)|` over all edge segments, subject to each layer keeping its
//! order with minimum separations, i.e. the objective of OGDF's `OptimalHierarchyLayout`. We
//! solve it by block coordinate descent: each layer in turn is placed optimally given its
//! neighbours, which is a weighted isotonic regression solved exactly by pool-adjacent-
//! violators. The L1 objective is approached by iteratively reweighted least squares, which
//! makes edges exactly straight wherever the constraints allow.

use super::LayeredGraph;

/// Sweeps for normal graphs; huge ones (hundreds of thousands of items) get fewer.
const SWEEPS: usize = 40;
const MIN_SWEEPS: usize = 12;
/// Below this distance (layout units) an edge counts as straight for reweighting.
const STRAIGHT: f32 = 1.0;
/// Pull of an item towards its previous position, relative to its edge weights.
const INERTIA: f32 = 0.02;
/// PROTOTYPE (centred trunk): weight pinning trunk items to `u = 0`.
const PIN: f32 = 1.0e6;

/// Returns the along-layer centre coordinate of every item.
pub fn assign(g: &LayeredGraph, node_gap: f32) -> Vec<f32> {
    let mut u = vec![0.0f32; g.items.len()];
    // Start packed to the left.
    for layer in &g.layers {
        let mut x = 0.0;
        for (k, &i) in layer.iter().enumerate() {
            if k > 0 {
                x += separation(g, layer[k - 1], i, node_gap);
            }
            u[i as usize] = x;
        }
    }

    let mut targets = Vec::new();
    let mut weights = Vec::new();
    let mut seps = Vec::new();
    let mut solver = Isotonic::default();
    let sweeps = if g.items.len() > 200_000 {
        MIN_SWEEPS
    } else {
        SWEEPS
    };
    for sweep in 0..sweeps {
        // L2 for the first sweeps to settle, then reweight towards L1.
        let l1 = sweep >= sweeps / 4;
        let order: Box<dyn Iterator<Item = usize>> = if sweep % 2 == 0 {
            Box::new(0..g.layers.len())
        } else {
            Box::new((0..g.layers.len()).rev())
        };
        for l in order {
            let layer = &g.layers[l];
            let pinned = |i: u32| g.side.get(i as usize) == Some(&0);
            if let [single] = layer[..] {
                if pinned(single) {
                    u[single as usize] = 0.0;
                    continue;
                }
                // Nothing to separate: move straight to the (weighted) target.
                let x = u[single as usize];
                let (mut sum_w, mut sum_wx) = (0.0, 0.0);
                for &(nb, w) in g.up(single as usize).iter().chain(g.down(single as usize)) {
                    let nx = u[nb as usize];
                    let w = if l1 {
                        w / (x - nx).abs().max(STRAIGHT)
                    } else {
                        w
                    };
                    sum_w += w;
                    sum_wx += w * nx;
                }
                if sum_w > 0.0 {
                    u[single as usize] = sum_wx / sum_w;
                }
                continue;
            }
            targets.clear();
            weights.clear();
            seps.clear();
            for (k, &i) in layer.iter().enumerate() {
                let x = u[i as usize];
                let mut sum_w = 0.0;
                let mut sum_wx = 0.0;
                for &(nb, w) in g.up(i as usize).iter().chain(g.down(i as usize)) {
                    let nx = u[nb as usize];
                    let w = if l1 {
                        w / (x - nx).abs().max(STRAIGHT)
                    } else {
                        w
                    };
                    sum_w += w;
                    sum_wx += w * nx;
                }
                let inertia = INERTIA * sum_w.max(1.0);
                if pinned(i) {
                    targets.push(0.0);
                    weights.push(PIN);
                } else {
                    targets.push((sum_wx + inertia * x) / (sum_w + inertia));
                    weights.push(sum_w + inertia);
                }
                if k > 0 {
                    seps.push(separation(g, layer[k - 1], i, node_gap));
                }
            }
            let placed = solver.solve(&targets, &weights, &seps);
            for (k, &i) in layer.iter().enumerate() {
                u[i as usize] = placed[k];
            }
        }
    }
    u
}

/// Minimum centre-to-centre distance between adjacent items `a` (left) and `b`.
fn separation(g: &LayeredGraph, a: u32, b: u32, node_gap: f32) -> f32 {
    let (ia, ib) = (&g.items[a as usize], &g.items[b as usize]);
    let gap = match (ia.dummy, ib.dummy) {
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
    /// Blocks of (sum w·y, sum w, item count).
    blocks: Vec<(f64, f64, usize)>,
    x: Vec<f32>,
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
        for i in 0..n {
            let w = weights[i].max(1e-6) as f64;
            let y = targets[i] as f64 - self.offset[i];
            self.blocks.push((w * y, w, 1));
            while self.blocks.len() >= 2 {
                let (b, a) = (
                    self.blocks[self.blocks.len() - 1],
                    self.blocks[self.blocks.len() - 2],
                );
                if a.0 / a.1 <= b.0 / b.1 {
                    break;
                }
                self.blocks.pop();
                let last = self.blocks.last_mut().unwrap();
                *last = (a.0 + b.0, a.1 + b.1, a.2 + b.2);
            }
        }
        self.x.clear();
        for &(swy, sw, count) in &self.blocks {
            let y = swy / sw;
            for _ in 0..count {
                let i = self.x.len();
                self.x.push((y + self.offset[i]) as f32);
            }
        }
        &self.x
    }
}

#[cfg(test)]
fn isotonic(targets: &[f32], weights: &[f32], seps: &[f32]) -> Vec<f32> {
    Isotonic::default().solve(targets, weights, seps).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Heavier item stays closer to its target.
        let x = isotonic(&[0.0, 0.0], &[3.0, 1.0], &[8.0]);
        assert_eq!(x, vec![-2.0, 6.0]);
    }
}
