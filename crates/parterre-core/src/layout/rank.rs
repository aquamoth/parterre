//! Layers are numbered from 0, the newest, up; every edge from a child to its parent goes to
//! a strictly greater layer.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use super::{LayoutInput, Ranking, layer_count};

/// Network simplex stops improving after this many pivots per component; the layering is
/// valid all the same, if maybe not minimal.
const MAX_SIMPLEX_PIVOTS: usize = 5_000;
const MIN_SIMPLEX_PIVOTS: usize = 50;
/// Pivots times graph size.
const SIMPLEX_WORK_BUDGET: usize = 60_000_000;
/// How much fuller than evenly spread the rows of a split layer may get.
const ROW_FILL_SLACK: f32 = 1.05;

pub fn rank(input: &LayoutInput, ranking: Ranking) -> Vec<u32> {
    let graph = RankGraph::new(input);
    match ranking {
        Ranking::LongestPath => graph.longest_path(),
        Ranking::Compact => {
            // Each pivot costs O(V + E); keep the total work bounded for huge or odd graphs.
            let size = graph.n + graph.tail.len();
            let pivots =
                (SIMPLEX_WORK_BUDGET / size.max(1)).clamp(MIN_SIMPLEX_PIVOTS, MAX_SIMPLEX_PIVOTS);
            graph.network_simplex(pivots)
        }
        Ranking::Chronological => chronological(input),
    }
}

/// A simple directed graph with parallel edges merged: tail = child, head = parent.
#[derive(Debug)]
struct RankGraph {
    n: usize,
    tail: Vec<usize>,
    head: Vec<usize>,
    weight: Vec<i64>,
    minlen: Vec<i64>,
    incident: Vec<Vec<usize>>,
}

impl RankGraph {
    fn new(input: &LayoutInput) -> RankGraph {
        let n = input.sizes.len();
        let mut merged: HashMap<(usize, usize), usize> = HashMap::new();
        let mut g = RankGraph {
            n,
            tail: Vec::new(),
            head: Vec::new(),
            weight: Vec::new(),
            minlen: Vec::new(),
            incident: vec![Vec::new(); n],
        };
        for e in &input.edges {
            let (t, h) = (e.child as usize, e.parent as usize);
            if t == h {
                continue;
            }
            // First-parent edges weigh more so that mainlines stay short and straight.
            let w = if e.first_parent { 2 } else { 1 };
            if let Some(&id) = merged.get(&(t, h)) {
                // A duplicate edge (e.g. both parents of a merge collapse onto the same
                // ancestor): stretch it over two layers so the two paths can be told apart.
                g.weight[id] += w;
                g.minlen[id] = 2;
                continue;
            }
            let id = g.tail.len();
            merged.insert((t, h), id);
            g.tail.push(t);
            g.head.push(h);
            g.weight.push(w);
            g.minlen.push(1);
            g.incident[t].push(id);
            g.incident[h].push(id);
        }
        g
    }

    fn other(&self, e: usize, v: usize) -> usize {
        if self.tail[e] == v {
            self.head[e]
        } else {
            self.tail[e]
        }
    }

    fn slack(&self, rank: &[i64], e: usize) -> i64 {
        rank[self.head[e]] - rank[self.tail[e]] - self.minlen[e]
    }

    /// Children before parents. A cycle (impossible in git) is broken anywhere.
    fn topo_order(&self) -> Vec<usize> {
        let mut indeg = vec![0usize; self.n];
        for &h in &self.head {
            indeg[h] += 1;
        }
        let mut stack: Vec<usize> = (0..self.n).rev().filter(|&v| indeg[v] == 0).collect();
        let mut order = Vec::with_capacity(self.n);
        let mut done = vec![false; self.n];
        let mut next_unvisited = 0;
        loop {
            while let Some(v) = stack.pop() {
                if done[v] {
                    continue;
                }
                done[v] = true;
                order.push(v);
                for &e in &self.incident[v] {
                    if self.tail[e] == v {
                        let h = self.head[e];
                        // A node forced out of a cycle may already be done.
                        indeg[h] = indeg[h].saturating_sub(1);
                        if indeg[h] == 0 && !done[h] {
                            stack.push(h);
                        }
                    }
                }
            }
            while next_unvisited < self.n && done[next_unvisited] {
                next_unvisited += 1;
            }
            if next_unvisited == self.n {
                break;
            }
            // Only reachable through a cycle: break it at the first unvisited node.
            stack.push(next_unvisited);
        }
        order
    }

