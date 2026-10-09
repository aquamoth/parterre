#!/usr/bin/env bash
# Takes the README's images, of the storefront demo (scripts/readme-demo-repo.py), into a
# folder: scripts/readme-images.sh [OUT] (default /tmp/parterre-readme). They go in a new
# docs/images/<major>.<minor>/, never over an old one: docs/images/README.md says why.
#
# Light and dark, at 2x. Uses target/debug/parterre, or $PARTERRE, under
# xvfb-run as scripts/screenshots.sh does. Needs Python with Pillow, to crop.
set -euo pipefail
out=$(realpath -m "${1:-/tmp/parterre-readme}")
root=$(cd "$(dirname "$0")/.." && pwd)

if [[ -z ${PARTERRE_XVFB:-} ]]; then
    exec env -u WAYLAND_DISPLAY PARTERRE_XVFB=1 \
        xvfb-run -a -s "-screen 0 3400x2600x24" "$0" "$out"
fi

bin=${PARTERRE:-$root/target/debug/parterre}
if [[ -z ${PARTERRE:-} ]]; then
    cargo build --quiet --manifest-path "$root/Cargo.toml" -p parterre
fi

# The worktrees' folder names show in the graph, and the repository's path in the status bar.
work=/tmp/acme
mkdir -p "$out"
failed=()

# shot NAME THEME CROP STEPS [ARGS...]: run STEPS, then take NAME-THEME.png, cropped to the
# topmost window (window) or not at all (full). $WSIZE sets the window's size, in points.
shot() {
    local name=$1 theme=$2 crop=$3 steps=$4 data
    shift 4
    # Scripted runs read no settings; this keeps them from writing any either.
    data=$(mktemp -d)
    if ! printf '%s\nscreenshot "%s/%s-%s.png" %s\n' "$steps" "$out" "$name" "$theme" "$crop" |
        XDG_DATA_HOME=$data XDG_CONFIG_HOME=$data WINIT_X11_SCALE_FACTOR=2 \
            "$bin" "$work/storefront" --window-size "${WSIZE:-1280x800}" --worktrees \
            --pull-requests-from "$work/prs.json" --theme "$theme" --script - "$@" \
            2> >(grep -v '^saved screenshot\|^frame interval\|^graph:' >&2); then
        failed+=("$name-$theme")
    fi
    rm -rf "$data"
}

# crop FILE LEFT TOP RIGHT BOTTOM, in pixels.
crop() {
    python3 -c 'import sys; from PIL import Image
f, *box = sys.argv[1:]; Image.open(f).crop(tuple(map(int, box))).save(f, optimize=True)' "$@"
}

"$root/scripts/readme-demo-repo.py" "$work" >/dev/null
# `open diff:` names a commit by a branch, a tag or a hash: "Cart: total in cents".
cents=$(git -C "$work/storefront" rev-parse --short fix/cart-rounding~1)
for theme in light dark; do
    shot hero $theme full $'hover "146"\nwait 1' --fit
    shot worktrees $theme full $'right-click node:feature/dark-mode\nhover "Actions"\nwait 0.5' --fit
    shot git-menu $theme full $'click node:fix/cart-rounding\nclick "Git"\nwait 0.5' --fit
    # A larger main window, so that a margin is left all round the windows' crops.
    WSIZE=1500x1100 shot log $theme window \
        $'open log:fix/cart-rounding\nwait-for "Cart: total in cents"\nclick "Cart: total in cents"\nwait 0.5'
    WSIZE=1500x1100 shot diff $theme window \
        $'open diff:'"$cents"$':src/cart.rs\nwait-for "total"\nwait 0.5' --diff-unfolded
    WSIZE=1500x1100 shot blame $theme window \
        $'open blame:fix/cart-rounding:src/cart.rs:30\nwait 0.5\nhover 1490,1000\nwait 1'
done
# The 16-point margin (32 px) that a window's crop keeps.
for f in "$out"/{log,diff,blame}-*.png; do
    [[ -f $f ]] || continue
    read -r w h < <(python3 -c 'import sys; from PIL import Image; print(*Image.open(sys.argv[1]).size)' "$f")
    crop "$f" 32 32 $((w - 32)) $((h - 32))
done

"$root/scripts/readme-demo-repo.py" "$work" --rebasing >/dev/null
for theme in light dark; do
    shot rebase $theme full $'click node:fix/cart-rounding\nhover canvas\nwait 0.5' --fit
    crop "$out/rebase-$theme.png" 0 250 2560 870
done

# Dragging, at 1x: the graph gives way, a subtree moves as one, and the layout comes back.
drag_steps='wait 0.5
drag node:experiment/wasm -330,60
hover canvas
wait 1.5
drag node:release/1.1 -240,-10
hover canvas
wait 1.5
key 3
drag node:v1.2.0 200,40
hover canvas
wait 1.5
click "Layout"
click "Return all nodes to layout"
wait 2
'
for theme in light dark; do
    "$root/scripts/readme-demo-repo.py" "$work" >/dev/null
    data=$(mktemp -d)
    if ! printf '%s' "$drag_steps" |
        XDG_DATA_HOME=$data XDG_CONFIG_HOME=$data WINIT_X11_SCALE_FACTOR=1 \
            "$bin" "$work/storefront" --window-size 1000x620 --worktrees \
            --pull-requests-from "$work/prs.json" --theme $theme --fit \
            --record "$out/drag-$theme.gif" --script - \
            2> >(grep -v '^saved recording\|^frame interval\|^graph:' >&2); then
        failed+=("drag-$theme")
    fi
    rm -rf "$data"
done

echo "images in $out"
if ((${#failed[@]})); then
    echo "failed: ${failed[*]}" >&2
    exit 1
fi
