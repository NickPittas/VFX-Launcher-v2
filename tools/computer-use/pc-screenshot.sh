#!/usr/bin/env bash
set -eo pipefail
source /home/npittas/.agents/skills/computer-use/scripts/pc-env.sh
capture_project_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
capture_path=${1:-"$capture_project_root/artifacts/screenshots/desktop-$(date +%Y%m%d-%H%M%S).png"}
capture_path=$(realpath -m -- "$capture_path")
[[ $capture_path == "$capture_project_root"/* ]] || {
  echo 'Screenshot destination must remain inside the project.' >&2; exit 2;
}
mkdir -p -- "$(dirname -- "$capture_path")"
if [[ -n ${2:-} ]]; then
  grim -s 1 -c -g "$2" "$capture_path"
else
  grim -s 1 -c "$capture_path"
fi
echo "$capture_path"
