#!/bin/sh
item=${1-}
case "$item" in
  "Show date") output=$(date 2>&1) ;;
  "Show system information")
    output=$({ uname -a; printf '\n'; id; } 2>&1)
    ;;
  *) output="Unknown system action: $item" ;;
esac
printf '%s\n' "$output" | jq -Rs '.'
