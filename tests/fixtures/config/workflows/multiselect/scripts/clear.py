#!/usr/bin/env python3
import json
import sys


def main():
    request = json.load(sys.stdin)
    context = request.get("context", {})
    parameters = context.get("parameters", {})
    engine = context.get("engine", {})
    state = engine.get("state", {}) if isinstance(engine, dict) else {}
    current_input = state.get("input", "")

    json.dump(
        {
            "version": 1,
            "operation": {
                "type": "navigate",
                "target": "multiselect:main",
                "replace": True,
                "query": {
                    "search": current_input,
                    "selected": [],
                },
            },
        },
        sys.stdout,
        separators=(",", ":"),
    )
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
