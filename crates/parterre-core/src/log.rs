//! The log query: which commits the log window lists, and in what order.
//!
//! A [`LogQuery`] is the tips to walk back from plus the commits to leave out (with everything
//! they reach). [`LogQuery::run`] turns the in-memory [`Repo`] snapshot into the ordered list
//! without asking git again. It knows nothing about the revision graph.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use serde::{Deserialize, Serialize};

use crate::repo::{Commit, CommitIx, GitRef, RefKind, Repo};

/// What the log lists: the commits reachable from any of `tips` but from none of `exclude`,
/// like `git log <tips> ^<exclude>`.
///
/// Built with [`LogQuery::commit`] or [`LogQuery::range`]. It is `non_exhaustive` so that
/// future filters (date range, author, text) can become new fields, defaulting to "no
/// filter", without breaking callers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct LogQuery {
    /// Where the walk starts. Their ancestors are listed too.
    pub tips: Vec<CommitIx>,
    /// Commits left out together with all their ancestors.
    pub exclude: Vec<CommitIx>,
}

impl LogQuery {
    /// One node: the commit and all its ancestors, like `git log <commit>`.
    pub fn commit(commit: CommitIx) -> LogQuery {
        LogQuery {
            tips: vec![commit],
            exclude: Vec::new(),
        }
    }

    /// Two nodes, in selection order: `first..second`, the commits reachable from `second` but
    /// not from `first`, as TortoiseGit does. Unrelated histories give all of `second`'s
    /// history; `first` itself is never listed.
    ///
    /// Deliberate deviation from TortoiseGit (decided in #28): when `second` is an ancestor of
    /// `first`, where TortoiseGit shows an empty list, the two are swapped so the commits in
    /// between are shown. Diverged pairs keep the selection order.
    ///
    /// The range label (`first..second`) reads `exclude[0]..tips[0]` of the result, which
    /// reflects the swap.
    pub fn range(repo: &Repo, first: CommitIx, second: CommitIx) -> LogQuery {
        let (first, second) = if first != second && is_ancestor(repo, second, first) {
            (second, first)
        } else {
            (first, second)
        };
        LogQuery {
            tips: vec![second],
            exclude: vec![first],
        }
    }

    /// The log for selected nodes' commits, in selection order: one gives [`LogQuery::commit`],
    /// two give [`LogQuery::range`], none or three and more give nothing (decided in #28).
    pub fn for_selection(repo: &Repo, selected: &[CommitIx]) -> Option<LogQuery> {
        match *selected {
            [one] => Some(LogQuery::commit(one)),
            [first, second] => Some(LogQuery::range(repo, first, second)),
            _ => None,
        }
    }

    /// What the query shows, as TortoiseGit labels it: `main` for one tip, `feature/x..main`
    /// for a range. A commit is named by its first ref that `show` accepts, else by its short
    /// hash. `refs` is [`Repo::refs_by_commit`].
    pub fn label(
        &self,
        repo: &Repo,
        refs: &[Vec<usize>],
        show: impl Fn(&GitRef) -> bool,
    ) -> LogLabel {
        let name = |c: CommitIx| commit_name(repo, refs, c, &show);
        LogLabel {
            from: self.exclude.first().map(|&c| name(c)),
            to: self.tips.first().map(|&c| name(c)).unwrap_or_default(),
        }
    }

    /// The commits the query selects, newest first by committer date, never a parent before
    /// any of its children: the order of `git log --date-order`.
    pub fn run(&self, repo: &Repo) -> Vec<CommitIx> {
        let options = LogOptions {
            order: LogOrder::Date,
            ..LogOptions::default()
        };
        self.list(repo, &options).commits
    }

