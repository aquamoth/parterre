//! Property tests over random inputs (adapted from a review's fuzzing).

#![allow(clippy::needless_range_loop)] // index loops read better in these tests

use parterre_core::layout::rank::{limit_width, rank};
use parterre_core::layout::{
    self, Direction, LayoutEdge, LayoutInput, LayoutOptions, Point, Ranking, Trunk,
};
use std::collections::HashMap;

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
    fn chance(&mut self, p: f64) -> bool {
        (self.next() % 1_000_000) as f64 / 1e6 < p
    }
}

/// Random DAG with nodes permuted (so node order is not topological).
fn random_input(rng: &mut Rng, n: usize, dup: bool, selfloop: bool, permute: bool) -> LayoutInput {
    let mut perm: Vec<usize> = (0..n).collect();
    if permute {
        for i in (1..n).rev() {
            let j = rng.below(i as u64 + 1) as usize;
            perm.swap(i, j);
        }
    }
    let mut edges = Vec::new();
    for c in 0..n {
        // 0..3 parents with higher index
        let k = match rng.below(10) {
            0 => 0,
            1..=6 => 1,
            7..=8 => 2,
            _ => 3,
        };
        let mut first = true;
        for _ in 0..k {
            if c + 1 >= n {
                break;
            }
            let span = if rng.chance(0.7) {
                1 + rng.below(3)
            } else {
                1 + rng.below((n - c - 1) as u64)
            };
            let p = (c as u64 + span).min(n as u64 - 1) as usize;
            if p == c {
                continue;
            }
            edges.push(LayoutEdge {
                child: perm[c] as u32,
                parent: perm[p] as u32,
                first_parent: first,
            });
            if dup && rng.chance(0.1) {
                edges.push(LayoutEdge {
                    child: perm[c] as u32,
                    parent: perm[p] as u32,
                    first_parent: false,
                });
            }
            first = false;
        }
        if selfloop && rng.chance(0.05) {
            edges.push(LayoutEdge {
                child: perm[c] as u32,
                parent: perm[c] as u32,
                first_parent: false,
            });
        }
    }
    let sizes = (0..n)
        .map(|_| {
            Point::new(
                40.0 + rng.below(200) as f32,
                22.0 * (1 + rng.below(3)) as f32,
            )
        })
        .collect();
    let mut times: Vec<i64> = vec![0; n];
    for c in 0..n {
        times[perm[c]] = (n - c) as i64 * 100 + rng.below(250) as i64 - 125; // with some skew
    }
    let priority = if n > 0 && rng.chance(0.5) {
        vec![rng.below(n as u64) as u32]
    } else {
        vec![]
    };
    LayoutInput {
        sizes,
        times,
        edges,
        priority,
    }
}

fn check_layers(input: &LayoutInput, layers: &[u32], what: &str) {
    assert_eq!(layers.len(), input.sizes.len(), "{what}: length");
    for e in &input.edges {
        if e.child == e.parent {
            continue;
        }
        assert!(
            layers[e.parent as usize] > layers[e.child as usize],
            "{what}: edge {e:?} not downward: {} -> {}\ninput={input:?}\nlayers={layers:?}",
            layers[e.child as usize],
            layers[e.parent as usize]
        );
    }
    if !layers.is_empty() {
        assert_eq!(*layers.iter().min().unwrap(), 0, "{what}: min layer");
    }
}

fn weighted_length(input: &LayoutInput, layers: &[u32]) -> i64 {
    // Same merging semantics as RankGraph
    let mut merged: HashMap<(u32, u32), (i64, i64)> = HashMap::new();
    for e in &input.edges {
        if e.child == e.parent {
            continue;
        }
        let w = if e.first_parent { 2 } else { 1 };
        let ent = merged.entry((e.child, e.parent)).or_insert((0, 1));
        if ent.0 > 0 {
            ent.1 = 2;
        }
        ent.0 += w;
    }
    merged
        .iter()
        .map(|(&(c, p), &(w, _))| w * (layers[p as usize] as i64 - layers[c as usize] as i64))
        .sum()
}

fn minlen_ok(input: &LayoutInput, layers: &[i64]) -> bool {
    let mut merged: HashMap<(u32, u32), i64> = HashMap::new();
    for e in &input.edges {
        if e.child == e.parent {
            continue;
        }
        let ent = merged.entry((e.child, e.parent)).or_insert(0);
        *ent += 1;
    }
    merged.iter().all(|(&(c, p), &cnt)| {
        let ml = if cnt > 1 { 2 } else { 1 };
        layers[p as usize] - layers[c as usize] >= ml
    })
}

