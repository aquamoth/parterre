//! Local branches whose pull request was squash- or rebase-merged (#384). Merge commits and
//! fast-forwards need nothing, as the graph shows where they landed; these leave the branch's
//! tip off the default branch, with its content on it. Only branches whose upstream is gone
//! are checked: a forge deletes the branch once merged, and a fetch prunes it.
//!
//! Found here from git alone, by patch: a squash merge is a commit on the default branch with
//! the change of the whole branch (`git diff <merge-base> <tip> | git patch-id --stable`), and
//! a rebase merge has a copy of each of the branch's commits there (as `rev-list --cherry-mark`
//! finds them). Either way it landed after the branch forked. Every branch is checked with the
//! same four git starts. The forge finds what this misses: conflicts resolved, or edits made,
//! as the pull request was completed (`parterre-forge`).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::git::{Git, GitError};
use crate::oid::Oid;
use crate::repo::{CommitIx, Repo};

/// How a branch's content came onto the default branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum How {
    /// As one commit with the change of the whole branch. A branch of one commit rebased looks
    /// the same, and is called this.
    Squash,
    /// As a copy of each of its commits.
    Rebase,
}

/// A local branch merged by squash or rebase.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Merged {
    /// `None` when only the forge says so, and not how.
    pub how: Option<How>,
    /// The commit its content landed as: the squash commit, or the copy of the branch's tip.
    pub landed: Option<Oid>,
    /// Its pull request's number, as the forge has it.
    pub pull_request: Option<u64>,
}

impl Merged {
    /// What the git check and the forge found, together: a branch is merged if either says so.
    /// The forge's commit is the one the pull request was completed as; the check's is found
    /// by patch.
    pub fn either(local: Option<Merged>, forge: Option<Merged>) -> Option<Merged> {
        match (local, forge) {
            (Some(local), Some(forge)) => Some(Merged {
                how: local.how.or(forge.how),
                landed: forge.landed.or(local.landed),
                pull_request: forge.pull_request,
            }),
            (local, forge) => local.or(forge),
        }
    }

    /// Where it landed, as the tooltip and the deletion warning say: `squash-merged as
    /// abc1234 in PR #123`, `rebase-merged into origin/main`.
    pub fn describe(&self, repo: &Repo) -> String {
        let short = |oid: Oid| oid.short(repo.abbrev_len);
        let into = repo.default_branch.as_deref().map(short_name);
        let mut text = match (self.how, self.landed, into) {
            (Some(How::Rebase), _, Some(into)) => format!("rebase-merged into {into}"),
            (Some(How::Rebase), Some(landed), None) => {
                format!("rebase-merged as {}", short(landed))
            }
            (Some(How::Squash), Some(landed), _) => format!("squash-merged as {}", short(landed)),
            (None, Some(landed), _) => format!("merged as {}", short(landed)),
            (Some(How::Squash), None, _) => "squash-merged".to_owned(),
            (Some(How::Rebase), None, None) => "rebase-merged".to_owned(),
            (None, None, _) => "merged".to_owned(),
        };
        if let Some(number) = self.pull_request {
            text.push_str(&format!(" in PR #{number}"));
        }
        text
    }
}

/// `origin/main` of `refs/remotes/origin/main`.
fn short_name(full_name: &str) -> &str {
    full_name.strip_prefix("refs/remotes/").unwrap_or(full_name)
}

/// The answers for local branches, by full name, each about the tip it was asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergedBranches {
    by_branch: HashMap<String, (Oid, Merged)>,
}

impl MergedBranches {
    pub fn insert(&mut self, full_name: &str, tip: Oid, merged: Merged) {
        self.by_branch.insert(full_name.to_owned(), (tip, merged));
    }

    pub fn is_empty(&self) -> bool {
        self.by_branch.is_empty()
    }

    /// The ref `branch` of `repo`'s answer (an index into [`Repo::refs`]), if it is about
    /// where the branch is now.
    pub fn of(&self, repo: &Repo, branch: usize) -> Option<&Merged> {
        let r = repo.refs.get(branch)?;
        self.of_branch(&r.full_name, repo.commit(r.target).oid)
    }

    /// The answer for the local branch `full_name` at `tip`.
    pub fn of_branch(&self, full_name: &str, tip: Oid) -> Option<&Merged> {
        let (asked, merged) = self.by_branch.get(full_name)?;
        (*asked == tip).then_some(merged)
    }

