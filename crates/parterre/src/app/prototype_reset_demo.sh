#!/bin/sh
# PROTOTYPE — throwaway. The demo repository for prototype_reset.rs (Resetting a branch, #172):
# one worktree per case, each to be reset to the commit before its tip.
#
#   sh crates/parterre/src/app/prototype_reset_demo.sh /tmp/pr
#   cargo run --release -- /tmp/pr/demo
#
# Go to a worktree, Show log, right-click the second row: Reset <branch> to here.
set -e
root=$(realpath -m "${1:-/tmp/parterre-reset}")
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
c app.txt "one" "Add app"
c lib.txt "lib 1" "Add lib"
c app.txt "two" "Change app"
c notes.txt "notes" "Add notes"
git remote add origin "$root/origin.git"
git push -q -u origin main
# A branch with app.txt changed, to merge or rebase onto with a conflict.
git branch other
git switch -q other
c app.txt "theirs" "Their app change"
git switch -q main

wt() {
    git worktree add -q -b "$1" "../demo.worktrees/$1" main
    cd "../demo.worktrees/$1"
}
back() { cd "$root/demo"; }

# Clean, and its tip is on origin: nothing can be lost.
wt pushed
c pushed.txt "pushed" "Pushed work"
git push -q -u origin pushed
back

# Clean, but its tip is on no other branch: the commit is lost (or kept as changes).
wt unpushed
c local.txt "local" "Local only work"
back

# A change to a file the tip didn't touch.
wt dirty
c lib.txt "lib 2" "Change lib"
git push -q -u origin dirty
printf 'An edit\n' >>README.md
back

# A staged new file, and a file staged as S and then edited to W.
wt staged
c lib.txt "lib 2" "Change lib"
git push -q -u origin staged
printf 'new\n' >new.txt
git add new.txt
printf 'S\n' >notes.txt
git add notes.txt
printf 'W\n' >notes.txt
back

# A change to the file the tip changed (Keep and Merge refuse), and an untracked file where
# the target has one.
wt in-the-way
printf 'three\n' >app.txt
git rm -q lib.txt
git add app.txt
git commit -qm "Change app, remove lib"
git push -q -u origin in-the-way
printf 'mine\n' >app.txt
printf 'untracked\n' >lib.txt
back

# A merge stopped on a conflict.
wt merging
c app.txt "ours" "Our app change"
git push -q -u origin merging
git merge -q other >/dev/null 2>&1 || true
back

# A rebase stopped on a conflict: the branch is in use, so there's nothing to reset.
wt rebasing
c app.txt "rebased" "App change to rebase"
git rebase -q other >/dev/null 2>&1 || true
back

echo "Demo in $root/demo; worktrees in $root/demo.worktrees"
