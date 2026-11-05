#!/usr/bin/env bash
set -eo pipefail
source /home/npittas/.agents/skills/computer-use/scripts/pc-env.sh
[[ $# -gt 0 ]] || { echo 'usage: pc-key.sh <key|combo|numeric-event> ...' >&2; exit 2; }
for keyboard_combo in "$@"; do
  if [[ $keyboard_combo =~ ^[0-9]+:[01]$ ]]; then
    ydotool key "$keyboard_combo"
    continue
  fi
  IFS=+ read -r -a keyboard_names <<< "$keyboard_combo"
  keyboard_codes=()
  for keyboard_name in "${keyboard_names[@]}"; do
    case ${keyboard_name,,} in
      ctrl|control) keyboard_name=LEFTCTRL ;;
      shift) keyboard_name=LEFTSHIFT ;;
      alt) keyboard_name=LEFTALT ;;
      super|meta|win) keyboard_name=LEFTMETA ;;
      escape) keyboard_name=ESC ;;
      return) keyboard_name=ENTER ;;
      *) keyboard_name=${keyboard_name^^} ;;
    esac
    keyboard_code=$(awk -v name="KEY_$keyboard_name" '$1 == "#define" && $2 == name { print $3; exit }' /usr/include/linux/input-event-codes.h)
    [[ $keyboard_code =~ ^(0x[0-9a-fA-F]+|[0-9]+)$ ]] || {
      echo "Unknown key: $keyboard_name" >&2; exit 2;
    }
    keyboard_codes+=("$((keyboard_code))")
  done
  keyboard_events=()
  for keyboard_code in "${keyboard_codes[@]}"; do keyboard_events+=("$keyboard_code:1"); done
  for ((keyboard_index=${#keyboard_codes[@]}-1; keyboard_index>=0; keyboard_index--)); do
    keyboard_events+=("${keyboard_codes[keyboard_index]}:0")
  done
  ydotool key "${keyboard_events[@]}"
done
