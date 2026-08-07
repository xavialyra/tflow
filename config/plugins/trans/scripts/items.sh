#!/bin/sh
input=$(cat)
query=$(printf '%s\n' "$input" | jq -r '. // empty' 2>/dev/null)
case "$query" in
  *" "*) ;;
  *) printf '%s\n' '[]'; exit 0 ;;
esac
text=${query#* }
case "$text" in
  *[![:space:]]*) ;;
  *) printf '%s\n' '[]'; exit 0 ;;
esac
set -f
set -- $query
trans -b -no-ansi "$@" |
jq -R -c --arg query "$query" '{label: ., value: ., metadata: {query: $query}}' |
jq -s '.'