    fn longest_path(&self) -> Vec<u32> {
        let rank = self.longest_path_ranks();
        rank.iter().map(|&r| r as u32).collect()
    }

    fn longest_path_ranks(&self) -> Vec<i64> {
        let order = self.topo_order();
        let mut height = vec![0i64; self.n];
        for &v in order.iter().rev() {
            for &e in &self.incident[v] {
                if self.tail[e] == v {
                    height[v] = height[v].max(height[self.head[e]] + self.minlen[e]);
                }
            }
        }
        let top = height.iter().copied().max().unwrap_or(0);
        height.iter().map(|h| top - h).collect()
    }

    /// Minimises the weighted total edge length (Gansner et al., "A Technique for Drawing
    /// Directed Graphs", 1993), starting from the longest-path layering.
    fn network_simplex(&self, max_pivots: usize) -> Vec<u32> {
        let mut rank = self.longest_path_ranks();
        let mut in_tree = vec![false; self.tail.len()];
        let mut in_comp = vec![false; self.n];
        let mut base = vec![0i64; self.n];
        let mut simplex: Option<Simplex> = None;
        for start in 0..self.n {
            if in_comp[start] {
                continue;
            }
            let component =
                self.feasible_tree(start, &mut rank, &mut in_tree, &mut in_comp, &mut base);
            if component.len() >= 2 {
                simplex.get_or_insert_with(|| Simplex::new(self)).run(
                    &component,
                    &mut in_tree,
                    &mut rank,
                    max_pivots,
                );
            }
        }
        normalize(&rank)
    }

    /// Grows a spanning tree of tight edges from `start`, shifting the tree's ranks to make
    /// the minimum-slack crossing edge tight whenever it gets stuck. Returns the component.
    fn feasible_tree(
        &self,
        start: usize,
        rank: &mut [i64],
        in_tree: &mut [bool],
        in_comp: &mut [bool],
        base: &mut [i64],
    ) -> Vec<usize> {
        // Tree nodes move together: actual rank = base[v] + shift.
        let mut shift = 0i64;
        let mut component = Vec::new();
        // Crossing edges keyed so the key stays valid as `shift` changes:
        // tail in tree: slack = key - shift; head in tree: slack = key + shift.
        let mut out_heap: BinaryHeap<Reverse<(i64, usize)>> = BinaryHeap::new();
        let mut in_heap: BinaryHeap<Reverse<(i64, usize)>> = BinaryHeap::new();
        let mut stack = vec![start];

        loop {
            // Add everything reachable over tight edges.
            while let Some(v) = stack.pop() {
                if in_comp[v] {
                    continue;
                }
                in_comp[v] = true;
                base[v] = rank[v] - shift;
                component.push(v);
                for &e in &self.incident[v] {
                    let w = self.other(e, v);
                    if in_comp[w] {
                        continue;
                    }
                    let v_rank = base[v] + shift;
                    let (slack, v_is_tail) = if self.tail[e] == v {
                        (rank[w] - v_rank - self.minlen[e], true)
                    } else {
                        (v_rank - rank[w] - self.minlen[e], false)
                    };
                    if slack == 0 {
                        in_tree[e] = true;
                        stack.push(w);
                    } else if v_is_tail {
                        out_heap.push(Reverse((slack + shift, e)));
                    } else {
                        in_heap.push(Reverse((slack - shift, e)));
                    }
                }
            }
            // Discard edges that no longer cross the tree boundary.
            let crossing =
                |e: usize, in_comp: &[bool]| in_comp[self.tail[e]] != in_comp[self.head[e]];
            while out_heap
                .peek()
                .is_some_and(|Reverse((_, e))| !crossing(*e, in_comp))
            {
                out_heap.pop();
            }
            while in_heap
                .peek()
                .is_some_and(|Reverse((_, e))| !crossing(*e, in_comp))
            {
                in_heap.pop();
            }
            let out_best = out_heap.peek().map(|Reverse((k, e))| (k - shift, *e));
            let in_best = in_heap.peek().map(|Reverse((k, e))| (k + shift, *e));
            let (slack, e, tail_in_tree) = match (out_best, in_best) {
                (None, None) => break,
                (Some((s, e)), None) => (s, e, true),
                (None, Some((s, e))) => (s, e, false),
                (Some((so, eo)), Some((si, ei))) => {
                    if so <= si {
                        (so, eo, true)
                    } else {
                        (si, ei, false)
                    }
                }
            };
            // Move the tree towards the edge's outside end so the edge becomes tight.
            shift += if tail_in_tree { slack } else { -slack };
            in_tree[e] = true;
            stack.push(if tail_in_tree {
                self.head[e]
            } else {
                self.tail[e]
            });
        }
        for &v in &component {
            rank[v] = base[v] + shift;
        }
        // Several tight edges may have reached one node.
        self.prune_to_tree(&component, in_tree);
        component
    }

