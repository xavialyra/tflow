#!/usr/bin/env python3
"""Copy the selected application desktop file path to the clipboard."""
import json
import sys


def build_clipboard_command(value: str) -> list[str]:
    shell_cmd = (
        'val="$1"; '
        'if command -v wl-copy >/dev/null 2>&1; then '
        '  printf %s "$val" | setsid --fork --wait wl-copy; '
        'elif command -v xclip >/dev/null 2>&1; then '
        '  printf %s "$val" | setsid --fork --wait xclip -selection clipboard; '
        'elif command -v xsel >/dev/null 2>&1; then '
        '  printf %s "$val" | setsid --fork --wait xsel --clipboard --input; '
        'else '
        '  encoded=$(printf %s "$val" | base64 | tr -d "\\r\\n"); '
        '  printf "\\033]52;c;%s\\007" "$encoded" > /dev/tty 2>/dev/null || true; '
        'fi'
    )
    return ["sh", "-c", shell_cmd, "apps-copy-path", value]


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        raise SystemExit("failed to parse JSON request")

    context = request.get("context", {})
    state = context.get("engine", {}).get("state", {})
    item = state.get("item") if isinstance(state, dict) else None
    value = item.get("value") if isinstance(item, dict) else None
    if not isinstance(value, str) or not value:
        raise SystemExit("requires a selected application")

    response = {
        "version": 1,
        "operation": {
            "type": "run",
            "mode": "foreground",
            "argv": build_clipboard_command(value),
            "exit": False,
            "success_message": f"Copied path: {value}",
        },
    }
    json.dump(response, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
