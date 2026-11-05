#!/usr/bin/env bash
set -eo pipefail
pointer_tool_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
[[ $# == 2 || $# == 3 ]] || { echo 'usage: pc-click.sh <x> <y> [left|right|middle]' >&2; exit 2; }
case ${3:-left} in
  left) pointer_button=0xC0 ;;
  right) pointer_button=0xC1 ;;
  middle) pointer_button=0xC2 ;;
  *) echo 'Unknown mouse button.' >&2; exit 2 ;;
esac
"$pointer_tool_dir/pc-move.sh" "$1" "$2"
source /home/npittas/.agents/skills/computer-use/scripts/pc-env.sh
ydotool click "$pointer_button"