    /// The commits the query selects under `options`, in their order, each with the parents
    /// the log shows for it.
    ///
    /// Runs in O(n log n) over the commits reachable from the tips, with a few flat arrays the
    /// size of the snapshot.
    pub fn list(&self, repo: &Repo, options: &LogOptions) -> LogList {
        let n = repo.commits.len();
        let walk_parents = |c: CommitIx| -> &[CommitIx] {
            let parents = &repo.commit(c).parents;
            if options.first_parent {
                &parents[..parents.len().min(1)]
            } else {
                parents
            }
        };
        let mut state = vec![State::Unseen; n];
        // Everything reachable from the exclusions is out.
        let mut stack: Vec<CommitIx> = Vec::new();
        for &c in &self.exclude {
            if state[c.ix()] == State::Unseen {
                state[c.ix()] = State::Excluded;
                stack.push(c);
            }
        }
        while let Some(c) = stack.pop() {
            for &p in &repo.commit(c).parents {
                if state[p.ix()] == State::Unseen {
                    state[p.ix()] = State::Excluded;
                    stack.push(p);
                }
            }
        }

        // Walk from the tips, counting for every selected commit its selected children.
        let mut tips = self.tips.clone();
        if options.all_branches {
            let branch_or_tag = |r: &&GitRef| {
                matches!(
                    r.kind,
                    RefKind::LocalBranch | RefKind::RemoteBranch | RefKind::Tag
                )
            };
            tips.extend(repo.refs.iter().filter(branch_or_tag).map(|r| r.target));
            tips.extend(repo.head_commit());
        }
        let mut children = vec![0u32; n];
        let mut selected = 0usize;
        for &c in &tips {
            if state[c.ix()] == State::Unseen {
                state[c.ix()] = State::Selected;
                selected += 1;
                stack.push(c);
            }
        }
        while let Some(c) = stack.pop() {
            for &p in walk_parents(c) {
                match state[p.ix()] {
                    State::Excluded => {}
                    State::Selected => children[p.ix()] += 1,
                    State::Unseen => {
                        state[p.ix()] = State::Selected;
                        selected += 1;
                        children[p.ix()] += 1;
                        stack.push(p);
                    }
                }
            }
        }
        let fork = |c: CommitIx, children: &[u32]| children[c.ix()] > 1;
        let forks: Vec<bool> = if options.branchings_only {
            (0..n)
                .map(|c| fork(CommitIx(c as u32), &children))
                .collect()
        } else {
            Vec::new()
        };

        // Initial candidates in snapshot order, which is git's own date order.
        tips.retain(|c| state[c.ix()] == State::Selected && children[c.ix()] == 0);
        tips.sort_unstable();
        tips.dedup();
        let mut order = Vec::with_capacity(selected);
        match options.order {
            LogOrder::Date => {
                // git's topological sort by commit date (`sort_in_topological_order`): a commit
                // becomes ready once all its children are out, and the newest ready commit goes
                // next. Ties go to the commit that became ready first, as in git's priority
                // queue.
                let mut ready = BinaryHeap::new();
                let mut seq = 0u32;
                let mut push = |ready: &mut BinaryHeap<Ready>, c: CommitIx| {
                    ready.push(Ready {
                        time: repo.commit(c).commit_time,
                        seq: Reverse(seq),
                        commit: c,
                    });
                    seq += 1;
                };
                for &c in &tips {
                    push(&mut ready, c);
                }
                while let Some(Ready { commit, .. }) = ready.pop() {
                    order.push(commit);
                    for &p in walk_parents(commit) {
                        if state[p.ix()] == State::Selected {
                            children[p.ix()] -= 1;
                            if children[p.ix()] == 0 {
                                push(&mut ready, p);
                            }
                        }
                    }
                }
            }
            LogOrder::Topological => {
                // git's `--topo-order`: the same sort with a stack for a queue, so a commit's
                // ancestors follow it until they reach one that has children still to come.
                // The tips start in date order, and a merge's last parent comes out first.
                let mut ready: Vec<CommitIx> = tips.iter().rev().copied().collect();
                while let Some(commit) = ready.pop() {
                    order.push(commit);
                    for &p in walk_parents(commit) {
                        if state[p.ix()] == State::Selected {
                            children[p.ix()] -= 1;
                            if children[p.ix()] == 0 {
                                ready.push(p);
                            }
                        }
                    }
                }
            }
        }
        debug_assert_eq!(order.len(), selected);

        // Which commits are listed, and the listed commits each one stands for: itself, or
        // for one left out, what its parents stand for. Parents come after their children, so
        // going backwards meets every parent first.
        let refs = options.branchings_only.then(|| repo.refs_by_commit());
        let head = repo.head_commit();
        let listed = |c: CommitIx| {
            let parents = walk_parents(c);
            (!options.no_merges || repo.commit(c).parents.len() < 2)
                && (!options.branchings_only
                    || parents.len() != 1
                    || forks[c.ix()]
                    || Some(c) == head
                    || self.tips.contains(&c)
                    || refs.as_ref().is_some_and(|refs| !refs[c.ix()].is_empty()))
        };
        let mut stands_for: Vec<Vec<CommitIx>> = vec![Vec::new(); n];
        let mut beyond = vec![false; n];
        let mut out = LogList::default();
        for &c in order.iter().rev() {
            let mut parents = Vec::new();
            let mut outside = false;
            for &p in walk_parents(c) {
                if state[p.ix()] == State::Selected {
                    for &q in &stands_for[p.ix()] {
                        if !parents.contains(&q) {
                            parents.push(q);
                        }
                    }
                    outside |= beyond[p.ix()];
                } else {
                    outside = true;
                }
            }
            if listed(c) {
                stands_for[c.ix()] = vec![c];
                out.commits.push(c);
                out.parents.push(parents);
                out.outside.push(outside);
            } else {
                stands_for[c.ix()] = parents;
                beyond[c.ix()] = outside;
            }
        }
        out.commits.reverse();
        out.parents.reverse();
        out.outside.reverse();
        out
    }
}