    fn prune_to_tree(&self, component: &[usize], in_tree: &mut [bool]) {
        let mut visited = vec![false; self.n];
        let mut used = vec![false; self.tail.len()];
        let mut stack = vec![component[0]];
        visited[component[0]] = true;
        while let Some(v) = stack.pop() {
            for &e in &self.incident[v] {
                if !in_tree[e] {
                    continue;
                }
                let w = self.other(e, v);
                if !visited[w] {
                    visited[w] = true;
                    used[e] = true;
                    stack.push(w);
                }
            }
        }
        for &v in component {
            for &e in &self.incident[v] {
                in_tree[e] &= used[e];
            }
        }
    }
}

/// Network simplex pivoting over one connected component's feasible tree. Its vectors are
/// indexed by node or edge and reused across components.
struct Simplex<'a> {
    g: &'a RankGraph,
    /// The edge to every node's tree parent; [`NO_EDGE`] for the root.
    parent_edge: Vec<usize>,
    /// Every node's postorder number, and the smallest one in its subtree.
    lim: Vec<usize>,
    low: Vec<usize>,
    /// Only meaningful for tree edges.
    cut: Vec<i64>,
}

const NO_EDGE: usize = usize::MAX;

impl<'a> Simplex<'a> {
    fn new(g: &'a RankGraph) -> Simplex<'a> {
        Simplex {
            g,
            parent_edge: vec![NO_EDGE; g.n],
            lim: vec![0; g.n],
            low: vec![0; g.n],
            cut: vec![0; g.tail.len()],
        }
    }

    fn run(&mut self, nodes: &[usize], in_tree: &mut [bool], rank: &mut [i64], max_pivots: usize) {
        let g = self.g;
        let mut edges: Vec<usize> = nodes
            .iter()
            .flat_map(|&v| {
                g.incident[v]
                    .iter()
                    .copied()
                    .filter(move |&e| g.tail[e] == v)
            })
            .collect();
        edges.sort_unstable();
        let mut search_from = 0;
        self.init_low_lim(nodes, in_tree);
        self.init_cut_values(nodes, in_tree);
        for _ in 0..max_pivots {
            let Some(leave) = self.leave_edge(&edges, in_tree, &mut search_from) else {
                break;
            };
            let Some(enter) = self.enter_edge(leave, &edges, in_tree, rank) else {
                break;
            };
            in_tree[leave] = false;
            in_tree[enter] = true;
            self.init_low_lim(nodes, in_tree);
            self.init_cut_values(nodes, in_tree);
            self.update_ranks(nodes, rank);
        }
    }

    /// Depth-first from `nodes[0]`.
    fn init_low_lim(&mut self, nodes: &[usize], in_tree: &[bool]) {
        for &v in nodes {
            self.parent_edge[v] = NO_EDGE;
        }
        let mut next = 1usize;
        // (node, position in its incident list, low)
        let mut stack: Vec<(usize, usize, usize)> = vec![(nodes[0], 0, next)];
        while let Some(top) = stack.last_mut() {
            let v = top.0;
            let incident = &self.g.incident[v];
            let mut child = None;
            while top.1 < incident.len() {
                let e = incident[top.1];
                top.1 += 1;
                if in_tree[e] && self.parent_edge[v] != e {
                    child = Some((self.g.other(e, v), e));
                    break;
                }
            }
            match child {
                Some((w, e)) => {
                    self.parent_edge[w] = e;
                    stack.push((w, 0, next));
                }
                None => {
                    self.low[v] = top.2;
                    self.lim[v] = next;
                    next += 1;
                    stack.pop();
                }
            }
        }
    }

    fn init_cut_values(&mut self, nodes: &[usize], in_tree: &[bool]) {
        let mut postorder: Vec<usize> = nodes.to_vec();
        postorder.sort_unstable_by_key(|&v| self.lim[v]);
        for &v in &postorder {
            let e = self.parent_edge[v];
            if e != NO_EDGE {
                self.cut[e] = self.cut_value(v, e, in_tree);
            }
        }
    }

    /// Cut value of the tree edge `te` joining `child` to its tree parent, given the cut
    /// values of all tree edges below `child` (the local formula from dagre).
    fn cut_value(&self, child: usize, te: usize, in_tree: &[bool]) -> i64 {
        let g = self.g;
        let child_is_tail = g.tail[te] == child;
        let parent = g.other(te, child);
        let mut cut = g.weight[te];
        for &e in &g.incident[child] {
            let other = g.other(e, child);
            if other == parent {
                continue;
            }
            let points_to_head = (g.tail[e] == child) == child_is_tail;
            let w = g.weight[e];
            cut += if points_to_head { w } else { -w };
            if in_tree[e] {
                cut += if points_to_head {
                    -self.cut[e]
                } else {
                    self.cut[e]
                };
            }
        }
        cut
    }

    fn leave_edge(
        &self,
        edges: &[usize],
        in_tree: &[bool],
        search_from: &mut usize,
    ) -> Option<usize> {
        let n = edges.len();
        let start = *search_from;
        let i = (0..n)
            .map(|k| (start + k) % n)
            .find(|&i| in_tree[edges[i]] && self.cut[edges[i]] < 0)?;
        *search_from = i + 1;
        Some(edges[i])
    }

    fn enter_edge(
        &self,
        leave: usize,
        edges: &[usize],
        in_tree: &[bool],
        rank: &[i64],
    ) -> Option<usize> {
        let (mut v, mut w) = (self.g.tail[leave], self.g.head[leave]);
        let mut flip = false;
        if self.lim[v] > self.lim[w] {
            std::mem::swap(&mut v, &mut w);
            flip = true;
        }
        // `v` is now the endpoint on the subtree side of the cut.
        let (sub_low, sub_lim) = (self.low[v], self.lim[v]);
        let in_sub = |x: usize| (sub_low..=sub_lim).contains(&self.lim[x]);
        edges
            .iter()
            .copied()
            .filter(|&e| !in_tree[e])
            .filter(|&e| flip == in_sub(self.g.tail[e]) && flip != in_sub(self.g.head[e]))
            .min_by_key(|&e| self.g.slack(rank, e))
    }

    /// Recomputes ranks from the root so that every tree edge is tight.
    fn update_ranks(&self, nodes: &[usize], rank: &mut [i64]) {
        let mut order: Vec<usize> = nodes.to_vec();
        // A parent's postorder number exceeds all of its descendants'.
        order.sort_unstable_by_key(|&v| Reverse(self.lim[v]));
        for &v in &order {
            let e = self.parent_edge[v];
            if e == NO_EDGE {
                continue;
            }
            let p = self.g.other(e, v);
            rank[v] = if self.g.tail[e] == v {
                rank[p] - self.g.minlen[e]
            } else {
                rank[p] + self.g.minlen[e]
            };
        }
    }
}

/// Splits layers wider than `max_width` into several rows, so that many siblings (typically
/// branch tips forking from one commit) stack up instead of forming one enormous row.
///
/// Rows are built bottom-up. A layer that fits becomes one row. Consecutive overfull layers are
/// placed together, node by node in depth-first order (a node, then its children as soon as all
/// their parents are placed), each into the lowest row above all its parents that has room, so
/// that parents and their children end up in neighbouring rows rather than in two separate
/// stacks.
pub fn limit_width(
    layers: &mut [u32],
    input: &LayoutInput,
    breadth: &[f32],
    max_width: f32,
    node_gap: f32,
) {
    let n = layers.len();
    if max_width.is_nan() || max_width <= 0.0 || n == 0 {
        return;
    }
    let mut parents: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in &input.edges {
        let (c, p) = (e.child as usize, e.parent as usize);
        if c != p {
            parents[c].push(p);
            children[p].push(c);
        }
    }

    let layer_count = layer_count(layers);
    let mut bottom_up: Vec<Vec<usize>> = vec![Vec::new(); layer_count];
    for (v, &l) in layers.iter().enumerate() {
        bottom_up[layer_count - 1 - l as usize].push(v);
    }
    bottom_up.retain(|l| !l.is_empty());
    let width_of = |nodes: &[usize]| {
        nodes.iter().map(|&v| breadth[v]).sum::<f32>()
            + node_gap * nodes.len().saturating_sub(1) as f32
    };
    let overfull = |nodes: &[usize]| nodes.len() > 1 && width_of(nodes) > max_width;

    let mut rows = Rows {
        nodes: Vec::new(),
        width: Vec::new(),
        row_of: vec![usize::MAX; n],
        node_gap,
    };
    // Rows below this one are closed to the layers still to come.
    let mut region_start = 0;
    let mut i = 0;
    while i < bottom_up.len() {
        if !overfull(&bottom_up[i]) {
            let first = rows.nodes.len().max(region_start);
            let r = rows.open(first);
            for &v in &bottom_up[i] {
                rows.put(v, r, breadth[v]);
            }
            region_start = r + 1;
            i += 1;
            continue;
        }
        let mut j = i;
        while j + 1 < bottom_up.len() && overfull(&bottom_up[j + 1]) {
            j += 1;
        }
        let run: Vec<usize> = bottom_up[i..=j].iter().flatten().copied().collect();
        let mut in_run = vec![false; n];
        for &v in &run {
            in_run[v] = true;
        }
        let mut pending: Vec<usize> = vec![0; n];
        for &v in &run {
            pending[v] = parents[v].iter().filter(|&&p| in_run[p]).count();
        }
        // Rows are filled evenly: as full as the widest layer of the run needs when spread
        // over its rows, never beyond the limit.
        let capacity = bottom_up[i..=j]
            .iter()
            .map(|l| {
                let total = width_of(l);
                total / (total / max_width).ceil() * ROW_FILL_SLACK
            })
            .fold(0.0f32, f32::max)
            .min(max_width);
        let floor = |v: usize, row_of: &[usize]| {
            parents[v]
                .iter()
                .map(|&p| row_of[p] + 1)
                .max()
                .unwrap_or(0)
                .max(region_start)
        };
        let mut ready: Vec<usize> = run.iter().copied().filter(|&v| pending[v] == 0).collect();
        // Nodes whose parents sit lowest first, then nodes with children, then oldest; the
        // stack pops from the end.
        ready.sort_by_key(|&v| {
            Reverse((
                floor(v, &rows.row_of),
                children[v].is_empty(),
                input.time(v),
                v,
            ))
        });
        let last_layer = layers[bottom_up[j][0]];
        let mut last_layer_rows = usize::MAX;
        while let Some(v) = ready.pop() {
            let lowest = floor(v, &rows.row_of);
            let r = (lowest..rows.nodes.len())
                .find(|&r| rows.fits(r, breadth[v], capacity))
                .unwrap_or_else(|| rows.open(rows.nodes.len().max(lowest)));
            rows.put(v, r, breadth[v]);
            if layers[v] == last_layer {
                last_layer_rows = last_layer_rows.min(r);
            }
            let mut next: Vec<usize> = children[v]
                .iter()
                .copied()
                .filter(|&c| in_run[c])
                .filter(|&c| {
                    pending[c] = pending[c].saturating_sub(1);
                    pending[c] == 0
                })
                .collect();
            next.sort_by_key(|&c| Reverse((input.time(c), c)));
            ready.extend(next);
        }
        // Nodes on a cycle (impossible in git) never become ready: put them on top.
        for &v in &run {
            if rows.row_of[v] == usize::MAX {
                let r = rows.open(rows.nodes.len());
                rows.put(v, r, breadth[v]);
            }
        }
        region_start = last_layer_rows.saturating_add(1).min(rows.nodes.len());
        i = j + 1;
    }
    let top = rows.nodes.len().saturating_sub(1);
    for (v, &r) in rows.row_of.iter().enumerate() {
        layers[v] = (top - r) as u32;
    }
}

/// Bottom first.
struct Rows {
    nodes: Vec<Vec<usize>>,
    width: Vec<f32>,
    row_of: Vec<usize>,
    node_gap: f32,
}

impl Rows {
    /// Creates row `r` and any missing below it, and returns `r`.
    fn open(&mut self, r: usize) -> usize {
        while self.nodes.len() <= r {
            self.nodes.push(Vec::new());
            self.width.push(0.0);
        }
        r
    }

