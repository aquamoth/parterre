//! Property tests: random graphs, random drags in every model, undo and redo. Positions and
//! routes stay finite, the net settles and then stays put, a reset returns every node to its
//! layout position and every edge to its layout route, and when the graph adapts to a drag,
//! children stay above their parents. (Adapted from a review's fuzzing.)

#![allow(clippy::needless_range_loop)] // index loops read better in these tests

use parterre_core::layout::{
    self, Direction, LayoutEdge, LayoutInput, LayoutOptions, Point, Ranking,
};
use parterre_core::physics::{DragModel, FLOW_GAP, Net, NetParams, RestPlace};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    fn f(&mut self) -> f32 {
        (self.next() % 10_000) as f32 / 10_000.0
    }
}

fn input(rng: &mut Rng, n: usize) -> LayoutInput {
    let mut edges = Vec::new();
    for c in 0..n {
        for k in 0..rng.below(3) {
            if c + 1 < n {
                let p = (c + 1 + rng.below(4) as usize).min(n - 1);
                if !edges
                    .iter()
                    .any(|e: &LayoutEdge| e.child == c as u32 && e.parent == p as u32)
                {
                    edges.push(LayoutEdge {
                        child: c as u32,
                        parent: p as u32,
                        first_parent: k == 0,
                    });
                }
            }
        }
    }
    LayoutInput {
        sizes: (0..n)
            .map(|_| Point::new(40.0 + rng.below(100) as f32, 22.0))
            .collect(),
        times: vec![],
        edges,
        priority: vec![],
    }
}

