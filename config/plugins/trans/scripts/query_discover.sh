command -v trans >/dev/null 2>&1 || exit 0
command -v jq >/dev/null 2>&1 || exit 0
case "$LAUNCHER_QUERY" in
  *" "*) ;;
  *) exit 0 ;;
esac
text="${LAUNCHER_QUERY#* }"
[ -n "${text//[[:space:]]/}" ] || exit 0
read -r -a args <<< "$LAUNCHER_QUERY"
trans -b -no-ansi "${args[@]}" |
jq -R -c --arg query "$LAUNCHER_QUERY" '{label: ., value: ., metadata: {query: $query}}'
