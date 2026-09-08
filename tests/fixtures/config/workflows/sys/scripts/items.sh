#!/bin/sh
command -v jq >/dev/null 2>&1 || { printf '%s\n' '{"version":1,"items":[]}'; exit 0; }
request=$(cat)
query=$(printf '%s' "$request" | jq -r '.context.engine.state.input // ""')
jq -cn --arg query "$query" '
def matches_query($text):
  ($query | ascii_downcase | split(" ") | map(select(length > 0))) as $tokens
  | ($text | ascii_downcase) as $folded
  | all($tokens[]; . as $token | $folded | contains($token));
[
  {display: "Show date"},
  {display: "Show system information"}
]
| if $query == "" then . else map(select(matches_query(.display))) end
| {version: 1, items: .}
'
