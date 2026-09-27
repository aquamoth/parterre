//! The graph column of the log window: lanes beside the commit list, as TortoiseGit's log
//! draws them, showing how the listed commits go together.
//!
//! The rows are walked top to bottom keeping the lanes, each waiting for the commit it leads
//! to. A commit takes the leftmost lane waiting for it, or the first free one if none is (a
//! tip). Its first parent continues in its lane; another parent joins a lane already waiting
//! for it or opens a new one. Lanes keep their column while they pass rows by, so their lines
//! stay straight, and a lane that is freed stays free until a new line needs one.
//!
//! Decided with the prototype on the branch `prototype/log-graph` (variant A, "TortoiseGit").
//!
//! Memory stays small for long logs: [`LogGraph`] keeps only the lanes at every
//! [`CHECKPOINT`]th row and works out the rows asked for from the nearest one.

use std::ops::Range;

use crate::log::LogList;
use crate::repo::CommitIx;

/// Rows between two saved states of the lanes.
const CHECKPOINT: usize = 64;

/// What a lane is doing at the top of a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lane {
    Free,
    /// Waiting for the commit of this row.
    Waits(u32),
    /// Held for the row's first parent while its other parents find lanes.
    Held,
}

/// The lanes of a log.
#[derive(Clone, Debug, Default)]
pub struct LogGraph {
    /// For every row, the rows of its parents as the log shows them, first parent first.
    parents: Vec<Vec<u32>>,
    /// For every row, whether some of its history is outside the log.
    outside: Vec<bool>,
    /// The lanes at the top of row `k * CHECKPOINT`.
    checkpoints: Vec<Vec<Lane>>,
    /// The number of lanes the widest row uses.
    pub lanes: usize,
}

/// One row of the graph column.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphRow {
    /// The lane of the row's commit.
    pub lane: usize,
    /// The commit has more than one parent in the log.
    pub merge: bool,
    /// Some of the commit's history is outside the log: draw a line that leads out of it.
    pub outside: bool,
    /// The lines in the row's upper half, from the top edge to the middle.
    pub upper: Vec<GraphLine>,
    /// The lines in the row's lower half, from the middle to the bottom edge.
    pub lower: Vec<GraphLine>,
}

/// A line within half a row, from lane `from` at its top to lane `to` at its bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphLine {
    pub from: usize,
    pub to: usize,
}

impl LogGraph {
    /// The lanes of `list`, one row per listed commit.
    pub fn new(list: &LogList) -> LogGraph {
        let row_of = |commits: &[CommitIx]| {
            let mut rows = std::collections::HashMap::with_capacity(commits.len());
            for (i, &c) in commits.iter().enumerate() {
                rows.insert(c, i as u32);
            }
            rows
        };
        let rows = row_of(&list.commits);
        let parents: Vec<Vec<u32>> = list
            .parents
            .iter()
            .map(|ps| ps.iter().filter_map(|p| rows.get(p).copied()).collect())
            .collect();
        let mut graph = LogGraph {
            parents,
            outside: list.outside.clone(),
            checkpoints: Vec::new(),
            lanes: 0,
        };
        let mut lanes = Vec::new();
        for row in 0..graph.parents.len() {
            if row % CHECKPOINT == 0 {
                graph.checkpoints.push(lanes.clone());
            }
            let lane = graph.step(&mut lanes, row, None);
            graph.lanes = graph.lanes.max(lanes.len()).max(lane + 1);
        }
        graph
    }

    /// The number of rows.
    pub fn len(&self) -> usize {
        self.parents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parents.is_empty()
    }

    /// The rows in `range` (clamped to the log).
    pub fn rows(&self, range: Range<usize>) -> Vec<GraphRow> {
        let end = range.end.min(self.len());
        let start = range.start.min(end);
        let Some(saved) = self.checkpoints.get(start / CHECKPOINT) else {
            return Vec::new();
        };
        let mut lanes = saved.clone();
        for row in start / CHECKPOINT * CHECKPOINT..start {
            self.step(&mut lanes, row, None);
        }
        (start..end)
            .map(|row| {
                let mut out = GraphRow::default();
                self.step(&mut lanes, row, Some(&mut out));
                out
            })
            .collect()
    }

