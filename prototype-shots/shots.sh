#!/usr/bin/env bash
set -uo pipefail
root=/home/mattias/.t3/worktrees/parterre/t3code-6badf677
out=/tmp/t175/shots; rm -rf $out; mkdir -p $out
work=$(mktemp -d); repo=$work/demo
"$root/scripts/make-demo-repo.sh" "$repo" >/dev/null
export XDG_DATA_HOME=$work/data XDG_CONFIG_HOME=$work/config PARTERRE_PROTOTYPE_NO_BAR=1
bin=$root/target/debug/parterre
shot() { # NAME VARIANT CROP STEPS [env...]
  local name=$1 v=$2 crop=$3 steps=$4; shift 4
  printf '%s\nscreenshot "%s/%s.png" %s\n' "$steps" "$out" "$name" "$crop" |
    env PARTERRE_PROTOTYPE=$v "$@" "$bin" "$repo" --window-size 1200x800 --theme light --script - 2>&1 | grep -v '^saved\|^frame interval\|^graph:' 
}
for v in A B C; do shot prompt-$v $v full "wait 0.3"; done

shot menu-update A popup "open menu" PARTERRE_PROTOTYPE_ANSWERED=1
shot main-update A full "wait 0.2" PARTERRE_PROTOTYPE_ANSWERED=1
shot privacy A window "open settings:privacy" PARTERRE_PROTOTYPE_ANSWERED=1
shot privacy-dnt A window "open settings:privacy" PARTERRE_PROTOTYPE_ANSWERED=1 DO_NOT_TRACK=1

rm -rf "$work"