/// Which commits the log walks and how it orders them: TortoiseGit's All Branches and walk
/// behaviour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct LogOptions {
    /// Also walk from every branch, remote branch and tag, and from HEAD (`git log --branches
    /// --remotes --tags HEAD`); a range's exclusions still apply.
    pub all_branches: bool,
    /// Follow only first parents (`--first-parent`).
    pub first_parent: bool,
    /// Leave out merges (`--no-merges`).
    pub no_merges: bool,
    /// List only the commits where history branches or joins, and those with refs: forks,
    /// merges, roots and tips (the revision graph's "Branchings and merges").
    pub branchings_only: bool,
    pub order: LogOrder,
}

/// The order of the log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LogOrder {
    /// `git log --topo-order`, TortoiseGit's default: a branch's commits stay together.
    #[default]
    Topological,
    /// `git log --date-order`: newest first, never a parent before its children.
    Date,
}

/// The result of [`LogQuery::list`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogList {
    /// The listed commits, in order.
    pub commits: Vec<CommitIx>,
    /// For each listed commit, the parents the log shows: its parents in the walk, where one
    /// that is left out (a merge with [`LogOptions::no_merges`], say) is replaced by the
    /// listed commits it stands for. Always listed below the commit.
    pub parents: Vec<Vec<CommitIx>>,
    /// For each listed commit, true if some of its history is outside the log: a parent in a
    /// range's excluded part.
    pub outside: Vec<bool>,
}

/// The label of a [`LogQuery`]: `to` alone, or `from..to` for a range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLabel {
    /// The excluded end of a range.
    pub from: Option<String>,
    pub to: String,
}

impl std::fmt::Display for LogLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.from {
            Some(from) => write!(f, "{from}..{}", self.to),
            None => f.write_str(&self.to),
        }
    }
}

/// A commit's name in labels: its first ref that `show` accepts, else its short hash. `refs`
/// is [`Repo::refs_by_commit`].
pub fn commit_name(
    repo: &Repo,
    refs: &[Vec<usize>],
    commit: CommitIx,
    show: impl Fn(&GitRef) -> bool,
) -> String {
    refs[commit.ix()]
        .iter()
        .map(|&r| &repo.refs[r])
        .find(|r| show(r))
        .map_or_else(
            || repo.commit(commit).oid.short(repo.abbrev_len),
            |r| r.name.clone(),
        )
}

/// True if `ancestor` is reachable from `descendant` through parent links (a commit counts as
/// its own ancestor, as in `git merge-base --is-ancestor`).
pub fn is_ancestor(repo: &Repo, ancestor: CommitIx, descendant: CommitIx) -> bool {
    if ancestor == descendant {
        return true;
    }
    // Clocks skew, so walk everything reachable rather than pruning by commit date.
    let mut seen = vec![false; repo.commits.len()];
    let mut stack = vec![descendant];
    seen[descendant.ix()] = true;
    while let Some(c) = stack.pop() {
        for &p in &repo.commit(c).parents {
            if p == ancestor {
                return true;
            }
            if !seen[p.ix()] {
                seen[p.ix()] = true;
                stack.push(p);
            }
        }
    }
    false
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Unseen,
    Excluded,
    Selected,
}

