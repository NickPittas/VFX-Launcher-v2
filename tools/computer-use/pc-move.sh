#!/usr/bin/env bash
# Global desktop coordinates; captures from pc-screenshot.sh use scale 1.
set -eo pipefail
source /home/npittas/.agents/skills/computer-use/scripts/pc-env.sh
[[ $# == 2 && $1 =~ ^-?[0-9]+$ && $2 =~ ^-?[0-9]+$ ]] || {
  echo 'usage: pc-move.sh <x> <y>' >&2; exit 2;
}
pointer_x=$1
pointer_y=$2
pointer_monitors=$(hyprctl monitors -j)
if ! jq -e --argjson x "$pointer_x" --argjson y "$pointer_y" \
  'any(.[]; (.disabled != true) and $x >= .x and $x < (.x + .width / .scale) and $y >= .y and $y < (.y + .height / .scale))' \
  <<< "$pointer_monitors" >/dev/null; then
  echo 'Target is outside the three-screen desktop.' >&2; exit 2
fi
pointer_profile=$(hyprctl getoption input:accel_profile | sed -n 's/^str: //p')
[[ $pointer_profile == flat ]] || {
  echo 'Pointer acceleration must be flat for predictable ydotool coordinates.' >&2; exit 1;
}
pointer_sensitivity=$(hyprctl getoption input:sensitivity | sed -n 's/^float: //p')
read -r pointer_device_x pointer_device_y < <(awk -v x="$pointer_x" -v y="$pointer_y" -v s="$pointer_sensitivity" '
  function round(v) { return v < 0 ? int(v - 0.5) : int(v + 0.5) }
  BEGIN { gain = 1 + s; if (gain <= 0) exit 1; print round(x / gain), round(y / gain) }')
# Flat acceleration still applies sensitivity gain to ydotool absolute motion.
ydotool mousemove --absolute -x "$pointer_device_x" -y "$pointer_device_y"
sleep 0.2
read -r pointer_actual_x pointer_actual_y < <(hyprctl cursorpos | tr -d ',')
pointer_error_x=$((pointer_x - pointer_actual_x))
pointer_error_y=$((pointer_y - pointer_actual_y))
if (( pointer_error_x < -1 || pointer_error_x > 1 || pointer_error_y < -1 || pointer_error_y > 1 )); then
  echo "Pointer missed: requested $pointer_x,$pointer_y; observed $pointer_actual_x,$pointer_actual_y. No click sent." >&2
  exit 1
fi
echo "cursor at $pointer_actual_x, $pointer_actual_y"