/// Brute force optimal weighted length for tiny graphs.
fn brute_optimum(input: &LayoutInput) -> i64 {
    let n = input.sizes.len();
    let maxr = (2 * n) as i64;
    let mut best = i64::MAX;
    let mut r = vec![0i64; n];
    fn rec(i: usize, n: usize, maxr: i64, r: &mut Vec<i64>, input: &LayoutInput, best: &mut i64) {
        if i == n {
            if minlen_ok(input, r) {
                let l: Vec<u32> = r.iter().map(|&x| x as u32).collect();
                let w = weighted_length(input, &l);
                if w < *best {
                    *best = w;
                }
            }
            return;
        }
        for v in 0..=maxr {
            r[i] = v;
            rec(i + 1, n, maxr, r, input, best);
        }
    }
    rec(0, n, maxr, &mut r, input, &mut best);
    best
}

#[test]
fn rank_all_modes_valid_random_dags() {
    let mut rng = Rng(0x9E3779B97F4A7C15);
    for iter in 0..600 {
        let n = 1 + rng.below(if iter % 10 == 0 { 300 } else { 40 }) as usize;
        let input = random_input(&mut rng, n, true, true, iter % 2 == 0);
        for r in Ranking::ALL {
            let mut l = rank(&input, r);
            check_layers(&input, &l, &format!("iter {iter} {r:?}"));
            let breadth: Vec<f32> = input.sizes.iter().map(|s| s.x).collect();
            for mw in [0.0, 100.0, 300.0, 1800.0, f32::NAN, 1.0] {
                let mut l2 = l.clone();
                limit_width(&mut l2, &input, &breadth, mw, 25.0);
                check_layers(&input, &l2, &format!("iter {iter} {r:?} limit {mw}"));
            }
            l.clear();
        }
        let ns = rank(&input, Ranking::Compact);
        let lp = rank(&input, Ranking::LongestPath);
        assert!(
            weighted_length(&input, &ns) <= weighted_length(&input, &lp),
            "iter {iter}: simplex worse than longest path: {} > {}\n{input:?}",
            weighted_length(&input, &ns),
            weighted_length(&input, &lp)
        );
    }
}

#[test]
fn simplex_is_optimal_on_tiny_graphs() {
    let mut rng = Rng(12345);
    let mut failures = 0;
    for iter in 0..100 {
        let n = 1 + rng.below(5) as usize;
        let input = random_input(&mut rng, n, true, false, true);
        let ns = rank(&input, Ranking::Compact);
        let got = weighted_length(&input, &ns);
        let opt = brute_optimum(&input);
        if got != opt {
            failures += 1;
            if failures < 5 {
                eprintln!(
                    "iter {iter}: simplex {got} vs optimum {opt}\nedges={:?}\nlayers={ns:?}",
                    input
                        .edges
                        .iter()
                        .map(|e| (e.child, e.parent, e.first_parent))
                        .collect::<Vec<_>>()
                );
            }
        }
    }
    assert_eq!(failures, 0, "non-optimal simplex results");
}