    /// The commits the branches of `repo` landed as, where it has them: the graph keeps them
    /// as nodes.
    pub fn landed(&self, repo: &Repo) -> Vec<CommitIx> {
        let mut landed: Vec<CommitIx> = (0..repo.refs.len())
            .filter_map(|b| repo.lookup(&self.of(repo, b)?.landed?))
            .collect();
        landed.sort_unstable();
        landed.dedup();
        landed
    }
}

/// The local branches to check, as indices into [`Repo::refs`]: their upstream is gone and
/// the default branch doesn't reach their tip. The newest tips (by committer date) first.
pub fn candidates(repo: &Repo) -> Vec<usize> {
    let Some(default) = default_tip(repo) else {
        return Vec::new();
    };
    let mut branches: Vec<usize> = repo
        .upstreams
        .iter()
        .filter(|u| u.is_gone())
        .map(|u| u.branch)
        .filter(|&b| !repo.reaches(default, repo.refs[b].target))
        .collect();
    branches.sort_by_key(|&b| {
        let c = repo.refs[b].target;
        (std::cmp::Reverse(repo.commit(c).commit_time), c)
    });
    branches
}

fn default_tip(repo: &Repo) -> Option<CommitIx> {
    let name = repo.default_branch.as_deref()?;
    repo.refs
        .iter()
        .find(|r| r.full_name == name)
        .map(|r| r.target)
}

/// One branch, as the check sees it.
struct Branch {
    tip: CommitIx,
    merge_base: CommitIx,
    /// Its non-merge commits the default branch lacks, newest first.
    own: Vec<CommitIx>,
}

/// From git alone: for each of `branches` (indices into [`Repo::refs`], as [`candidates`]
/// gives them), how it was merged into the default branch, if it was. Blocks: run it on a
/// worker thread.
pub fn find(git: &Git, repo: &Repo, branches: &[usize]) -> Result<Vec<Option<Merged>>, GitError> {
    let Some(default) = default_tip(repo) else {
        return Ok(vec![None; branches.len()]);
    };
    let generation = repo.generations();
    let checked: Vec<Option<Branch>> = branches
        .iter()
        .map(|&b| {
            let tip = repo.refs[b].target;
            let (own, _) = crate::upstream::difference(repo, generation, tip, default);
            let own_set: HashSet<CommitIx> = own.iter().copied().collect();
            // The newest commit both reach.
            let merge_base = own
                .iter()
                .flat_map(|&c| repo.commit(c).parents.iter().copied())
                .filter(|p| !own_set.contains(p))
                .max_by_key(|&p| (generation[p.ix()], p))?;
            let own = own
                .into_iter()
                .filter(|&c| repo.commit(c).parents.len() <= 1)
                .collect();
            Some(Branch {
                tip,
                merge_base,
                own,
            })
        })
        .collect();
    let Some(oldest_base) = checked
        .iter()
        .flatten()
        .map(|b| b.merge_base)
        .min_by_key(|&c| (generation[c.ix()], c))
    else {
        return Ok(vec![None; branches.len()]);
    };
    // What the default branch has after the oldest merge base, newest first: after every
    // branch's merge base too, generations being what they are.
    let (upstream, _) = crate::upstream::difference(repo, generation, default, oldest_base);
    let upstream: Vec<CommitIx> = upstream
        .into_iter()
        .filter(|&c| repo.commit(c).parents.len() <= 1)
        .collect();

    let mut singles: Vec<CommitIx> = upstream.clone();
    let mut seen: HashSet<CommitIx> = singles.iter().copied().collect();
    for b in checked.iter().flatten() {
        singles.extend(b.own.iter().copied().filter(|&c| seen.insert(c)));
    }
    let oid = |c: CommitIx| repo.commit(c).oid.to_hex();
    let lines: String = singles.iter().map(|&c| oid(c) + "\n").collect();
    let commit_ids = patch_ids(git, lines)?;
    // `<tip> <merge base>` diffs the merge base against the tip, headed by the tip.
    let mut pairs: Vec<(CommitIx, CommitIx)> = checked
        .iter()
        .flatten()
        .map(|b| (b.tip, b.merge_base))
        .collect();
    pairs.sort_unstable();
    pairs.dedup();
    let lines: String = pairs
        .iter()
        .map(|&(tip, base)| format!("{} {}\n", oid(tip), oid(base)))
        .collect();
    let branch_ids = patch_ids(git, lines)?;

    let mut copies: HashMap<&str, Vec<CommitIx>> = HashMap::new();
    for &c in &upstream {
        if let Some(id) = commit_ids.get(&repo.commit(c).oid) {
            copies.entry(id).or_default().push(c);
        }
    }
    let answers = checked
        .iter()
        .map(|b| {
            let b = b.as_ref()?;
            let after: HashSet<CommitIx> =
                crate::upstream::difference(repo, generation, default, b.merge_base)
                    .0
                    .into_iter()
                    .collect();
            let landing = |id: Option<&String>| -> Option<CommitIx> {
                copies
                    .get(id?.as_str())?
                    .iter()
                    .copied()
                    .find(|c| after.contains(c))
            };
            let landed = |how: How, c: CommitIx| Merged {
                how: Some(how),
                landed: Some(repo.commit(c).oid),
                pull_request: None,
            };
            // Commits without a change (empty ones) have no patch id, and a rebase drops them.
            let ids: Vec<&String> = b
                .own
                .iter()
                .filter_map(|c| commit_ids.get(&repo.commit(*c).oid))
                .collect();
            if ids.len() >= 2 {
                let rebased: Option<Vec<CommitIx>> =
                    ids.iter().map(|id| landing(Some(id))).collect();
                if let Some(copies) = rebased {
                    return Some(landed(How::Rebase, copies[0]));
                }
            }
            let squashed = landing(branch_ids.get(&repo.commit(b.tip).oid))?;
            Some(landed(How::Squash, squashed))
        })
        .collect();
    Ok(answers)
}

