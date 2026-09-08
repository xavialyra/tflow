#!/bin/sh
query=${1:-}
workflow_dir="${WORKFLOW_DIR:-.}"
if command -v python3 >/dev/null 2>&1; then
  if [ -f "$workflow_dir/scripts/apps.py" ]; then
    exec python3 "$workflow_dir/scripts/apps.py"
  fi
fi

command -v fzf >/dev/null 2>&1 || { printf '%s\n' '{"version":1,"items":[]}'; exit 0; }
command -v jq >/dev/null 2>&1 || { printf '%s\n' '{"version":1,"items":[]}'; exit 0; }
cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/tui-launcher"
cache_file="$cache_dir/desktop-apps-v3.list"
refresh=true
if [ -s "$cache_file" ]; then
  cache_mtime=$(stat -c %Y "$cache_file" 2>/dev/null || printf '0')
  now=$(date +%s)
  [ "$((now - cache_mtime))" -lt 300 ] && refresh=false
fi
if $refresh; then
  mkdir -p "$cache_dir" || { printf '%s\n' '{"version":1,"items":[]}'; exit 0; }
  tmp_file="$cache_file.$$"
  trap 'rm -f "$tmp_file"' EXIT
  data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
  data_dirs="${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
  for base in "$data_home" $(printf '%s' "$data_dirs" | tr ':' ' '); do
    dir="$base/applications"
    [ -d "$dir" ] || continue
    find "$dir" -maxdepth 1 -type f -name '*.desktop' -print
  done |
  while IFS= read -r file; do
    name=$(sed -n 's/^Name=//p' "$file" | head -n 1)
    if [ -n "$name" ]; then
      item=$(jq -cn --arg display "$name" --arg value "$file" \
        '{display: $display, value: $value, metadata: {desktop_file: $value}}')
      printf '%s\t%s\n' "$name" "$item"
    fi
  done > "$tmp_file"
  mv "$tmp_file" "$cache_file" || { printf '%s\n' '{"version":1,"items":[]}'; exit 0; }
fi
fzf --filter="$query" --no-sort --delimiter="$(printf '\t')" --nth=1 --accept-nth=2 < "$cache_file" |
jq -s '{version: 1, items: .}'
