#!/bin/sh
# Prints the ID of a canary.yml run of commit SHA on main whose binaries (build.yml's
# binary-<target> artifacts, all four) are still there, or nothing: then the caller builds them
# itself. Needs gh with GH_TOKEN and GITHUB_REPOSITORY.
#
#   .github/scripts/canary-run.sh SHA
set -eu
sha=$1
targets="x86_64-unknown-linux-gnu x86_64-pc-windows-msvc aarch64-apple-darwin x86_64-apple-darwin"
# None at all where canary.yml isn't on the default branch yet: GitHub answers 404, and gh
# prints the answer.
runs=$(gh api "repos/$GITHUB_REPOSITORY/actions/workflows/canary.yml/runs?head_sha=$sha&branch=main&per_page=20" \
    --jq '.workflow_runs[].id' 2>/dev/null) || runs=
for run in $runs; do
    names=$(gh api "repos/$GITHUB_REPOSITORY/actions/runs/$run/artifacts?per_page=100" \
        --jq '.artifacts[] | select(.expired | not) | .name') || continue
    found=true
    for target in $targets; do
        printf '%s\n' "$names" | grep -qx "binary-$target" || found=false
    done
    if [ "$found" = true ]; then
        echo "$run"
        exit 0
    fi
done
