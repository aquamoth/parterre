//! Which local branches were squash- or rebase-merged (#384), found on a worker thread by git
//! and, where `origin` is on a forge, by the forge as well (`parterre_core::merged`). Only gone
//! branches whose answer isn't known are asked about, ten at a time, the newest tips first: on
//! opening a repository and on every snapshot after it, so a fetch that prunes a merged branch
//! marks it soon after. A branch is asked about once per opening, and once more after a fetch
//! moves `origin`'s branches, so that a forge that couldn't answer gets another go.
//!
//! Answers are kept between restarts, by repository, for the branch's name and tip: "merged"
//! never changes for that pair, and is kept as soon as either side says so. "Not found" is kept
//! only once git and the forge have both answered, and dropped once the branch's upstream is
//! seen again: a branch made to track one not pushed yet is gone until its first push, and can
//! be merged with that tip later. Answers of branches that moved or went are dropped.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use eframe::egui;
use parterre_core::git::Git;
use parterre_core::merged::{self, Merged, MergedBranches};
use parterre_core::{Oid, Repo};
use parterre_forge::GoneBranch;
use serde::{Deserialize, Serialize};

use crate::worker;

pub const MERGED_KEY: &str = "parterre-merged-branches";

/// Branches asked about per job: the first answers show soon in a repository with many.
const PER_JOB: usize = 10;

/// By repository (its folder, as moves are kept), each local branch's answer by full name.
pub type Remembered = HashMap<String, HashMap<String, Answer>>;

pub fn load(storage: &dyn eframe::Storage) -> Remembered {
    eframe::get_value(storage, MERGED_KEY).unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    /// The tip it is about.
    pub tip: Oid,
    /// `None`: neither git nor the forge found it merged.
    pub merged: Option<Merged>,
}

/// What a job found for its branches, in their order.
#[derive(Debug)]
struct Found {
    merged: Vec<Option<Merged>>,
    /// Both git and the forge, where there is one, answered: "not found" can be kept.
    complete: bool,
}

#[derive(Debug)]
struct Job {
    key: String,
    /// Full names and tips.
    branches: Vec<(String, Oid)>,
    rx: Receiver<Found>,
}

#[derive(Debug, Default)]
pub struct MergedLoader {
    remembered: Remembered,
    shown_repo: Option<Arc<Repo>>,
    shown: Arc<MergedBranches>,
    /// Asked about since the repository was opened, or since `origin`'s branches last moved.
    asked: HashSet<(String, Oid)>,
    job: Option<Job>,
    /// A new snapshot, or a job ended: branches may be left to ask about.
    due: bool,
    /// Off for canned pull requests (`--pull-requests-from`): screenshots ask no forge.
    no_forge: bool,
}

impl MergedLoader {
    pub fn new(remembered: Remembered, no_forge: bool) -> MergedLoader {
        MergedLoader {
            remembered,
            no_forge,
            ..MergedLoader::default()
        }
    }

    pub fn remembered(&self) -> &Remembered {
        &self.remembered
    }

    /// The answers for the repository shown, for the branches where they are now.
    pub fn shown(&self) -> &Arc<MergedBranches> {
        &self.shown
    }

    pub fn is_loading(&self) -> bool {
        self.job.is_some()
    }

    /// Follows the snapshot shown and starts a job if one is due; returns whether the answers
    /// shown changed.
    pub fn update(&mut self, repo: Option<&Arc<Repo>>, ctx: &egui::Context) -> bool {
        let mut changed = false;
        if !same_snapshot(self.shown_repo.as_ref(), repo) {
            let reopened = self
                .shown_repo
                .as_ref()
                .zip(repo)
                .is_none_or(|(old, new)| old.path != new.path);
            let origin_moved = self
                .shown_repo
                .as_ref()
                .zip(repo)
                .is_some_and(|(old, new)| parterre_forge::origin_moved(old, new));
            if reopened || origin_moved {
                self.asked.clear();
            }
            self.shown_repo = repo.cloned();
            if let Some(repo) = repo
                && let Some(answers) = self.remembered.get_mut(&key(repo))
            {
                forget_stale(answers, repo);
                if answers.is_empty() {
                    self.remembered.remove(&key(repo));
                }
            }
            changed |= self.show();
            self.due = true;
        }
        if let Some(found) = self.job.as_ref().and_then(|job| {
            worker::received(&job.rx, || Found {
                merged: vec![None; job.branches.len()],
                complete: false,
            })
        }) {
            let job = self.job.take().expect("a job");
            let answers = self.remembered.entry(job.key).or_default();
            for ((name, tip), merged) in job.branches.into_iter().zip(found.merged) {
                if merged.is_some() || found.complete {
                    answers.insert(name, Answer { tip, merged });
                }
            }
            changed |= self.show();
            self.due = true;
        }
        if self.job.is_none() && std::mem::take(&mut self.due) {
            self.start(ctx);
        }
        changed
    }

