#!/usr/bin/env python3
"""Restore a selected clipboard history entry to the system clipboard.

Supports Wayland (wl-copy) and X11 (xclip, xsel).
"""
import json
import shutil
import sys


def main():
    request = json.load(sys.stdin)
    state = request.get("context", {}).get("engine", {}).get("state", {})
    item = state.get("item") if isinstance(state, dict) else None
    entry_id = item.get("value") if isinstance(item, dict) else None
    if not isinstance(entry_id, str) or not entry_id:
        raise ValueError("select a clipboard history item first")
    if shutil.which("cliphist") is None:
        raise RuntimeError("restoring clipboard history requires cliphist")

    metadata = item.get("metadata", {})
    mime = metadata.get("mime", "") if isinstance(metadata, dict) else ""
    if mime not in ("image/png", "image/jpeg", "image/gif", "image/webp", "image/bmp"):
        mime = ""

    shell_script = (
        'umask 077; tmp=$(mktemp) || exit; '
        'trap \'rm -f "$tmp"\' EXIT; '
        'cliphist decode "$1" > "$tmp" || exit; '
        'if command -v wl-copy >/dev/null 2>&1; then '
        '  if [ -n "$2" ]; then '
        '    setsid --fork --wait wl-copy --type "$2" < "$tmp"; '
        '  else '
        '    setsid --fork --wait wl-copy < "$tmp"; '
        '  fi; '
        'elif command -v xclip >/dev/null 2>&1; then '
        '  if [ -n "$2" ]; then '
        '    setsid --fork --wait xclip -selection clipboard -t "$2" < "$tmp"; '
        '  else '
        '    setsid --fork --wait xclip -selection clipboard < "$tmp"; '
        '  fi; '
        'elif command -v xsel >/dev/null 2>&1; then '
        '  setsid --fork --wait xsel --clipboard --input < "$tmp"; '
        'else '
        '  encoded=$(base64 < "$tmp" | tr -d "\\r\\n"); '
        '  printf "\\033]52;c;%s\\007" "$encoded" > /dev/tty 2>/dev/null || true; '
        'fi'
    )

    json.dump(
        {
            "version": 1,
            "operation": {
                "type": "run",
                "mode": "foreground",
                "argv": [
                    "sh",
                    "-c",
                    shell_script,
                    "clipboard-history",
                    entry_id,
                    mime,
                ],
                "exit": True,
                "success_message": "Restored to clipboard",
            },
        },
        sys.stdout,
        separators=(",", ":"),
    )
    sys.stdout.write("\n")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"clipboard: {error}", file=sys.stderr)
        raise SystemExit(2)
