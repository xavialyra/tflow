#!/bin/sh
query=${1:-{}}

# Fast path for empty text in query
if [ -z "$query" ] || [ "$query" = "{}" ] || ! printf '%s' "$query" | grep -q '"text"[[:space:]]*:[[:space:]]*"[^"]'; then
  printf '[]\n'
  exit 0
fi

IFS='	' read -r source target text << EOF
$(printf '%s' "$query" | jq -r '[(.source // ""), (.target // ""), (.text // "")] | @tsv' 2>/dev/null)
EOF

if [ -z "$text" ]; then
  printf '[]\n'
  exit 0
fi

if [ -n "$source" ] || [ -n "$target" ]; then
  direction=${source}:${target}
  trans -b -no-ansi "$direction" "$text"
else
  trans -b -no-ansi "$text"
fi |
jq -R -c --argjson query "$query" \
  '{display: ., value: ., metadata: {query: $query}}' |
jq -s '.'