    fn fits(&self, r: usize, w: f32, capacity: f32) -> bool {
        self.nodes[r].is_empty() || self.width[r] + self.node_gap + w <= capacity
    }

    fn put(&mut self, v: usize, r: usize, w: f32) {
        let gap = if self.nodes[r].is_empty() {
            0.0
        } else {
            self.node_gap
        };
        self.width[r] += gap + w;
        self.nodes[r].push(v);
        self.row_of[v] = r;
    }
}

fn normalize(rank: &[i64]) -> Vec<u32> {
    let min = rank.iter().copied().min().unwrap_or(0);
    rank.iter().map(|&r| (r - min) as u32).collect()
}

/// Never places a parent above its child.
fn chronological(input: &LayoutInput) -> Vec<u32> {
    let n = input.sizes.len();
    let mut pending_children = vec![0usize; n];
    let mut parents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in &input.edges {
        let (c, p) = (e.child as usize, e.parent as usize);
        if c != p {
            pending_children[p] += 1;
            parents[c].push(p);
        }
    }
    let mut ready: BinaryHeap<(i64, Reverse<usize>)> = (0..n)
        .filter(|&v| pending_children[v] == 0)
        .map(|v| (input.time(v), Reverse(v)))
        .collect();
    let mut layer = vec![u32::MAX; n];
    let mut next = 0u32;
    let mut oldest_first: Vec<usize> = (0..n).collect();
    oldest_first.sort_by_key(|&v| input.time(v));
    while (next as usize) < n {
        let Some((_, Reverse(v))) = ready.pop() else {
            // Only possible with a cycle: release the newest unplaced node.
            while oldest_first.last().is_some_and(|&v| layer[v] != u32::MAX) {
                oldest_first.pop();
            }
            let &v = oldest_first.last().expect("a node is unplaced");
            ready.push((input.time(v), Reverse(v)));
            continue;
        };
        if layer[v] != u32::MAX {
            continue;
        }
        layer[v] = next;
        next += 1;
        for &p in &parents[v] {
            pending_children[p] = pending_children[p].saturating_sub(1);
            if pending_children[p] == 0 && layer[p] == u32::MAX {
                ready.push((input.time(p), Reverse(p)));
            }
        }
    }
    layer
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{LayoutEdge, Point};

    fn input(n: usize, edges: &[(u32, u32)]) -> LayoutInput {
        LayoutInput {
            sizes: vec![Point::new(10.0, 10.0); n],
            times: (0..n as i64).rev().collect(),
            edges: edges
                .iter()
                .map(|&(child, parent)| LayoutEdge {
                    child,
                    parent,
                    first_parent: true,
                })
                .collect(),
            priority: Vec::new(),
        }
    }

    fn assert_valid(input: &LayoutInput, layers: &[u32]) {
        for e in &input.edges {
            assert!(
                layers[e.parent as usize] > layers[e.child as usize],
                "edge {e:?} not downward in {layers:?}"
            );
        }
        assert_eq!(layers.iter().min(), Some(&0));
    }

    fn total_length(input: &LayoutInput, layers: &[u32]) -> u32 {
        input
            .edges
            .iter()
            .map(|e| layers[e.parent as usize] - layers[e.child as usize])
            .sum()
    }

    /// 0 (tip) -> 1 -> 2 (root), and a short side branch 3 -> 2 (tip on an old commit).
    const SIDE_BRANCH: &[(u32, u32)] = &[(0, 1), (1, 2), (3, 2)];

    #[test]
    fn longest_path_hangs_branches_near_fork() {
        let inp = input(4, SIDE_BRANCH);
        let l = rank(&inp, Ranking::LongestPath);
        assert_valid(&inp, &l);
        assert_eq!(l, vec![0, 1, 2, 1]);
    }

    #[test]
    fn network_simplex_is_valid_and_no_longer_than_longest_path() {
        // A diamond with a long side: merge 0 of (1 -> 2 -> 3 -> 5) and (4 -> 5), plus tips.
        let edges = &[
            (0, 1),
            (0, 4),
            (1, 2),
            (2, 3),
            (3, 5),
            (4, 5),
            (6, 4),
            (7, 6),
        ];
        let inp = input(8, edges);
        let ns = rank(&inp, Ranking::Compact);
        let lp = rank(&inp, Ranking::LongestPath);
        assert_valid(&inp, &ns);
        assert!(total_length(&inp, &ns) <= total_length(&inp, &lp));
    }

    #[test]
    fn network_simplex_pulls_isolated_tip_down() {
        // Tip 3 hangs off root 2; tip 0 has a long chain. Optimal puts 3 right above 2.
        let inp = input(4, SIDE_BRANCH);
        let l = rank(&inp, Ranking::Compact);
        assert_valid(&inp, &l);
        assert_eq!(l[2] - l[3], 1);
    }

    #[test]
    fn handles_disconnected_components() {
        let inp = input(5, &[(0, 1), (2, 3), (3, 4)]);
        for r in Ranking::ALL {
            let l = rank(&inp, r);
            assert_valid(&inp, &l);
        }
    }

    #[test]
    fn chronological_gives_one_layer_per_node() {
        let inp = input(4, SIDE_BRANCH);
        let mut l = rank(&inp, Ranking::Chronological);
        assert_valid(&inp, &l);
        l.sort();
        assert_eq!(l, vec![0, 1, 2, 3]);
    }

    #[test]
    fn wide_layers_are_split_keeping_order() {
        // Twelve tips (1..=12) on one root (0), each 100 wide: one layer of 1200 + gaps.
        let edges: Vec<(u32, u32)> = (1..=12).map(|t| (t, 0)).collect();
        let mut inp = input(13, &edges);
        inp.sizes = vec![Point::new(100.0, 20.0); 13];
        let mut l = rank(&inp, Ranking::Compact);
        let breadth = vec![100.0; 13];
        limit_width(&mut l, &inp, &breadth, 450.0, 10.0);
        assert_valid(&inp, &l);
        let mut per_layer = std::collections::BTreeMap::new();
        for &x in &l[1..] {
            *per_layer.entry(x).or_insert(0) += 1;
        }
        assert!(per_layer.values().all(|&c| c <= 4), "{per_layer:?}");
        assert_eq!(per_layer.len(), 3);
    }

    #[test]
    fn split_layers_keep_children_close_to_their_parents() {
        // 400 parent/child pairs on one root: both the parent layer and the child layer overflow.
        let pairs = 400u32;
        let mut edges = Vec::new();
        for i in 0..pairs {
            let (parent, child) = (1 + 2 * i, 2 + 2 * i);
            edges.push((parent, 0));
            edges.push((child, parent));
        }
        let n = 1 + 2 * pairs as usize;
        let inp = input(n, &edges);
        let breadth = vec![100.0; n];
        let mut l = rank(&inp, Ranking::Compact);
        limit_width(&mut l, &inp, &breadth, 1800.0, 25.0);
        assert_valid(&inp, &l);
        // Edges into the root are long by necessity (400 siblings stack up); the edges from
        // each child to its own parent must stay short.
        let pair_edges = inp.edges.iter().filter(|e| e.parent != 0);
        let longest = pair_edges
            .map(|e| l[e.parent as usize] - l[e.child as usize])
            .max()
            .unwrap();
        assert!(
            longest <= 2,
            "child/parent edges stay short (longest {longest})"
        );
    }

    #[test]
    fn duplicate_edges_span_two_layers() {
        let inp = input(2, &[(0, 1), (0, 1)]);
        for r in [Ranking::Compact, Ranking::LongestPath] {
            let l = rank(&inp, r);
            assert_eq!(l[1] - l[0], 2, "{r:?}");
        }
    }
}
