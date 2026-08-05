command -v fzf >/dev/null 2>&1 || exit 0
cache_file="${XDG_CACHE_HOME:-$HOME/.cache}/tui-launcher/desktop-apps-v2.list"
[ -s "$cache_file" ] || exit 0
exec fzf --filter="$LAUNCHER_QUERY" --no-sort --delimiter=$'\t' --nth=1 --accept-nth=2 < "$cache_file"