/// A commit whose children have all been listed. Max-heap order: newest first, then earliest
/// to become ready.
#[derive(PartialEq, Eq)]
struct Ready {
    time: i64,
    seq: Reverse<u32>,
    commit: CommitIx,
}

impl Ord for Ready {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.time, self.seq).cmp(&(other.time, other.seq))
    }
}

impl PartialOrd for Ready {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The fewest hex digits a find query needs to match the start of a hash, as git's shortest
/// abbreviation; fewer would match a sixteenth of the commits a digit.
pub const FIND_HASH_MIN: usize = 4;

/// Where a find query occurs in a commit's row of the log ([`find_in`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// How many hex digits of the hash, from its start, the query is.
    pub hash: usize,
    /// Where in the subject, as byte ranges ([`crate::find::find`]).
    pub subject: Vec<std::ops::Range<usize>>,
}

impl Found {
    pub fn is_empty(&self) -> bool {
        self.hash == 0 && self.subject.is_empty()
    }
}

/// Where `query` occurs in `commit`: the start of its hash, if the query (blanks around it
/// left out) is at least [`FIND_HASH_MIN`] hex digits, in either case; and every place in its
/// subject, ignoring case, as the other finds do.
pub fn find_in(commit: &Commit, query: &str) -> Found {
    let hex = query.trim().to_ascii_lowercase();
    let hash = if hex.len() >= FIND_HASH_MIN
        && hex.bytes().all(|b| b.is_ascii_hexdigit())
        && commit.oid.to_hex().starts_with(&hex)
    {
        hex.len()
    } else {
        0
    };
    let subject = if query.trim().is_empty() {
        Vec::new()
    } else {
        crate::find::find([&commit.subject], query)
            .into_iter()
            .map(|m| m.range)
            .collect()
    };
    Found { hash, subject }
}

