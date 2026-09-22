#!/usr/bin/env python3
import json
import sys


def main():
    state = json.load(sys.stdin)["context"]["engine"]["state"]
    item = state.get("item") or {}
    value = item.get("value")
    if not isinstance(value, str):
        raise SystemExit("calculator requires a selected result")
    response = {
        "version": 1,
        "operation": {
            "type": "run",
            "mode": "foreground",
            "argv": [
                "sh", "-c", 'printf %s "$1" | setsid --fork --wait wl-copy',
                "calculator-copy", value,
            ],
            "exit": False,
            "success_message": "Copied to clipboard",
        },
    }
    print(json.dumps(response, separators=(",", ":")))


if __name__ == "__main__":
    main()
