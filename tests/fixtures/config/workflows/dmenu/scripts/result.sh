#!/bin/sh
command -v python3 >/dev/null 2>&1 || {
  printf '%s\n' 'dmenu workflow requires python3' >&2
  exit 127
}
workflow_dir="${WORKFLOW_DIR:-.}"
exec python3 "$workflow_dir/scripts/dmenu.py" result "$@"