fn check_layout(input: &LayoutInput, opts: &LayoutOptions, what: &str) {
    let l = layout::layout(input, opts);
    let n = input.sizes.len();
    assert_eq!(l.nodes.len(), n, "{what}");
    assert_eq!(l.edges.len(), input.edges.len(), "{what}");
    if n > 0 {
        check_layers(input, &l.layers, what);
    }
    for p in &l.nodes {
        assert!(p.x.is_finite() && p.y.is_finite(), "{what}: node NaN {p:?}");
        assert!(
            p.x >= l.min.x - 0.01
                && p.x <= l.max.x + 0.01
                && p.y >= l.min.y - 0.01
                && p.y <= l.max.y + 0.01,
            "{what}: node {p:?} outside bounds {:?} {:?}",
            l.min,
            l.max
        );
    }
    for (ei, pts) in l.edges.iter().enumerate() {
        assert!(pts.len() >= 2, "{what}: edge with <2 pts");
        for p in pts {
            assert!(p.x.is_finite() && p.y.is_finite(), "{what}: edge NaN");
        }
        let e = input.edges[ei];
        if e.child == e.parent {
            continue;
        }
        // monotone along flow
        let f = opts.direction.flow();
        let proj: Vec<f32> = pts.iter().map(|p| p.x * f.x + p.y * f.y).collect();
        for w in proj.windows(2) {
            assert!(
                w[1] >= w[0] - 0.01,
                "{what}: edge {ei} {e:?} not monotone along flow: {proj:?}"
            );
        }
        // edge length vs layers: chain length = span - 1 dummies
        let span = l.layers[e.parent as usize] - l.layers[e.child as usize];
        assert_eq!(
            pts.len() as u32,
            span + 1,
            "{what}: edge {ei} points vs span"
        );
    }
    // Nodes in the same layer must not overlap along the layer.
    let vertical = opts.direction.is_vertical();
    let mut by_layer: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, &ly) in l.layers.iter().enumerate() {
        by_layer.entry(ly).or_default().push(i);
    }
    for (_, mut v) in by_layer {
        let along = |i: usize| if vertical { l.nodes[i].x } else { l.nodes[i].y };
        let br = |i: usize| {
            if vertical {
                input.sizes[i].x
            } else {
                input.sizes[i].y
            }
        };
        v.sort_by(|&a, &b| along(a).total_cmp(&along(b)));
        for w in v.windows(2) {
            let gap = along(w[1]) - along(w[0]) - (br(w[0]) + br(w[1])) / 2.0;
            assert!(
                gap >= opts.node_gap - 0.5,
                "{what}: nodes {} {} overlap in layer (gap {gap})",
                w[0],
                w[1]
            );
        }
    }
    // A centred trunk is one straight line.
    if opts.trunk != Trunk::Leftmost {
        let along = |i: usize| if vertical { l.nodes[i].x } else { l.nodes[i].y };
        let line = trunk_of(input);
        for &i in &line {
            assert!(
                (along(i) - along(line[0])).abs() < 0.5,
                "{what}: trunk node {i} at {} off the line at {}",
                along(i),
                along(line[0])
            );
        }
    }
}

/// The trunk's nodes: first parents from the first priority node down, or the only edge left
/// where a merge's first parent was dropped.
fn trunk_of(input: &LayoutInput) -> Vec<usize> {
    let Some(&anchor) = input.priority.first() else {
        return Vec::new();
    };
    let mut line = vec![anchor as usize];
    loop {
        let here = *line.last().unwrap() as u32;
        let next = input
            .edges
            .iter()
            .filter(|e| e.child == here)
            .min_by_key(|e| !e.first_parent);
        match next {
            Some(e) if !line.contains(&(e.parent as usize)) => line.push(e.parent as usize),
            _ => return line,
        }
    }
}

#[test]
fn layout_invariants_random() {
    let mut rng = Rng(0xDEADBEEF);
    for iter in 0..300 {
        let n = rng.below(if iter % 10 == 0 { 200 } else { 30 }) as usize;
        let input = random_input(&mut rng, n, iter % 3 == 0, iter % 5 == 0, iter % 2 == 0);
        for ranking in Ranking::ALL {
            for concentrate in [false, true] {
                for mw in [0.0f32, 400.0, 1800.0] {
                    let dir = Direction::ALL[rng.below(4) as usize];
                    let trunk = Trunk::ALL[rng.below(3) as usize];
                    let opts = LayoutOptions {
                        ranking,
                        concentrate_edges: concentrate,
                        max_layer_width: mw,
                        direction: dir,
                        trunk,
                        ..LayoutOptions::default()
                    };
                    check_layout(
                        &input,
                        &opts,
                        &format!(
                            "iter {iter} {ranking:?} conc={concentrate} mw={mw} {dir:?} {trunk:?}"
                        ),
                    );
                }
            }
        }
    }
}

#[test]
fn layout_edge_cases() {
    let opts = LayoutOptions::default();
    // empty
    let l = layout::layout(&LayoutInput::default(), &opts);
    assert!(l.nodes.is_empty());
    // single node, no edges
    let inp = LayoutInput {
        sizes: vec![Point::new(10.0, 10.0)],
        times: vec![],
        edges: vec![],
        priority: vec![],
    };
    for r in Ranking::ALL {
        check_layout(
            &inp,
            &LayoutOptions {
                ranking: r,
                ..opts.clone()
            },
            "single",
        );
    }
    // zero edges, many nodes
    let inp = LayoutInput {
        sizes: vec![Point::new(100.0, 10.0); 50],
        times: vec![],
        edges: vec![],
        priority: vec![],
    };
    for r in Ranking::ALL {
        check_layout(
            &inp,
            &LayoutOptions {
                ranking: r,
                ..opts.clone()
            },
            "noedges",
        );
    }
    // self loop only
    let inp = LayoutInput {
        sizes: vec![Point::new(10.0, 10.0); 2],
        times: vec![],
        edges: vec![LayoutEdge {
            child: 0,
            parent: 0,
            first_parent: true,
        }],
        priority: vec![],
    };
    for r in Ranking::ALL {
        check_layout(
            &inp,
            &LayoutOptions {
                ranking: r,
                ..opts.clone()
            },
            "selfloop",
        );
    }
    // zero-size nodes
    let inp = LayoutInput {
        sizes: vec![Point::new(0.0, 0.0); 3],
        times: vec![],
        edges: vec![
            LayoutEdge {
                child: 0,
                parent: 2,
                first_parent: true,
            },
            LayoutEdge {
                child: 1,
                parent: 2,
                first_parent: true,
            },
        ],
        priority: vec![],
    };
    let l = layout::layout(
        &inp,
        &LayoutOptions {
            node_gap: 0.0,
            edge_gap: 0.0,
            layer_gap: 0.0,
            ..opts.clone()
        },
    );
    for p in &l.nodes {
        assert!(p.x.is_finite() && p.y.is_finite());
    }
}

