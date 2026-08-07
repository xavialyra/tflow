#!/bin/sh
input=$(cat)
log_file=$(printf '%s\n' "$input" | jq -r '. // empty' 2>/dev/null)
if [ -n "$log_file" ] && [ -r "$log_file" ]; then
    jq -s '.' "$log_file"
else
    printf '%s\n' '[]'
fi
