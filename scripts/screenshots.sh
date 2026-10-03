#!/usr/bin/env bash
# Takes a screenshot of every window, dialog and menu of parterre, of a demo repository, into
# a folder: scripts/screenshots.sh [OUT] (default /tmp/parterre-screenshots).
#
# Uses target/debug/parterre (egui shows widget id clashes only in debug builds), or $PARTERRE.
# It runs itself under xvfb-run, so that no real pointer hovers anything. Extra arguments for every run, such
# as `--theme dark` or `--text-size 1.5`, go in $PARTERRE_ARGS. See docs/automation.md.
set -euo pipefail
out=$(realpath -m "${1:-/tmp/parterre-screenshots}")
root=$(cd "$(dirname "$0")/.." && pwd)

if [[ -z ${PARTERRE_XVFB:-} ]]; then
    # A fixed, larger screen than the window, and X11 even on a Wayland desktop: the same
    # pictures everywhere, and no real pointer hovering over them.
    exec env -u WAYLAND_DISPLAY PARTERRE_XVFB=1 \
        xvfb-run -a -s "-screen 0 1920x1200x24" "$0" "$out"
fi

bin=${PARTERRE:-$root/target/debug/parterre}
if [[ -z ${PARTERRE:-} ]]; then
    cargo build --quiet --manifest-path "$root/Cargo.toml" -p parterre
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
repo=$work/demo
"$root/scripts/make-demo-repo.sh" "$repo" >/dev/null
# A change to a file, for the diff and blame windows, and a second worktree.
git -C "$repo" switch -q feature/reports
printf 'parterre demo\n\nReports are exported as PDF.\n' >"$repo/README"
git -C "$repo" commit -q -am "Describe reports"
git -C "$repo" switch -q main
git -C "$repo" worktree add -q "$work/demo-login" feature/login

mkdir -p "$out"
# Stored settings (window size, moved nodes) are not read in scripted runs; this keeps the
# runs from writing any either.
export XDG_DATA_HOME=$work/data XDG_CONFIG_HOME=$work/config
failed=()

# shot NAME CROP STEPS [ARGS...]: run STEPS (a script, one step per line), then take NAME.png,
# cropped to the topmost window (window), the open menus (popup) or not at all (full). Of the
# demo repository, or of the worktree in $SHOT_REPO.
shot() {
    local name=$1 crop=$2 steps=$3
    shift 3
    # shellcheck disable=SC2086 # PARTERRE_ARGS is a list of arguments.
    if ! printf '%s\nscreenshot "%s/%s.png" %s\n' "$steps" "$out" "$name" "$crop" |
        "$bin" "${SHOT_REPO:-$repo}" --window-size 1200x800 --script - "$@" ${PARTERRE_ARGS:-} \
            2> >(grep -v '^saved screenshot\|^frame interval' >&2); then
        failed+=("$name")
    fi
}

shot main full ""
shot main-fit full "" --fit
shot main-drag full $'drag node:feature/search 250,60\nhover canvas\nwait 1.5'
shot search full $'key Ctrl+F\ntype "login"'

shot menu popup "click toolbar:menu"
for sub in "Recent folders" Export Show Filter Zoom Drag "Newest commits"; do
    name=$(tr 'A-Z ' 'a-z-' <<<"$sub")
    shot "menu-$name" popup $'click toolbar:menu\nhover "'"$sub"'"'
done
for popover in filter zoom drag; do
    shot "popover-$popover" popup "click toolbar:$popover"
done
shot context-node popup "right-click node:main"
for sub in Compare Open Copy; do
    name=$(tr 'A-Z ' 'a-z-' <<<"$sub")
    shot "context-node-$name" popup $'right-click node:main\nhover "'"$sub"'"'
done
shot context-canvas popup "right-click canvas"

for page in appearance branchcolours graph filters dragging advanced manage; do
    shot "settings-$page" window "open settings:$page"
done
shot reset-settings window $'open settings:manage\nclick "Reset…"'
shot shortcuts window "open shortcuts"
shot legend window "open legend"
shot about window "open about"

shot log window $'open log:v0.1.0..feature/reports\nwait-for "Describe reports"'
shot compare window $'open compare:v0.3.0..feature/reports\nwait-for "README"'
shot diff window $'open diff:feature/reports:README\nwait-for "Reports are exported as PDF."'
shot blame window $'open blame:feature/reports:README:3\nwait-for "Reports are exported as PDF."'

shot create-branch window $'open create-branch:v0.3.0\nwait-for "Create branch"'
shot add-worktree window $'open add-worktree:v0.3.0\nwait-for "Add a worktree"'
shot delete-worktree window $'open delete-worktree:demo-login\nwait 0.5'
shot reset window $'open reset:v0.3.0\nwait 0.5'
shot rebase window $'open rebase:feature/dark-mode\nwait 0.5'
shot merge window $'open merge:feature/dark-mode\nwait 0.5'
git -C "$repo" worktree add -q "$work/demo-dark-mode" feature/dark-mode
SHOT_REPO=$work/demo-dark-mode shot merge-into window \
    $'right-click node:main\nclick "Merge feature/dark-mode into main…"\nwait 0.5'

echo "screenshots in $out"
if ((${#failed[@]})); then
    echo "failed: ${failed[*]}" >&2
    exit 1
fi
