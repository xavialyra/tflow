#!/bin/sh
command -v jq >/dev/null 2>&1 || { printf '%s\n' '[]'; exit 0; }
input=$(cat)
log_file=$(printf '%s\n' "$input" | jq -r '.log_file // empty' 2>/dev/null)
query=$(printf '%s\n' "$input" | jq -r '.query // empty' 2>/dev/null)
if [ -n "$log_file" ] && [ -r "$log_file" ]; then
    jq -s --arg query "$query" '
def matches_query($text):
  ($query | ascii_downcase | split(" ") | map(select(length > 0))) as $tokens
  | ($text | ascii_downcase) as $folded
  | all($tokens[]; . as $token | $folded | contains($token));
if $query == "" then . else map(select(matches_query(.label))) end
' "$log_file"
else
    printf '%s\n' '[]'
fi