/// `PHYSICS_SEED` and `PHYSICS_ITERS` override the seed and the number of graphs, to hunt
/// for failures: `PHYSICS_SEED=31337 PHYSICS_ITERS=1000 cargo test --test properties_physics`.
fn env_or(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[test]
fn physics_random_drags() {
    let mut rng = Rng(env_or("PHYSICS_SEED", 4242));
    for iter in 0..env_or("PHYSICS_ITERS", 120) as usize {
        let n = 1 + rng.below(60) as usize;
        let inp = input(&mut rng, n);
        let opts = LayoutOptions {
            concentrate_edges: iter % 2 == 0,
            ranking: Ranking::ALL[iter % 3],
            direction: Direction::ALL[iter % 4],
            ..LayoutOptions::default()
        };
        let l = layout::layout(&inp, &opts);
        let mut net = Net::new(&l, &inp.sizes);
        let mut params = NetParams {
            model: DragModel::Adapt,
            pull: rng.f(),
            push: rng.f(),
            wobble: rng.f(),
            avoid_overlap: iter % 5 != 0,
        };
        for _ in 0..5 {
            params.model = DragModel::ALL[rng.below(3) as usize];
            let node = rng.below(n as u64) as usize;
            let mut nodes = vec![node];
            for _ in 0..rng.below(4) {
                nodes.push(rng.below(n as u64) as usize);
            }
            let carried: Vec<usize> = (0..rng.below(3))
                .map(|_| rng.below(n as u64) as usize)
                .collect();
            net.grab(node, &nodes, &carried, params.model.adapts());
            for s in 0..20 {
                let t = Point::new(
                    l.nodes[node].x + (s as f32) * 20.0 * (rng.f() - 0.5),
                    l.nodes[node].y + (s as f32) * 20.0 * (rng.f() - 0.5),
                );
                net.drag_to(t);
                let dt = [1.0 / 60.0, 0.0, 1.0, 1e-6, 1.0 / 240.0][rng.below(5) as usize];
                net.step(dt, &params);
            }
            net.release(&params);
            match rng.below(6) {
                0 => net.return_to_layout(&nodes),
                1 => {
                    net.undo();
                }
                2 => {
                    net.undo();
                    net.redo();
                }
                _ => {}
            }
            for _ in 0..30 {
                net.step(1.0 / 60.0, &params);
            }
        }
        // Once settled, nothing drifts.
        let mut settled = false;
        for _ in 0..5000 {
            if !net.step(1.0 / 60.0, &params) {
                settled = true;
                break;
            }
        }
        assert!(settled, "iter {iter}: never settles after dragging");
        let resting: Vec<Point> = (0..n).map(|i| net.node_pos(i)).collect();
        for _ in 0..10 {
            net.step(1.0 / 60.0, &params);
        }
        assert!(
            (0..n).all(|i| net.node_pos(i) == resting[i]),
            "iter {iter}: drifts after settling"
        );
        for i in 0..n {
            let p = net.node_pos(i);
            assert!(
                p.x.is_finite() && p.y.is_finite(),
                "iter {iter}: NaN node {i}"
            );
        }
        for e in 0..inp.edges.len() {
            let pts: Vec<Point> = net.edge_points(e).collect();
            assert!(pts.len() >= 2);
            assert!(
                pts.iter().all(|p| p.x.is_finite() && p.y.is_finite()),
                "iter {iter}: NaN edge"
            );
        }
        net.reset();
        settled = false;
        for _ in 0..5000 {
            if !net.step(1.0 / 60.0, &params) {
                settled = true;
                break;
            }
        }
        assert!(settled, "iter {iter}: never settles after reset");
        // Everything returns to the layout after a reset.
        let residual = (0..n)
            .map(|i| {
                let p = net.node_pos(i);
                (p.x - l.nodes[i].x).abs().max((p.y - l.nodes[i].y).abs())
            })
            .fold(0.0f32, f32::max);
        assert!(
            residual < 1.0,
            "iter {iter} {params:?}: {residual} px off after reset"
        );
        assert!(
            (0..inp.edges.len()).all(|e| !net.is_rerouted(e)),
            "iter {iter}: edges keep routes of their own after a reset"
        );
    }
}

#[test]
fn physics_keeps_children_above_parents() {
    let mut rng = Rng(777);
    for iter in 0..120 {
        let n = 2 + rng.below(50) as usize;
        let inp = input(&mut rng, n);
        let opts = LayoutOptions {
            concentrate_edges: iter % 2 == 0,
            ranking: Ranking::ALL[iter % 3],
            direction: Direction::ALL[iter % 4],
            ..LayoutOptions::default()
        };
        let l = layout::layout(&inp, &opts);
        let mut net = Net::new(&l, &inp.sizes);
        let params = NetParams {
            model: DragModel::Adapt,
            pull: rng.f(),
            push: rng.f(),
            wobble: rng.f(),
            avoid_overlap: iter % 3 != 0,
        };
        // Drag one node far along or against the flow (and a bit sideways), and drop it.
        let node = rng.below(n as u64) as usize;
        let f = opts.direction.flow();
        let (along, side) = (1200.0 * (rng.f() - 0.5), 200.0 * (rng.f() - 0.5));
        net.grab(node, &[node], &[], true);
        for s in 1..=20 {
            let t = s as f32 / 20.0;
            net.drag_to(Point::new(
                l.nodes[node].x + t * (along * f.x + side * f.y),
                l.nodes[node].y + t * (along * f.y + side * f.x),
            ));
            net.step(1.0 / 60.0, &params);
        }
        net.release(&params);
        let mut settled = false;
        for _ in 0..5000 {
            if !net.step(1.0 / 60.0, &params) {
                settled = true;
                break;
            }
        }
        assert!(settled, "iter {iter}: never settles");
        let depth = |i: usize| {
            let s = inp.sizes[i];
            (f.x.abs() * s.x + f.y.abs() * s.y) / 2.0
        };
        let along_flow = |p: Point| p.x * f.x + p.y * f.y;
        for e in &inp.edges {
            let (c, p) = (e.child as usize, e.parent as usize);
            if c == p {
                continue;
            }
            let needed = (depth(c) + depth(p) + FLOW_GAP)
                .min(along_flow(l.nodes[p]) - along_flow(l.nodes[c]));
            let have = along_flow(net.node_pos(p)) - along_flow(net.node_pos(c));
            assert!(
                have >= needed - 1.0,
                "iter {iter} {params:?}, dragged {node} by {along}: edge {c} -> {p} \
                 has {have} along the flow, needs {needed}"
            );
        }
    }
}

/// Putting moved nodes back into a layout they no longer fit (as after a reload that changed
/// the graph) leaves no boxes overlapping: nodes moved by hand land on others (and on each
/// other), some beside their first parent, and a few others rest a little away from the
/// layout. Putting the result back again changes nothing.
#[test]
fn physics_restores_without_overlaps() {
    let mut rng = Rng(env_or("PHYSICS_SEED", 1990));
    for iter in 0..env_or("PHYSICS_ITERS", 120) as usize {
        let n = 2 + rng.below(60) as usize;
        let inp = input(&mut rng, n);
        let opts = LayoutOptions {
            concentrate_edges: iter % 2 == 0,
            ranking: Ranking::ALL[iter % 3],
            direction: Direction::ALL[iter % 4],
            ..LayoutOptions::default()
        };
        let l = layout::layout(&inp, &opts);
        let first_parent = |i: usize| {
            inp.edges
                .iter()
                .find(|e| e.child as usize == i && e.first_parent)
                .map(|e| e.parent as usize)
        };
        let mut saved: Vec<RestPlace> = Vec::new();
        let moved = 1 + rng.below(3) as usize;
        for k in 0..moved + rng.below(4) as usize {
            let node = rng.below(n as u64) as usize;
            if saved.iter().any(|s| s.node == node) {
                continue;
            }
            let by_hand = k < moved;
            let offset = if by_hand {
                // Onto another node, give or take a little.
                let onto = l.nodes[rng.below(n as u64) as usize];
                Point::new(
                    onto.x - l.nodes[node].x + rng.f() * 30.0 - 15.0,
                    onto.y - l.nodes[node].y + rng.f() * 10.0 - 5.0,
                )
            } else {
                Point::new(rng.f() * 80.0 - 40.0, rng.f() * 20.0 - 10.0)
            };
            let beside = first_parent(node).filter(|_| rng.below(2) == 0).map(|p| {
                (
                    p,
                    Point::new(rng.f() * 300.0 - 150.0, -40.0 - rng.f() * 80.0),
                )
            });
            saved.push(RestPlace {
                node,
                offset,
                moved: by_hand,
                beside,
            });
        }
        let mut net = Net::new(&l, &inp.sizes);
        net.restore(saved.clone(), &NetParams::default());
        assert!(!net.is_awake(), "iter {iter}: settles at once");
        for i in 0..n {
            let p = net.node_pos(i);
            assert!(p.x.is_finite() && p.y.is_finite(), "iter {iter}: node {i}");
        }
        assert_eq!(net.tidiness().overlaps, 0, "iter {iter}: n={n}");
        if let [only] = saved.as_slice() {
            let mut alone = Net::new(&l, &inp.sizes);
            alone.restore([*only], &NetParams::default());
            assert!(
                (alone.node_pos(only.node).x - net.node_pos(only.node).x).abs() < 0.01,
                "iter {iter}: the node moved by hand stays"
            );
        }
        let mut again = Net::new(&l, &inp.sizes);
        again.restore(net.rest_places(), &NetParams::default());
        for i in 0..n {
            let (a, b) = (net.node_pos(i), again.node_pos(i));
            assert!(
                (a.x - b.x).abs() < 0.01 && (a.y - b.y).abs() < 0.01,
                "iter {iter}: node {i} put back again moved from {a:?} to {b:?}"
            );
        }
    }
}
