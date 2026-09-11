#!/usr/bin/env python3
import json
import sys


def selected_entry(request):
    state = request.get("context", {}).get("engine", {}).get("state", {})
    item = state.get("item") if isinstance(state, dict) else None
    entry = item.get("value") if isinstance(item, dict) else None
    if not isinstance(entry, str) or not entry:
        raise ValueError("select a password with an OTP secret first")
    return entry


def main():
    entry = selected_entry(json.load(sys.stdin))
    json.dump(
        {
            "version": 1,
            "operation": {
                "type": "run",
                "mode": "foreground",
                "argv": ["pass", "otp", "--clip", entry],
                "exit": True,
                "success_message": "OTP copied",
            },
        },
        sys.stdout,
        separators=(",", ":"),
    )
    sys.stdout.write("\n")


if __name__ == "__main__":
    try:
        main()
    except (TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"pass: {error}", file=sys.stderr)
        raise SystemExit(2)
