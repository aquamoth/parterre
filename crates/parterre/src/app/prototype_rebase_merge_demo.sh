#!/bin/sh
# PROTOTYPE — throwaway. The demo repository for merging with a rebase first (#188), round 2:
# from a topic branch's worktree, Merge feature into main…, as a pull request merges.
#
#   sh crates/parterre/src/app/prototype_rebase_merge_demo.sh /tmp/rm
#   cargo run -- /tmp/rm/demo.worktrees/feature
#
# Right-click main (or release), Merge feature into main…, and pick one of the four methods.
# Use Go to worktree for the other cases. Run the script again to start over.
#
# Branches merged into:
#   main      checked out in the main worktree (/tmp/rm/demo): the merge runs there
#   release   checked out nowhere: a fast-forward moves the ref; a merge commit switches this
#             worktree to release and back
#   staging   checked out in demo.worktrees/staging, with uncommitted changes: no merge
#             commit into it
# Worktrees to merge from (demo.worktrees/…):
#   feature     diverged from main, nothing special: the happy path for all four methods
#   pushed      like feature, but pushed: after a rebase it needs a force push (the upstreams'
#               dashed edge)
#   conflict    its first commit conflicts with main. A rebase stops here, in its own
#               worktree; a merge commit stops in main's worktree
#   ahead       on top of main, no merges: the rebase methods have nothing to replay
#   with-merge  on top of main, with a merge inside: the rebase flattens it, so offered
#   applied     its only commit is already on main: nothing to replay
#   dirty       like feature, with uncommitted changes: the rebase methods offer Stash changes
#   The other way round still works too: Merge into feature › main, with only Fast-forward and
#   Merge commit.
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

branch conflict
c lib.txt "lib theirs" "Their lib change"
c other.txt "other" "Other work"

branch applied
c typo.txt "fixed" "Fix typo"

branch dirty
c dirty.txt "dirty 1" "Dirty work"

git switch -q main
c lib.txt "lib mine" "My lib change"
git branch -q release
c typo.txt "fixed" "Fix typo"
git push -q origin main
git branch -q staging

branch ahead main
c ahead.txt "ahead" "Ahead work"

branch with-merge main
c merge1.txt "one" "Before the merge"
git switch -q -c side
c side.txt "side" "Side work"
git switch -q with-merge
git merge -q --no-ff -m "Merge branch 'side' into with-merge" side
git branch -q -D side

git switch -q main
for b in feature pushed conflict ahead with-merge applied dirty staging; do
    git worktree add -q "../demo.worktrees/$b" "$b"
done
printf 'An edit\n' >>../demo.worktrees/dirty/README.md
printf 'A staging edit\n' >>../demo.worktrees/staging/README.md
echo "Demo in $root/demo; start from $root/demo.worktrees/feature"
