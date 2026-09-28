#!/usr/bin/env python3
"""Copy the selected calculator result to the system clipboard.

Supports Wayland (wl-copy) and X11 (xclip, xsel), with automatic fallback
to OSC 52 terminal clipboard escape sequences.
"""
import json
import shutil
import sys


def build_clipboard_command(value: str) -> list[str]:
    """Build a command pipeline that copies value to the clipboard with fallbacks."""
    # We craft a shell script that checks available clipboard tools in order:
    # 1. wl-copy (Wayland)
    # 2. xclip (X11)
    # 3. xsel (X11)
    # 4. OSC 52 terminal escape sequence (works in modern terminals over SSH/local)
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
    return ["sh", "-c", shell_cmd, "calculator-copy", value]


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        raise SystemExit("failed to parse JSON request")

    state = request.get("context", {}).get("engine", {}).get("state", {})
    item = state.get("item") or {}
    value = item.get("value")
    if not isinstance(value, str) or not value:
        raise SystemExit("calculator requires a selected result")

    response = {
        "version": 1,
        "operation": {
            "type": "run",
            "argv": build_clipboard_command(value),
            "exit": False,
            "success_message": f"Copied '{value}' to clipboard",
        },
    }
    print(json.dumps(response, separators=(",", ":")))


if __name__ == "__main__":
    main()
