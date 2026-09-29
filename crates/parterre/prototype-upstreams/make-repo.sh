#!/usr/bin/env bash
# PROTOTYPE (throwaway): the demo repository plus one local branch for each upstream case.
# crates/parterre/prototype-upstreams/make-repo.sh /tmp/parterre-upstreams
set -euo pipefail
dir=${1:-/tmp/parterre-upstreams}
here=$(cd "$(dirname "$0")" && pwd)
"$here/../../../scripts/make-demo-repo.sh" "$dir" >/dev/null
cd "$dir"
t=1700200000
c() { t=$((t + 3600)); GIT_AUTHOR_DATE="$t +0000" GIT_COMMITTER_DATE="$t +0000" git commit -q --allow-empty -m "$1"; }

# Level: feature/reports and origin/feature/reports on one node.
git branch -q -u origin/feature/reports feature/reports
# Ahead: feature/dark-mode has "Contrast tweaks", not pushed.
git branch -q -u origin/feature/dark-mode feature/dark-mode
# Behind: someone else pushed to release/0.2.
git branch -q -u origin/release/0.2 release/0.2
other=$(mktemp -d)
git clone -q "$dir-origin.git" "$other/o"
( cd "$other/o" && git config user.name Other && git config user.email o@example.com &&
  git switch -q release/0.2 && t=1700300000 &&
  GIT_AUTHOR_DATE="$t +0000" GIT_COMMITTER_DATE="$t +0000" git commit -q --allow-empty -m "Backport search fix" &&
  git push -q origin release/0.2 )
rm -rf "$other"
# Diverged, safely: feature/export was pushed, then rebased onto main. Every commit on the
# remote has a rebased copy, so a force push loses nothing.
f() { echo "$2" >> "$1"; git add "$1"; c "$3"; }
git switch -q -c feature/export main~1
f export.txt csv "Export to CSV" && f export.txt xlsx "Export to Excel"
git push -q -u origin feature/export
# Diverged, unsafely: feature/import was pushed, a colleague pushed a fix to it, and then it
# was rebased onto main without pulling. A force push would drop the colleague's fix.
git switch -q -c feature/import main~1
f import.txt csv "Import from CSV" && f import.txt json "Import from JSON"
git push -q -u origin feature/import
other=$(mktemp -d)
git clone -q "$dir-origin.git" "$other/o"
( cd "$other/o" && git config user.name Other && git config user.email o@example.com &&
  git switch -q feature/import && echo fix >> fix.txt && git add fix.txt &&
  GIT_AUTHOR_DATE="1700400000 +0000" GIT_COMMITTER_DATE="1700400000 +0000" git commit -q -m "Fix encoding (colleague)" &&
  git push -q origin feature/import )
rm -rf "$other"
git fetch -q origin
git switch -q main
f main.txt x "Tidy up main"
for b in feature/export feature/import; do
  git switch -q "$b"
  GIT_COMMITTER_DATE="1700500000 +0000" git rebase -q main
done
git switch -q main
# Ahead by 3, with another branch on the middle commit: the dashed edge has a node between.
git switch -q feature/dark-mode
c "High-contrast theme" && git branch -q spike/theme && c "Theme preview" && c "Theme docs"
git switch -q main
# Rebased, then the colleague's fix cherry-picked on top: every remote commit has a copy,
# yet git's --force-if-includes still refuses, as the fix was never on the branch.
git switch -q -c feature/sync main~2
f sync.txt a "Sync engine" && f sync.txt b "Sync conflicts"
git push -q -u origin feature/sync
other=$(mktemp -d)
git clone -q "$dir-origin.git" "$other/o"
( cd "$other/o" && git config user.name Other && git config user.email o@example.com &&
  git switch -q feature/sync && echo retry >> retry.txt && git add retry.txt &&
  GIT_AUTHOR_DATE="1700600000 +0000" GIT_COMMITTER_DATE="1700600000 +0000" git commit -q -m "Retry on timeout (colleague)" &&
  git push -q origin feature/sync )
rm -rf "$other"
git fetch -q origin
GIT_COMMITTER_DATE="1700700000 +0000" git rebase -q main
GIT_COMMITTER_DATE="1700700100 +0000" git cherry-pick origin/feature/sync >/dev/null
git switch -q main
# Different name: a local "search-v2" tracking origin/feature/search, one commit ahead.
git switch -q -c search-v2 origin/feature/search
c "Search ranking"
git switch -q main
# Gone: feature/login's upstream was deleted on the remote.
git branch -q -u origin/feature/login feature/login
git push -q origin --delete feature/login
git fetch -q --prune origin
# No upstream: feature/search (never set).
git for-each-ref --format='%(refname:short) -> %(upstream:short) %(upstream:track)' refs/heads
echo "Upstreams demo repository in $dir"
