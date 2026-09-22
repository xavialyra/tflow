#!/usr/bin/env python3
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
    if (
        shutil.which("cliphist") is None
        or shutil.which("wl-copy") is None
        or shutil.which("setsid") is None
    ):
        raise RuntimeError(
            "restoring clipboard history requires cliphist, wl-copy, and setsid"
        )

    metadata = item.get("metadata", {})
    mime = metadata.get("mime", "") if isinstance(metadata, dict) else ""
    if mime not in ("image/png", "image/jpeg", "image/gif", "image/webp", "image/bmp"):
        mime = ""

    json.dump(
        {
            "version": 1,
            "operation": {
                "type": "run",
                "mode": "foreground",
                "argv": [
                    "sh",
                    "-c",
                    'umask 077; tmp=$(mktemp) || exit; '
                    'trap \'rm -f "$tmp"\' EXIT; '
                    'cliphist decode "$1" > "$tmp" || exit; '
                    'if [ -n "$2" ]; then '
                    'setsid --fork --wait wl-copy --type "$2" < "$tmp"; '
                    'else setsid --fork --wait wl-copy < "$tmp"; fi',
                    "clipboard-history",
                    entry_id,
                    mime,
                ],
                "exit": True,
                "success_message": "Copied to clipboard",
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
