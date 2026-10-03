#!/bin/sh
# PROTOTYPE — throwaway. The demo repository for the rebase merge methods (#188): Rebase and
# fast-forward, Semi-linear merge, and the question what they rebase.
#
#   sh crates/parterre/src/app/prototype_rebase_merge_demo.sh /tmp/rm
#   cargo run -- /tmp/rm/demo
#
# The open worktree is on main. Right-click a node, Merge into main ▸ X, pick a rebase method,
# then under PROTOTYPE · What's rebased: X itself, a copy (detached), or a copy picked onto
# main (cherry-pick). Run the script again to start over.
#
#   feature      diverged from main, nothing special: the happy path for all three
#   pushed       like feature, but pushed with an upstream: X itself leaves it needing a
#                force push (the upstreams' dashed edge)
#   busy         checked out in worktree demo.worktrees/busy: X itself is greyed out
#   origin/remote-only   only a remote-tracking branch: X itself is greyed out
#   (a commit)   in the log window, a row of feature's (feature~1): X itself is greyed out
#   conflict     its first commit conflicts with main: every way stops halfway. Compare what's
#                checked out, what the banner says, and what's left to do
#   applied      its only commit is already on main (same patch): replays nothing, greyed out
#   ahead        on top of main, no merges: replays nothing, greyed out (Fast-forward will do)
#   with-merge   on top of main, with a merge inside: the rebase flattens it, so offered
#   dirty        a worktree (demo.worktrees/dirty) with uncommitted changes; go to it and
#                merge feature: the rebase methods need a clean worktree
set -e
root=$(realpath -m "${1:-/tmp/parterre-rebase-merge}")
rm -rf "$root"
mkdir -p "$root"
cd "$root"
git init -q --bare origin.git
git init -q -b main demo
cd demo
git config user.name Demo
git config user.email demo@example.com
c() { printf '%s\n' "$2" >"$1"; git add "$1"; git commit -qm "$3"; }
c README.md "A demo" "Initial commit"
c app.txt "app 1" "Add app"
c lib.txt "lib 1" "Add lib"
git remote add origin "$root/origin.git"
git push -q -u origin main
base=$(git rev-parse HEAD)

branch() { git switch -q -c "$1" "${2:-$base}"; }

branch feature
c feature.txt "feature 1" "Start feature"
c feature.txt "feature 2" "Finish feature"

branch pushed
c pushed.txt "pushed 1" "Pushed work"
c pushed.txt "pushed 2" "More pushed work"
git push -q -u origin pushed

branch busy
c busy.txt "busy" "Busy work"

branch remote-only
c remote.txt "remote" "Someone else's work"
git push -q origin remote-only
git switch -q main
git branch -q -D remote-only

branch conflict
c lib.txt "lib theirs" "Their lib change"
c other.txt "other" "Other work"

branch applied
c typo.txt "fixed" "Fix typo"

git switch -q main
c lib.txt "lib mine" "My lib change"
c typo.txt "fixed" "Fix typo"
git push -q origin main

branch ahead main
c ahead.txt "ahead" "Ahead work"

branch with-merge main
c merge1.txt "one" "Before the merge"
git switch -q -c side
c side.txt "side" "Side work"
git switch -q with-merge
git merge -q --no-ff -m "Merge branch 'side' into with-merge" side
git branch -q -D side

branch dirty main
git switch -q main
git worktree add -q ../demo.worktrees/dirty dirty
printf 'An edit\n' >>../demo.worktrees/dirty/README.md

git worktree add -q ../demo.worktrees/busy busy
git switch -q main
echo "Demo in $root/demo"
