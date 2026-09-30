#!/usr/bin/env bash
# THROWAWAY native UI. Scratch repository only; Create previews a command.
# Bottom bar: five scenarios and A/B/C layouts; arrows preserve the current form.
# PARTERRE_BRANCH_PROTO=0..4 and PARTERRE_BRANCH_VARIANT=A|B|C choose startup state.
# Selecting a remote preserves a manually edited name; use the suggested-name button to reset.
set -euo pipefail
cd "$(dirname "$0")/.."
proto_repo=$(mktemp -d /tmp/parterre-branch-dialog-XXXXXX)
git -C "$proto_repo" init -q -b main
git -C "$proto_repo" config user.name 'Alex Developer'
git -C "$proto_repo" config user.email 'prototype@example.invalid'
git -C "$proto_repo" remote add origin https://example.invalid/prototype.git
git -C "$proto_repo" commit -q --allow-empty -m 'Start the project'
proto_root=$(git -C "$proto_repo" rev-parse HEAD)
git -C "$proto_repo" branch new-topic "$proto_root"
git -C "$proto_repo" commit -q --allow-empty -m 'A commit without branch labels'
git -C "$proto_repo" tag demo-unlabelled
git -C "$proto_repo" commit -q --allow-empty -m 'Explore a new feature'
proto_untracked=$(git -C "$proto_repo" rev-parse HEAD)
git -C "$proto_repo" update-ref refs/remotes/origin/new-topic "$proto_untracked"
git -C "$proto_repo" update-ref refs/remotes/origin/second-topic "$proto_untracked"
git -C "$proto_repo" commit -q --allow-empty -m 'Refine the shared feature'
proto_tracked=$(git -C "$proto_repo" rev-parse HEAD)
git -C "$proto_repo" update-ref refs/remotes/origin/shared-topic "$proto_tracked"
git -C "$proto_repo" branch --track shared-topic origin/shared-topic >/dev/null
git -C "$proto_repo" branch --track shared-experiment origin/shared-topic >/dev/null
git -C "$proto_repo" commit -q --allow-empty -m 'Compare two remote branches at one commit'
proto_mixed=$(git -C "$proto_repo" rev-parse HEAD)
git -C "$proto_repo" update-ref refs/remotes/origin/a-already-tracked "$proto_mixed"
git -C "$proto_repo" update-ref refs/remotes/origin/b-untracked "$proto_mixed"
git -C "$proto_repo" branch --track existing-topic origin/a-already-tracked >/dev/null
git -C "$proto_repo" commit -q --allow-empty -m 'Current work on main'
printf 'Prototype repository: %s\n' "$proto_repo"
export PARTERRE_BRANCH_PROTO=${PARTERRE_BRANCH_PROTO:-1}
cargo run --quiet -- "$proto_repo" --fit --window-size 1280x900 "$@"
