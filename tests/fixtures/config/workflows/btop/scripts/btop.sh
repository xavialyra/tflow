#!/bin/sh
set -eu

resource=${1:-cpu}
case "$resource" in
    cpu) boxes="cpu" ;;
    memory) boxes="mem" ;;
    network) boxes="net" ;;
    processes) boxes="proc" ;;
    *)
        printf 'unknown btop resource: %s\n' "$resource" >&2
        exit 64
        ;;
esac

if [ -n "${XDG_CONFIG_HOME:-}" ]; then
    user_config="$XDG_CONFIG_HOME/btop/btop.conf"
elif [ -n "${HOME:-}" ]; then
    user_config="$HOME/.config/btop/btop.conf"
else
    user_config=""
fi

# Preserve the user's btop settings while changing only the visible resource box.
tmp_config=$(mktemp "${TMPDIR:-/tmp}/tflow-btop.XXXXXX")
cleanup() {
    rm -f "$tmp_config"
}
trap cleanup EXIT

if [ -n "$user_config" ] && [ -f "$user_config" ]; then
    awk -v boxes="$boxes" '
        /^[[:space:]]*shown_boxes[[:space:]]*=/ {
            print "shown_boxes = \"" boxes "\""
            found = 1
            next
        }
        { print }
        END {
            if (!found) print "shown_boxes = \"" boxes "\""
        }
    ' "$user_config" > "$tmp_config"
else
    printf 'shown_boxes = "%s"\n' "$boxes" > "$tmp_config"
fi

btop --config "$tmp_config" --force-utf