    /// Moves `lanes` from the top of `row` to its bottom, describing the row in `out` if given.
    /// Returns the row's lane.
    fn step(&self, lanes: &mut Vec<Lane>, row: usize, out: Option<&mut GraphRow>) -> usize {
        let me = Lane::Waits(row as u32);
        let top = lanes.clone();
        let lane = top
            .iter()
            .position(|&l| l == me)
            .or_else(|| lanes.iter().position(|&l| l == Lane::Free))
            .unwrap_or(lanes.len());
        if lane == lanes.len() {
            lanes.push(Lane::Free);
        }
        for l in lanes.iter_mut().filter(|l| **l == me) {
            *l = Lane::Free;
        }
        lanes[lane] = Lane::Held;
        let mut lower = Vec::new();
        let parents = &self.parents[row];
        for (j, &p) in parents.iter().enumerate() {
            let waits = Lane::Waits(p);
            let to = if let Some(k) = lanes.iter().position(|&l| l == waits) {
                k
            } else if j == 0 {
                lanes[lane] = waits;
                lane
            } else {
                let k = lanes
                    .iter()
                    .position(|&l| l == Lane::Free)
                    .unwrap_or(lanes.len());
                if k == lanes.len() {
                    lanes.push(Lane::Free);
                }
                lanes[k] = waits;
                k
            };
            lower.push(GraphLine { from: lane, to });
        }
        if lanes[lane] == Lane::Held {
            lanes[lane] = Lane::Free;
        }
        while lanes.last() == Some(&Lane::Free) {
            lanes.pop();
        }
        if let Some(out) = out {
            out.lane = lane;
            out.merge = parents.len() > 1;
            out.outside = self.outside[row];
            for (k, &l) in top.iter().enumerate() {
                match l {
                    Lane::Waits(_) if l == me => out.upper.push(GraphLine { from: k, to: lane }),
                    Lane::Waits(_) => {
                        out.upper.push(GraphLine { from: k, to: k });
                        out.lower.push(GraphLine { from: k, to: k });
                    }
                    Lane::Free | Lane::Held => {}
                }
            }
            out.lower.extend(lower);
        }
        lane
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A log of `parents` (rows of each row's parents), nothing outside it.
    fn graph(parents: &[&[u32]]) -> LogGraph {
        let commits: Vec<CommitIx> = (0..parents.len() as u32).map(CommitIx).collect();
        let list = LogList {
            parents: parents
                .iter()
                .map(|ps| ps.iter().map(|&p| CommitIx(p)).collect())
                .collect(),
            outside: vec![false; commits.len()],
            commits,
        };
        LogGraph::new(&list)
    }

    fn line(from: usize, to: usize) -> GraphLine {
        GraphLine { from, to }
    }

    #[test]
    fn a_straight_history_is_one_lane() {
        let g = graph(&[&[1], &[2], &[]]);
        assert_eq!(g.lanes, 1);
        let rows = g.rows(0..3);
        assert_eq!(rows[0].upper, []);
        assert_eq!(rows[0].lower, [line(0, 0)]);
        assert_eq!(rows[1].upper, [line(0, 0)]);
        assert_eq!(rows[2].lower, []);
    }

    /// 0 merges 1 (first parent) and 2, which both come from 3.
    #[test]
    fn a_merge_opens_a_lane_that_joins_the_fork_point() {
        let g = graph(&[&[1, 2], &[3], &[3], &[]]);
        assert_eq!(g.lanes, 2);
        let rows = g.rows(0..4);
        assert!(rows[0].merge);
        assert_eq!(rows[0].lower, [line(0, 0), line(0, 1)]);
        // 1 in lane 0; lane 1 passes it by.
        assert_eq!(
            (rows[1].lane, &rows[1].upper),
            (0, &vec![line(0, 0), line(1, 1)])
        );
        // 2 in lane 1 continues towards 3, which lane 0 already waits for.
        assert_eq!(rows[2].lane, 1);
        assert_eq!(rows[2].lower, [line(0, 0), line(1, 0)]);
        // So 3 has one line coming in.
        assert_eq!((rows[3].lane, &rows[3].upper), (0, &vec![line(0, 0)]));
    }

    #[test]
    fn two_tips_take_two_lanes_and_a_freed_lane_is_reused() {
        // 0 and 1 are tips; both lead to 2. Then 3 is a new tip, 4 its root.
        let g = graph(&[&[2], &[2], &[], &[4], &[]]);
        let rows = g.rows(0..5);
        assert_eq!(rows[1].lane, 1);
        assert_eq!(rows[1].upper, [line(0, 0)]);
        assert_eq!(rows[3].lane, 0);
        assert_eq!(g.lanes, 2);
    }

    #[test]
    fn rows_from_the_middle_match_rows_from_the_top() {
        // Long enough to need checkpoints: a history with a side branch every few commits.
        let mut parents: Vec<Vec<u32>> = Vec::new();
        for i in 0..500u32 {
            parents.push(match i % 7 {
                0 => vec![i + 1, i + 3],
                _ if i + 1 < 500 => vec![i + 1],
                _ => vec![],
            });
        }
        let refs: Vec<&[u32]> = parents.iter().map(Vec::as_slice).collect();
        let g = graph(&refs);
        let all = g.rows(0..g.len());
        for start in [0, 63, 64, 65, 200, 499] {
            assert_eq!(g.rows(start..start + 3), all[start..(start + 3).min(500)]);
        }
    }
}
