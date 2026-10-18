//! The log walks the [`Repo`] snapshot rather than ask git again.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use serde::{Deserialize, Serialize};

use crate::repo::{Commit, CommitIx, GitRef, RefKind, Repo};

/// As `git log <tips> ^<exclude>`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct LogQuery {
    pub tips: Vec<CommitIx>,
    pub exclude: Vec<CommitIx>,
}

impl LogQuery {
    pub fn commit(commit: CommitIx) -> LogQuery {
        LogQuery {
            tips: vec![commit],
            exclude: Vec::new(),
        }
    }

    /// `first..second`, as TortoiseGit does, except when `second` is an ancestor of `first`:
    /// rather than nothing, the two are swapped to show the commits between (#28).
    pub fn range(repo: &Repo, first: CommitIx, second: CommitIx) -> LogQuery {
        let (first, second) = ancestor_first(repo, first, second);
        LogQuery {
            tips: vec![second],
            exclude: vec![first],
        }
    }

    /// `selected` must be in selection order, which sets a range's direction (#28).
    pub fn for_selection(repo: &Repo, selected: &[CommitIx]) -> Option<LogQuery> {
        match *selected {
            [one] => Some(LogQuery::commit(one)),
            [first, second] => Some(LogQuery::range(repo, first, second)),
            _ => None,
        }
    }

    /// `refs` is [`Repo::refs_by_commit`].
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

    /// In `git log --date-order` order.
    pub fn run(&self, repo: &Repo) -> Vec<CommitIx> {
        let options = LogOptions {
            order: LogOrder::Date,
            ..LogOptions::default()
        };
        self.list(repo, &options).commits
    }

    /// O(n log n) in the commits reachable from the tips.
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
        // Before the sort below counts `children` down.
        let forks: Vec<bool> = if options.branchings_only {
            children.iter().map(|&count| count > 1).collect()
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct LogOptions {
    /// `git log --branches --remotes --tags HEAD`; a range's exclusions still apply.
    pub all_branches: bool,
    pub first_parent: bool,
    pub no_merges: bool,
    /// Only forks, merges, roots, and commits with refs, as the revision graph's "Branchings
    /// and merges".
    pub branchings_only: bool,
    pub order: LogOrder,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LogOrder {
    /// TortoiseGit's default.
    #[default]
    Topological,
    Date,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogList {
    pub commits: Vec<CommitIx>,
    /// Per commit, its parents in the walk, where one left out (a merge with
    /// [`LogOptions::no_merges`], say) is replaced by the listed commits it stands for. Always
    /// listed below the commit.
    pub parents: Vec<Vec<CommitIx>>,
    /// Per commit, whether a parent is outside the log, in a range's excluded part.
    pub outside: Vec<bool>,
}

/// `to`, or `from..to` for a range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLabel {
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

/// The commit's first ref that `show` accepts, else its short hash. `refs` is
/// [`Repo::refs_by_commit`].
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

/// A commit is its own ancestor, as in `git merge-base --is-ancestor`.
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

/// `first` and `second` in that order, unless `second` is an ancestor of `first`.
pub(crate) fn ancestor_first(
    repo: &Repo,
    first: CommitIx,
    second: CommitIx,
) -> (CommitIx, CommitIx) {
    if is_ancestor(repo, second, first) {
        (second, first)
    } else {
        (first, second)
    }
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

/// git's shortest abbreviation: fewer digits would match too many commits.
const MIN_HASH_QUERY_DIGITS: usize = 4;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// How many of the hash's first digits the query matches; 0 for none.
    pub hash_digits: usize,
    /// Byte ranges.
    pub subject: Vec<std::ops::Range<usize>>,
}

impl Found {
    pub fn is_empty(&self) -> bool {
        self.hash_digits == 0 && self.subject.is_empty()
    }
}

/// The query, trimmed, matches the hash's start from `MIN_HASH_QUERY_DIGITS` digits on, and
/// the subject anywhere.
pub fn find_in(commit: &Commit, query: &str) -> Found {
    let hex = query.trim().to_ascii_lowercase();
    let hash_digits = if hex.len() >= MIN_HASH_QUERY_DIGITS
        && hex.bytes().all(|b| b.is_ascii_hexdigit())
        && commit.oid.to_hex().starts_with(&hex)
    {
        hex.len()
    } else {
        0
    };
    let subject = if hex.is_empty() {
        Vec::new()
    } else {
        crate::find::find([&commit.subject], query)
            .into_iter()
            .map(|m| m.range)
            .collect()
    };
    Found {
        hash_digits,
        subject,
    }
}

/// The rows of `commits` that `query` occurs in.
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
        assert_eq!(places.hash_digits, 0);
        assert_eq!(places.subject, [0..5, 8..13, 15..20]);

        // A hash by its start, from 4 hex digits, ignoring case and blanks around it.
        assert_eq!(find(&repo, &rows, " DEADB "), [0]);
        assert_eq!(find_in(repo.commit(CommitIx(0)), "DEADB").hash_digits, 5);
        assert_eq!(find(&repo, &rows, "0123"), [1]);
        // Shorter, or not at the start: only the subjects count ("dead" is in commit 2's).
        assert_eq!(find(&repo, &rows, "dead"), [0, 2]);
        assert_eq!(find_in(repo.commit(CommitIx(0)), "dead").hash_digits, 4);
        assert_eq!(find(&repo, &rows, "abcd"), [2]);
        assert_eq!(find(&repo, &rows, "dea"), [2]);
        // Not the author.
        assert!(find(&repo, &rows, "Dan").is_empty());
        assert!(find(&repo, &rows, "").is_empty());
        assert!(find(&repo, &rows, "  ").is_empty());
    }
}