    /// Puts the remembered answers of the repository shown on show; returns whether that
    /// changed what is shown.
    fn show(&mut self) -> bool {
        let mut shown = MergedBranches::default();
        if let Some(repo) = &self.shown_repo
            && let Some(answers) = self.remembered.get(&key(repo))
        {
            for (name, answer) in answers {
                if let Some(merged) = &answer.merged {
                    shown.insert(name, answer.tip, merged.clone());
                }
            }
        }
        let changed = *self.shown != shown;
        self.shown = Arc::new(shown);
        changed
    }

    fn start(&mut self, ctx: &egui::Context) {
        let Some(repo) = self.shown_repo.clone() else {
            return;
        };
        let key = key(&repo);
        let known = self.remembered.get(&key);
        let due: Vec<usize> = merged::candidates(&repo)
            .into_iter()
            .filter(|&b| {
                let r = &repo.refs[b];
                let tip = repo.commit(r.target).oid;
                let remembered = known
                    .and_then(|k| k.get(&r.full_name))
                    .is_some_and(|a| a.tip == tip);
                !remembered && !self.asked.contains(&(r.full_name.clone(), tip))
            })
            .take(PER_JOB)
            .collect();
        if due.is_empty() {
            return;
        }
        let branches: Vec<(String, Oid)> = due
            .iter()
            .map(|&b| {
                let r = &repo.refs[b];
                (r.full_name.clone(), repo.commit(r.target).oid)
            })
            .collect();
        self.asked.extend(branches.iter().cloned());
        let no_forge = self.no_forge;
        let rx = worker::spawn(ctx, move || check(&repo, &due, no_forge));
        self.job = Some(Job { key, branches, rx });
    }
}

/// Asks git, then the forge `origin` is on, about `branches` of `repo`.
fn check(repo: &Repo, branches: &[usize], no_forge: bool) -> Found {
    let git = Git::new(&repo.path);
    let local = merged::find(&git, repo, branches);
    let origin = (!no_forge).then(|| parterre_forge::origin(&git)).flatten();
    // The forge knows only branches whose upstream was on `origin`.
    let gone: Vec<(usize, GoneBranch)> = branches
        .iter()
        .enumerate()
        .filter_map(|(i, &b)| Some((i, GoneBranch::of(repo, b)?)))
        .collect();
    let mut forge: Vec<Option<Merged>> = vec![None; branches.len()];
    let forge_answered = match &origin {
        None => true,
        Some(origin) => {
            let asked: Vec<GoneBranch> = gone.iter().map(|(_, g)| g.clone()).collect();
            match origin.merged(&git, &asked) {
                Ok(found) => {
                    for ((i, _), merged) in gone.iter().zip(found) {
                        forge[*i] = merged;
                    }
                    true
                }
                // A build without it never will.
                Err(parterre_forge::ForgeError::Unsupported(_)) => true,
                Err(_) => false,
            }
        }
    };
    let complete = local.is_ok() && forge_answered;
    let local = local.unwrap_or_else(|_| vec![None; branches.len()]);
    Found {
        merged: local
            .into_iter()
            .zip(forge)
            .map(|(local, forge)| Merged::either(local, forge))
            .collect(),
        complete,
    }
}

fn key(repo: &Repo) -> String {
    repo.path.display().to_string()
}