/// The rows of a log's `commits` that `query` occurs in ([`find_in`]).
pub fn find(repo: &Repo, commits: &[CommitIx], query: &str) -> Vec<usize> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    commits
        .iter()
        .enumerate()
        .filter(|&(_, &c)| !find_in(repo.commit(c), query).is_empty())
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oid::Oid;
    use crate::repo::{Commit, Head};

    /// A repository from `(commit_time, parents)` pairs; commit `i` gets index `i`.
    fn repo(spec: &[(i64, &[u32])]) -> Repo {
        let commits = spec
            .iter()
            .enumerate()
            .map(|(i, &(time, parents))| Commit {
                oid: Oid::from_hex(&format!("{i:040x}")).unwrap(),
                parents: parents.iter().map(|&p| CommitIx(p)).collect(),
                truncated: false,
                empty_tree: false,
                author_name: String::new(),
                author_email: String::new(),
                author_time: time,
                author_date: String::new(),
                commit_time: time,
                subject: format!("c{i}"),
            })
            .collect();
        Repo::new(
            "/x".into(),
            commits,
            Vec::new(),
            Head::Detached(CommitIx(0)),
        )
    }

    fn ixs(v: &[u32]) -> Vec<CommitIx> {
        v.iter().map(|&i| CommitIx(i)).collect()
    }

    /// 0 is a merge of 1 (first parent) and 2; both come from 3.
    ///   0
    ///  / \
    /// 1   2
    ///  \ /
    ///   3
    fn diamond(t1: i64, t2: i64) -> Repo {
        repo(&[(10, &[1, 2]), (t1, &[3]), (t2, &[3]), (1, &[])])
    }

    #[test]
    fn one_node_lists_the_commit_and_its_ancestors_newest_first() {
        let r = diamond(5, 7);
        assert_eq!(LogQuery::commit(CommitIx(0)).run(&r), ixs(&[0, 2, 1, 3]));
        assert_eq!(LogQuery::commit(CommitIx(1)).run(&r), ixs(&[1, 3]));
    }

    #[test]
    fn a_parent_never_comes_before_its_child_despite_clock_skew() {
        // The parent 1 claims to be newer than its child 0 and than 2.
        let r = repo(&[(5, &[1]), (100, &[3]), (7, &[3]), (1, &[])]);
        let q = LogQuery {
            tips: ixs(&[0, 2]),
            exclude: Vec::new(),
        };
        assert_eq!(q.run(&r), ixs(&[2, 0, 1, 3]));
    }

    #[test]
    fn equal_times_keep_the_order_commits_became_ready() {
        let r = diamond(5, 5);
        // 1 (first parent) becomes ready before 2.
        assert_eq!(LogQuery::commit(CommitIx(0)).run(&r), ixs(&[0, 1, 2, 3]));
    }

    #[test]
    fn range_excludes_what_first_reaches() {
        // 0 - 1 - 2 - 3 (3 is the root)
        let r = repo(&[(4, &[1]), (3, &[2]), (2, &[3]), (1, &[])]);
        let q = LogQuery::range(&r, CommitIx(2), CommitIx(0));
        assert_eq!((q.tips.clone(), q.exclude.clone()), (ixs(&[0]), ixs(&[2])));
        assert_eq!(q.run(&r), ixs(&[0, 1]));
    }

    #[test]
    fn range_swaps_when_second_is_an_ancestor_of_first() {
        let r = repo(&[(4, &[1]), (3, &[2]), (2, &[3]), (1, &[])]);
        let q = LogQuery::range(&r, CommitIx(0), CommitIx(2));
        assert_eq!((q.tips.clone(), q.exclude.clone()), (ixs(&[0]), ixs(&[2])));
        assert_eq!(q.run(&r), ixs(&[0, 1]));
    }

    #[test]
    fn range_keeps_diverged_pairs_in_selection_order() {
        let r = diamond(5, 7);
        let q = LogQuery::range(&r, CommitIx(1), CommitIx(2));
        assert_eq!(q.run(&r), ixs(&[2]));
        let q = LogQuery::range(&r, CommitIx(2), CommitIx(1));
        assert_eq!(q.run(&r), ixs(&[1]));
    }

    #[test]
    fn range_of_unrelated_histories_is_all_of_seconds_history() {
        // Two roots: 0 - 1 and 2 - 3.
        let r = repo(&[(4, &[1]), (3, &[]), (2, &[3]), (1, &[])]);
        assert_eq!(
            LogQuery::range(&r, CommitIx(0), CommitIx(2)).run(&r),
            ixs(&[2, 3])
        );
    }

    #[test]
    fn range_of_a_node_with_itself_is_empty() {
        let r = diamond(5, 7);
        assert!(
            LogQuery::range(&r, CommitIx(1), CommitIx(1))
                .run(&r)
                .is_empty()
        );
    }

    #[test]
    fn selection_maps_to_a_query() {
        let r = repo(&[(4, &[1]), (3, &[2]), (2, &[3]), (1, &[])]);
        assert_eq!(LogQuery::for_selection(&r, &[]), None);
        assert_eq!(
            LogQuery::for_selection(&r, &ixs(&[1])),
            Some(LogQuery::commit(CommitIx(1)))
        );
        // Selection order is `first..second`, including the ancestor swap.
        assert_eq!(
            LogQuery::for_selection(&r, &ixs(&[2, 0])),
            Some(LogQuery::range(&r, CommitIx(2), CommitIx(0)))
        );
        assert_eq!(
            LogQuery::for_selection(&r, &ixs(&[0, 2])),
            LogQuery::for_selection(&r, &ixs(&[2, 0]))
        );
        assert_eq!(LogQuery::for_selection(&r, &ixs(&[0, 1, 2])), None);
    }

    #[test]
    fn labels_use_ref_names_else_short_hashes() {
        use crate::repo::{GitRef, RefKind};
        let mut r = repo(&[(4, &[1]), (3, &[2]), (2, &[3]), (1, &[])]);
        let git_ref = |full: &str, name: &str, kind, target| GitRef {
            full_name: full.into(),
            name: name.into(),
            kind,
            target: CommitIx(target),
            annotated: false,
            is_head: false,
        };
        r.refs = vec![
            git_ref("refs/tags/v1", "v1", RefKind::Tag, 0),
            git_ref("refs/heads/main", "main", RefKind::LocalBranch, 0),
            git_ref("refs/pull/1", "pull/1", RefKind::Other, 2),
        ];
        r.abbrev_len = 9;
        let refs = r.refs_by_commit();
        let all = |_: &GitRef| true;
        // Heads sort before tags.
        let one = LogQuery::commit(CommitIx(0)).label(&r, &refs, all);
        assert_eq!(one.to_string(), "main");
        let range = LogQuery::range(&r, CommitIx(3), CommitIx(0)).label(&r, &refs, all);
        assert_eq!(range.to_string(), format!("{}..main", "0".repeat(9)));
        let pull = LogQuery::range(&r, CommitIx(2), CommitIx(0));
        assert_eq!(pull.label(&r, &refs, all).to_string(), "pull/1..main");
        let hidden = pull.label(&r, &refs, |g: &GitRef| g.kind != RefKind::Other);
        assert_eq!(
            hidden,
            LogLabel {
                from: Some(r.commit(CommitIx(2)).oid.short(9)),
                to: "main".into()
            }
        );
    }

    #[test]
    fn ancestry() {
        let r = diamond(5, 7);
        assert!(is_ancestor(&r, CommitIx(3), CommitIx(0)));
        assert!(is_ancestor(&r, CommitIx(2), CommitIx(0)));
        assert!(!is_ancestor(&r, CommitIx(0), CommitIx(3)));
        assert!(!is_ancestor(&r, CommitIx(1), CommitIx(2)));
    }

    #[test]
    fn large_linear_history_is_fast_and_complete() {
        let n = 100_000u32;
        let parents: Vec<[u32; 1]> = (0..n).map(|i| [i + 1]).collect();
        let spec: Vec<(i64, &[u32])> = (0..n)
            .map(|i| {
                let p: &[u32] = if i + 1 < n { &parents[i as usize] } else { &[] };
                (i64::from(n - i), p)
            })
            .collect();
        let r = repo(&spec);
        let start = std::time::Instant::now();
        let out = LogQuery::commit(CommitIx(0)).run(&r);
        let took = start.elapsed();
        assert_eq!(out.len(), n as usize);
        assert!(out.windows(2).all(|w| w[0].0 + 1 == w[1].0));
        // Generous: debug builds on slow CI. Release takes a few milliseconds.
        assert!(took.as_secs() < 5, "took {took:?}");
    }

    #[test]
    fn finds_in_subjects_and_hash_prefixes() {
        let commit = |hex: &str, subject: &str| Commit {
            oid: Oid::from_hex(&format!("{hex:0<40}")).unwrap(),
            parents: Vec::new(),
            truncated: false,
            empty_tree: false,
            author_name: "Deadbeef Dan".into(),
            author_email: String::new(),
            author_time: 0,
            author_date: String::new(),
            commit_time: 0,
            subject: subject.into(),
        };
        let commits = vec![
            commit("deadbeef", "Fix the Parser"),
            commit("0123abcd", "parser: parse, parse again"),
            commit("abcdef01", "Add dead code"),
        ];
        let repo = Repo::new(
            "/x".into(),
            commits,
            Vec::new(),
            Head::Detached(CommitIx(0)),
        );
        let rows = ixs(&[0, 1, 2]);

        // The subject anywhere, ignoring case, every place in it.
        assert_eq!(find(&repo, &rows, "PARSE"), [0, 1]);
        let places = find_in(repo.commit(CommitIx(1)), "parse");
        assert_eq!(places.hash, 0);
        assert_eq!(places.subject, [0..5, 8..13, 15..20]);

        // A hash by its start, from 4 hex digits, ignoring case and blanks around it.
        assert_eq!(find(&repo, &rows, " DEADB "), [0]);
        assert_eq!(find_in(repo.commit(CommitIx(0)), "DEADB").hash, 5);
        assert_eq!(find(&repo, &rows, "0123"), [1]);
        // Shorter, or not at the start: only the subjects count ("dead" is in commit 2's).
        assert_eq!(find(&repo, &rows, "dead"), [0, 2]);
        assert_eq!(find_in(repo.commit(CommitIx(0)), "dead").hash, 4);
        assert_eq!(find(&repo, &rows, "abcd"), [2]);
        assert_eq!(find(&repo, &rows, "dea"), [2]);
        // Not the author.
        assert!(find(&repo, &rows, "Dan").is_empty());
        assert!(find(&repo, &rows, "").is_empty());
        assert!(find(&repo, &rows, "  ").is_empty());
    }
}
