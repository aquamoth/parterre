#!/bin/sh
# PROTOTYPE — throwaway. The demo repository for prototype_rebase.rs (Rebasing, #184).
#
#   sh crates/parterre/src/app/prototype_rebase_demo.sh /tmp/rb
#   cargo run -- /tmp/rb/demo
#
# main (the open worktree): right-click origin/main's node, Rebase main onto ▸ (it also has
# release and staging). Replays 2, drops 1 already in origin/main, flattens 1 merge.
# Worktrees (Go to worktree, on the node at the first commits):
#   conflict   rebasing onto origin/main stops on lib.txt
#   dirty      uncommitted change to README.md: the Stash changes checkbox
#   autostash  rebase.autoStash set here; putting lib.txt back conflicts, so it stays stashed
#   behind    only behind origin/main: no Rebase offered (it would only fast-forward)
#   stuck      already mid-rebase with a conflict: the banner, and what's greyed out
set -e
root=$(realpath -m "${1:-/tmp/parterre-rebase}")
rm -rf "$root"
mkdir -p "$root"
cd "$root"
git init -q --bare origin.git
git init -q -b main demo
cd demo
git config user.name Demo
git config user.email demo@example.com
git config extensions.worktreeConfig true
c() { printf '%s\n' "$2" >"$1"; git add "$1"; git commit -qm "$3"; }
c README.md "A demo" "Initial commit"
c app.txt "app 1" "Add app"
c lib.txt "lib 1" "Add lib"
git remote add origin "$root/origin.git"
git push -q -u origin main
base=$(git rev-parse HEAD)

# Someone else's work on origin/main: a lib change and a typo fix.
git switch -q -c theirs
c lib.txt "lib theirs" "Their lib change"
c typo.txt "fixed" "Fix typo"
git push -q origin theirs:main
git switch -q main
git branch -q -D theirs
git fetch -q origin
git branch -q release origin/main
git branch -q staging origin/main

wt() {
    git worktree add -q -b "$1" "../demo.worktrees/$1" "$base"
    cd "../demo.worktrees/$1"
}
back() { cd "$root/demo"; }

wt conflict
c lib.txt "lib mine" "My lib change"
back

wt dirty
c dirty.txt "dirty" "Dirty work"
printf 'An edit\n' >>README.md
back

wt autostash
git config --worktree rebase.autoStash true
c other.txt "other" "Other work"
printf 'lib local edit\n' >lib.txt
back

git worktree add -q -b behind ../demo.worktrees/behind "$base"

wt stuck
c stuck.txt "stuck" "Stuck work"
c lib.txt "lib stuck" "Stuck lib change"
GIT_EDITOR=: git rebase -q origin/main >/dev/null 2>&1 || true
back

# main: the same typo fix (dropped), a feature, and a merged side branch (flattened).
c typo.txt "fixed" "Fix typo"
c feature.txt "feature" "Add feature"
git switch -q -c side
c side.txt "side" "Side work"
git switch -q main
git merge -q --no-ff -m "Merge branch 'side'" side
git branch -q -D side
echo "Demo in $root/demo"