#[test]
fn cycle_does_not_hang() {
    // Cycles are impossible in git but make sure nothing loops forever / panics.
    let inp = LayoutInput {
        sizes: vec![Point::new(10.0, 10.0); 3],
        times: vec![3, 2, 1],
        edges: vec![
            LayoutEdge {
                child: 0,
                parent: 1,
                first_parent: true,
            },
            LayoutEdge {
                child: 1,
                parent: 2,
                first_parent: true,
            },
            LayoutEdge {
                child: 2,
                parent: 0,
                first_parent: true,
            },
        ],
        priority: vec![],
    };
    for r in Ranking::ALL {
        let l = rank(&inp, r);
        eprintln!("cycle {r:?}: {l:?}");
        let _ = layout::layout(
            &inp,
            &LayoutOptions {
                ranking: r,
                ..LayoutOptions::default()
            },
        );
    }
}

/// Timings on large and awkward inputs; slow, so run on demand:
/// `cargo test --release -p parterre-core --test properties_layout -- --ignored --nocapture`
#[test]
#[ignore]
fn perf_many_components_and_big_random() {
    use std::time::Instant;
    for n in [5_000usize, 20_000] {
        let inp = LayoutInput {
            sizes: vec![Point::new(80.0, 20.0); n],
            times: vec![],
            edges: vec![],
            priority: vec![],
        };
        let t = Instant::now();
        let _ = rank(&inp, Ranking::Compact);
        eprintln!("no edges n={n}: compact rank {:?}", t.elapsed());
        // pairs: n/2 components of 2
        let edges = (0..n / 2)
            .map(|i| LayoutEdge {
                child: (2 * i) as u32,
                parent: (2 * i + 1) as u32,
                first_parent: true,
            })
            .collect();
        let inp = LayoutInput {
            sizes: vec![Point::new(80.0, 20.0); n],
            times: vec![],
            edges,
            priority: vec![],
        };
        let t = Instant::now();
        let _ = rank(&inp, Ranking::Compact);
        eprintln!("pairs n={n}: compact rank {:?}", t.elapsed());
        let t = Instant::now();
        let _ = layout::layout(&inp, &LayoutOptions::default());
        eprintln!("pairs n={n}: full layout {:?}", t.elapsed());
    }
    let mut rng = Rng(777);
    for n in [5_000usize, 15_000] {
        let inp = random_input(&mut rng, n, false, false, false);
        let t = Instant::now();
        let _ = rank(&inp, Ranking::Compact);
        eprintln!(
            "random n={n} e={}: compact rank {:?}",
            inp.edges.len(),
            t.elapsed()
        );
        let t = Instant::now();
        let l = layout::layout(
            &inp,
            &LayoutOptions {
                concentrate_edges: true,
                ..LayoutOptions::default()
            },
        );
        eprintln!(
            "random n={n}: full layout {:?} dummies {}",
            t.elapsed(),
            l.edges.iter().map(|e| e.len() - 2).sum::<usize>()
        );
        let t = Instant::now();
        let l = layout::layout(
            &inp,
            &LayoutOptions {
                ranking: Ranking::Chronological,
                ..LayoutOptions::default()
            },
        );
        eprintln!(
            "random n={n}: chrono layout {:?} dummies {}",
            t.elapsed(),
            l.edges.iter().map(|e| e.len() - 2).sum::<usize>()
        );
    }
}

