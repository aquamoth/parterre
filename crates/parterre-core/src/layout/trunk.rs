//! PROTOTYPE (centred trunk, throwaway): which side of the trunk every item goes.
//!
//! The trunk is the first-parent line of the first priority node (the default branch), with
//! the bend points of its edges. Every other item belongs to a *side branch*: a connected
//! group of non-trunk items. A whole side branch goes on one side, so its line never crosses
//! the trunk. Ordering keeps `left | trunk | right` in every layer, and coordinate assignment
//! pins the trunk to one straight line.

use super::{LayeredGraph, LayoutInput, Trunk};

pub const LEFT: i8 = -1;
pub const TRUNK: i8 = 0;
pub const RIGHT: i8 = 1;

/// Side of every item (empty = no trunk, layout as before).
pub fn sides(g: &LayeredGraph, input: &LayoutInput, variant: Trunk, node_gap: f32) -> Vec<i8> {
    if variant == Trunk::Current {
        return Vec::new();
    }
    let Some(&anchor) = input.priority.first() else {
        return Vec::new();
    };
    let n_items = g.items.len();
    let mut side = vec![RIGHT; n_items];

    // Trunk: first parents from the anchor down, with their bend points. Where the graph
    // dropped a merge's first parent as redundant (it is an ancestor of another), the line goes
    // on through the edge that is left.
    let mut first_parent_edge = vec![usize::MAX; g.node_count];
    for (k, e) in input.edges.iter().enumerate() {
        let slot = &mut first_parent_edge[e.child as usize];
        if *slot == usize::MAX || (e.first_parent && !input.edges[*slot].first_parent) {
            *slot = k;
        }
    }
    let mut node = anchor as usize;
    loop {
        side[node] = TRUNK;
        let k = first_parent_edge[node];
        if k == usize::MAX {
            break;
        }
        for &d in &g.chains[k] {
            side[d as usize] = TRUNK;
        }
        node = input.edges[k].parent as usize;
        if side[node] == TRUNK {
            break;
        }
    }

    // Side branches: connected groups of non-trunk items.
    let mut group = vec![u32::MAX; n_items];
    let mut groups: Vec<Vec<u32>> = Vec::new();
    let mut stack = Vec::new();
    for start in 0..n_items {
        if side[start] == TRUNK || group[start] != u32::MAX {
            continue;
        }
        let id = groups.len() as u32;
        let mut members = Vec::new();
        group[start] = id;
        stack.push(start as u32);
        while let Some(i) = stack.pop() {
            members.push(i);
            for &(nb, _) in g.up(i as usize).iter().chain(g.down(i as usize)) {
                let nb_ = nb as usize;
                if side[nb_] != TRUNK && group[nb_] == u32::MAX {
                    group[nb_] = id;
                    stack.push(nb);
                }
            }
        }
        groups.push(members);
    }

    match variant {
        Trunk::Current => unreachable!(),
        Trunk::StraightLeft => {}
        Trunk::CentredBalanced => {
            // Biggest side branches first, each where the busiest of its layers stays narrowest.
            let layer_count = g.layers.len();
            let mut load = [vec![0.0f32; layer_count], vec![0.0f32; layer_count]];
            let mut order: Vec<usize> = (0..groups.len()).collect();
            let width = |m: &Vec<u32>| -> f32 {
                m.iter()
                    .map(|&i| g.items[i as usize].breadth + node_gap)
                    .sum()
            };
            let widths: Vec<f32> = groups.iter().map(width).collect();
            order.sort_by(|&a, &b| widths[b].total_cmp(&widths[a]).then(a.cmp(&b)));
            let mut per_layer: Vec<(usize, f32)> = Vec::new();
            for gi in order {
                per_layer.clear();
                for &i in &groups[gi] {
                    let it = &g.items[i as usize];
                    per_layer.push((it.layer as usize, it.breadth + node_gap));
                }
                let cost = |s: usize, per_layer: &[(usize, f32)]| {
                    // Peak width this side would reach in the group's layers.
                    let mut extra: std::collections::HashMap<usize, f32> = Default::default();
                    for &(l, w) in per_layer {
                        *extra.entry(l).or_default() += w;
                    }
                    extra
                        .into_iter()
                        .map(|(l, w)| load[s][l] + w)
                        .fold(0.0f32, f32::max)
                };
                let (cl, cr) = (cost(0, &per_layer), cost(1, &per_layer));
                let s = if cl < cr { 0 } else { 1 };
                for &(l, w) in &per_layer {
                    load[s][l] += w;
                }
                if s == 0 {
                    for &i in &groups[gi] {
                        side[i as usize] = LEFT;
                    }
                }
            }
        }
        Trunk::CentredAlternating => {
            // Side branches alternate right, left, right… from the newest fork down.
            let mut order: Vec<(u32, usize)> = groups
                .iter()
                .enumerate()
                .map(|(gi, m)| {
                    let deepest = m.iter().map(|&i| g.items[i as usize].layer).max();
                    (deepest.unwrap_or(0), gi)
                })
                .collect();
            order.sort_unstable();
            for (k, &(_, gi)) in order.iter().enumerate() {
                if k % 2 == 1 {
                    for &i in &groups[gi] {
                        side[i as usize] = LEFT;
                    }
                }
            }
        }
    }
    side
}
