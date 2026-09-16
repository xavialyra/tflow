#!/usr/bin/env python3
import json
import sys


def main():
    request = json.load(sys.stdin)
    context = request.get("context", {})
    parameters = context.get("parameters", {})
    selected = list(parameters.get("selected", []))

    engine = context.get("engine", {})
    state = engine.get("state", {}) if isinstance(engine, dict) else {}
    current_input = state.get("input", "")
    item = state.get("item")

    if isinstance(item, dict):
        val = item.get("value")
        if val:
            if val in selected:
                selected.remove(val)
            else:
                selected.append(val)

    query = {
        "search": current_input,
        "selected": selected,
    }
    if isinstance(item, dict) and item.get("value"):
        query["__engine"] = {
            "focus": item["value"],
        }

    json.dump(
        {
            "version": 1,
            "operation": {
                "type": "navigate",
                "target": "multiselect:main",
                "replace": True,
                "query": query,
            },
        },
        sys.stdout,
        separators=(",", ":"),
    )
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