fn same_snapshot(a: Option<&Arc<Repo>>, b: Option<&Arc<Repo>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// Drops the answers about branches that went or moved, and the "not found" of each branch
/// whose upstream is there again.
fn forget_stale(answers: &mut HashMap<String, Answer>, repo: &Repo) {
    answers.retain(|name, answer| {
        let Some(b) = repo.refs.iter().position(|r| r.full_name == *name) else {
            return false;
        };
        let at_tip = repo.commit(repo.refs[b].target).oid == answer.tip;
        let upstream_back = repo.upstream_of(b).is_some_and(|u| !u.is_gone());
        at_tip && (answer.merged.is_some() || !upstream_back)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use parterre_core::merged::How;

    use crate::app::tool_harness::{git, init};

    /// A repository with `topic` squash-merged into `main` on a bare `origin` (not on a
    /// forge), and `topic` deleted there, which the push takes out here too. Returns the squash
    /// commit.
    fn squash_merged(dir: &std::path::Path) -> String {
        let origin = dir.join("origin.git");
        let origin = origin.to_str().unwrap();
        let work = dir.join("work");
        git(dir, &["init", "-q", "--bare", "-b", "main", origin]);
        std::fs::create_dir(&work).unwrap();
        init(&work);
        let run = |args: &[&str]| git(&work, args);
        run(&["remote", "add", "origin", origin]);
        run(&["commit", "-q", "--allow-empty", "-m", "Start"]);
        run(&["switch", "-q", "-c", "topic"]);
        std::fs::write(work.join("b.txt"), "1\n").unwrap();
        run(&["add", "b.txt"]);
        run(&["commit", "-q", "-m", "Topic"]);
        run(&["push", "-q", "-u", "origin", "main", "topic"]);
        run(&["remote", "set-head", "origin", "main"]);
        run(&["switch", "-q", "main"]);
        run(&["merge", "-q", "--squash", "topic"]);
        run(&["commit", "-q", "-m", "Topic (#1)"]);
        run(&["push", "-q", "origin", "main", ":topic"]);
        run(&["rev-parse", "HEAD"])
    }

    fn settle(loader: &mut MergedLoader, repo: &Arc<Repo>, ctx: &egui::Context) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut changed = loader.update(Some(repo), ctx);
        while loader.is_loading() {
            assert!(
                std::time::Instant::now() < deadline,
                "the check never ended"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
            changed |= loader.update(Some(repo), ctx);
        }
        changed
    }

    #[test]
    fn a_merged_branch_is_found_once_and_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let landed = squash_merged(dir.path());
        let repo = Arc::new(parterre_core::git::load_repo(&dir.path().join("work")).unwrap());
        let ctx = egui::Context::default();
        let mut loader = MergedLoader::new(Remembered::default(), false);
        assert!(
            settle(&mut loader, &repo, &ctx),
            "the answer changed nothing"
        );
        let topic = repo
            .refs
            .iter()
            .position(|r| r.full_name == "refs/heads/topic")
            .unwrap();
        let expected = Merged {
            how: Some(How::Squash),
            landed: Oid::from_hex(&landed),
            pull_request: None,
        };
        assert_eq!(loader.shown().of(&repo, topic), Some(&expected));
        // Asked once: the next snapshot asks nothing.
        let again = Arc::new(parterre_core::git::load_repo(&dir.path().join("work")).unwrap());
        assert!(!settle(&mut loader, &again, &ctx));
        // A restart remembers it.
        let mut restarted = MergedLoader::new(loader.remembered().clone(), false);
        restarted.update(Some(&again), &ctx);
        assert!(!restarted.is_loading());
        assert_eq!(restarted.shown().of(&again, topic), Some(&expected));
    }

    #[test]
    fn stale_answers_are_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        squash_merged(dir.path());
        let work = dir.path().join("work");
        let repo = parterre_core::git::load_repo(&work).unwrap();
        let tip = |name: &str| {
            let r = repo.refs.iter().find(|r| r.name == name).unwrap();
            repo.commit(r.target).oid
        };
        let not_found = |tip| Answer { tip, merged: None };
        let mut answers: HashMap<String, Answer> = [
            ("refs/heads/topic", not_found(tip("topic"))),
            ("refs/heads/main", not_found(tip("main"))),
            ("refs/heads/moved", not_found(tip("main"))),
            ("refs/heads/gone", not_found(tip("main"))),
        ]
        .into_iter()
        .map(|(name, answer)| (name.to_owned(), answer))
        .collect();
        git(&work, &["branch", "-q", "moved", "topic"]);
        let repo = parterre_core::git::load_repo(&work).unwrap();
        forget_stale(&mut answers, &repo);
        let mut kept: Vec<&str> = answers.keys().map(String::as_str).collect();
        kept.sort_unstable();
        // `main`'s upstream is there: its "not found" may change.
        assert_eq!(kept, ["refs/heads/topic"]);
    }
}