/// `git patch-id --stable` of what `git diff-tree --stdin` prints for `lines`: by the commit
/// heading each diff. A diff that changes nothing has none.
fn patch_ids(git: &Git, lines: String) -> Result<HashMap<Oid, String>, GitError> {
    if lines.is_empty() {
        return Ok(HashMap::new());
    }
    let diffs = git.run_with_input_bytes(&["diff-tree", "--stdin", "-p", "--no-renames"], lines)?;
    let ids = git.run_with_input(&["patch-id", "--stable"], diffs)?;
    Ok(ids
        .lines()
        .filter_map(|line| {
            let (id, commit) = line.split_once(' ')?;
            Some((Oid::from_hex(commit)?, id.to_owned()))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{Commit, Head};

    fn repo(default_branch: Option<&str>) -> Repo {
        let commit = Commit {
            oid: Oid::from_hex(&"ab".repeat(20)).unwrap(),
            parents: Vec::new(),
            truncated: false,
            empty_tree: false,
            author_name: String::new(),
            author_email: String::new(),
            author_time: 0,
            author_date: String::new(),
            commit_time: 0,
            subject: String::new(),
        };
        let mut repo = Repo::new(
            "/x".into(),
            vec![commit],
            Vec::new(),
            Head::Detached(CommitIx(0)),
        );
        repo.abbrev_len = 7;
        repo.default_branch = default_branch.map(str::to_owned);
        repo
    }

    fn merged(how: Option<How>, landed: bool, pull_request: Option<u64>) -> Merged {
        Merged {
            how,
            landed: landed.then(|| Oid::from_hex(&"abcdef0123".repeat(4)).unwrap()),
            pull_request,
        }
    }

    #[test]
    fn says_where_it_landed() {
        let r = repo(Some("refs/remotes/origin/main"));
        let says = |m: Merged| m.describe(&r);
        assert_eq!(
            says(merged(Some(How::Squash), true, None)),
            "squash-merged as abcdef0"
        );
        assert_eq!(
            says(merged(Some(How::Squash), true, Some(123))),
            "squash-merged as abcdef0 in PR #123"
        );
        assert_eq!(
            says(merged(Some(How::Rebase), true, None)),
            "rebase-merged into origin/main"
        );
        assert_eq!(
            says(merged(None, true, Some(7))),
            "merged as abcdef0 in PR #7"
        );
        assert_eq!(says(merged(None, false, Some(7))), "merged in PR #7");
        let r = repo(None);
        assert_eq!(
            merged(Some(How::Rebase), true, None).describe(&r),
            "rebase-merged as abcdef0"
        );
    }

    #[test]
    fn either_says_merged() {
        let local = merged(Some(How::Squash), false, None);
        let forge = merged(None, true, Some(9));
        assert_eq!(
            Merged::either(Some(local.clone()), Some(forge.clone())),
            Some(merged(Some(How::Squash), true, Some(9)))
        );
        assert_eq!(Merged::either(Some(local.clone()), None), Some(local));
        assert_eq!(Merged::either(None, Some(forge.clone())), Some(forge));
        assert_eq!(Merged::either(None, None), None);
    }

    #[test]
    fn stored_as_json() {
        let m = merged(Some(How::Rebase), true, Some(3));
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<Merged>(&json).unwrap(), m);
    }
}
