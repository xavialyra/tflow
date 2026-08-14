#!/bin/sh
command -v jq >/dev/null 2>&1 || { printf '%s\n' '[]'; exit 0; }
input=$(cat)
query=$(printf '%s\n' "$input" | jq -r '. // empty' 2>/dev/null)
jq -cn --arg query "$query" '
def matches_query($text):
  ($query | ascii_downcase | split(" ") | map(select(length > 0))) as $tokens
  | ($text | ascii_downcase) as $folded
  | all($tokens[]; . as $token | $folded | contains($token));
[
  {label: "Open shell"}
]
| if $query == "" then . else map(select(matches_query(.label))) end
'
