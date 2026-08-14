#!/bin/sh
command -v fzf >/dev/null 2>&1 || { printf '%s\n' '[]'; exit 0; }
command -v jq >/dev/null 2>&1 || { printf '%s\n' '[]'; exit 0; }
input=$(cat)
query=$(printf '%s\n' "$input" | jq -r '. // empty' 2>/dev/null)
cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/tui-launcher"
cache_file="$cache_dir/desktop-apps-v2.list"
refresh=true
if [ -s "$cache_file" ]; then
  cache_mtime=$(stat -c %Y "$cache_file" 2>/dev/null || printf '0')
  now=$(date +%s)
  [ "$((now - cache_mtime))" -lt 300 ] && refresh=false
fi
if $refresh; then
  mkdir -p "$cache_dir" || { printf '%s\n' '[]'; exit 0; }
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
      item=$(jq -cn --arg label "$name" --arg value "$file" \
        '{label: $label, value: $value, metadata: {desktop_file: $value}}')
      printf '%s\t%s\n' "$name" "$item"
    fi
  done > "$tmp_file"
  mv "$tmp_file" "$cache_file" || { printf '%s\n' '[]'; exit 0; }
fi
fzf --filter="$query" --no-sort --delimiter="$(printf '\t')" --nth=1 --accept-nth=2 < "$cache_file" |
jq -s '.'