#[test]
fn limit_width_long_edges() {
    use std::time::Instant;
    for n in [2_000usize, 8_000] {
        let edges = (0..n / 2)
            .map(|i| LayoutEdge {
                child: (2 * i) as u32,
                parent: (2 * i + 1) as u32,
                first_parent: true,
            })
            .collect();
        let inp = LayoutInput {
            sizes: vec![Point::new(80.0, 20.0); n],
            times: vec![],
            edges,
            priority: vec![],
        };
        for mw in [0.0, 1800.0] {
            let t = Instant::now();
            let l = layout::layout(
                &inp,
                &LayoutOptions {
                    max_layer_width: mw,
                    ..LayoutOptions::default()
                },
            );
            let dummies: usize = l.edges.iter().map(|e| e.len() - 2).sum();
            let maxspan = l.edges.iter().map(|e| e.len() - 1).max().unwrap();
            eprintln!(
                "pairs n={n} max_width={mw}: layers {} dummies {dummies} longest edge spans {maxspan} layers, {:?}",
                l.layers.iter().max().unwrap() + 1,
                t.elapsed()
            );
        }
    }
}

/// A trunk `0 → 1 → … → len-1` with `branches` one-node side branches forking off its last node.
fn trunk_with_branches(len: u32, branches: u32) -> LayoutInput {
    let n = len + branches;
    let mut edges: Vec<LayoutEdge> = (0..len - 1)
        .map(|i| LayoutEdge {
            child: i,
            parent: i + 1,
            first_parent: true,
        })
        .collect();
    for b in 0..branches {
        edges.push(LayoutEdge {
            child: len + b,
            parent: len - 1,
            first_parent: true,
        });
    }
    LayoutInput {
        sizes: vec![Point::new(100.0, 22.0); n as usize],
        times: (0..n as i64).map(|i| 1000 - i).collect(),
        edges,
        priority: vec![0],
    }
}

#[test]
fn centred_trunk_has_branches_on_both_sides() {
    let input = trunk_with_branches(2, 2);
    let at = |trunk| {
        let l = layout::layout(
            &input,
            &LayoutOptions {
                trunk,
                ..LayoutOptions::default()
            },
        );
        (l.nodes[0].x, l.nodes[2].x, l.nodes[3].x)
    };
    // Centred: the tip in the middle of its layer, one branch either side.
    for trunk in [Trunk::Centred, Trunk::Alternating] {
        let (tip, a, b) = at(trunk);
        assert!(a.min(b) < tip && tip < a.max(b), "{trunk:?}: {a} {tip} {b}");
    }
    // Leftmost, as before: the tip first in its layer.
    let (tip, a, b) = at(Trunk::Leftmost);
    assert!(tip < a && tip < b, "Leftmost: {tip} {a} {b}");
}

#[test]
fn bundles_split_by_side() {
    // Four branches, newer than the whole trunk, into its oldest node: one layer per commit
    // makes them long, and bundled they would share their bend points and so one side.
    let mut input = trunk_with_branches(3, 4);
    for (i, t) in input.times.iter_mut().enumerate() {
        *t = if i < 3 {
            100 - i as i64
        } else {
            1000 - i as i64
        };
    }
    let l = layout::layout(
        &input,
        &LayoutOptions {
            concentrate_edges: true,
            ranking: Ranking::Chronological,
            ..LayoutOptions::default()
        },
    );
    let line = l.nodes[2].x;
    let left = (3..7).filter(|&i| l.nodes[i].x < line).count();
    assert_eq!(left, 2, "{:?}", l.nodes);
    // Where all four pass, one bend point per side.
    let mut through: HashMap<u32, std::collections::HashSet<u32>> = HashMap::new();
    for (bends, e) in l.edge_bends.iter().zip(&input.edges) {
        for (k, &b) in bends.iter().enumerate() {
            through
                .entry(l.layers[e.child as usize] + 1 + k as u32)
                .or_default()
                .insert(b);
        }
    }
    let last_branch_layer = (3..7).map(|i| l.layers[i]).max().unwrap();
    let shared = &through[&(last_branch_layer + 1)];
    assert_eq!(shared.len(), 2, "{:?}", l.edge_bends);
}

#[test]
fn trunk_follows_the_edge_left_of_a_merge() {
    // 1's first parent was dropped as redundant: its only edge is to its second parent.
    let mut input = trunk_with_branches(3, 2);
    input.edges[1].first_parent = false;
    let l = layout::layout(&input, &LayoutOptions::default());
    assert!((l.nodes[0].x - l.nodes[2].x).abs() < 0.5, "{:?}", l.nodes);
}
