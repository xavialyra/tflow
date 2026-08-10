#!/bin/sh
query=$(cat)
source=$(printf '%s\n' "$query" | jq -r '.source // empty' 2>/dev/null)
target=$(printf '%s\n' "$query" | jq -r '.target // empty' 2>/dev/null)
text=$(printf '%s\n' "$query" | jq -r '.text // empty' 2>/dev/null)

if [ -n "$text" ]; then
  if [ -n "$source" ] || [ -n "$target" ]; then
    direction=${source}:${target}
    trans -b -no-ansi "$direction" "$text"
  else
    trans -b -no-ansi "$text"
  fi
else
  printf '%s\n' '[]'
fi |
jq -R -c --argjson query "$query" \
  '{label: ., value: ., metadata: {query: $query}}' |
jq -s '.'
