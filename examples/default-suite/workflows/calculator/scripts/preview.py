#!/usr/bin/env python3
"""Picker preview provider for the calculator workflow.

Renders detailed representation breakdowns (decimal, hex, binary, octal)
and mathematical properties when the preview pane is open.
"""
import json
import sys


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        raise ValueError("failed to parse JSON request")

    if request.get("version") != 1 or request.get("entrypoint") != "picker-preview":
        raise ValueError("expected a version-1 picker-preview request")

    state = request.get("context", {}).get("engine", {}).get("state", {})
    item = state.get("item")
    if not isinstance(item, dict):
        json.dump({"version": 1, "preview": None}, sys.stdout)
        sys.stdout.write("\n")
        return

    metadata = item.get("metadata") or {}
    value = item.get("value", "")

    children = []
    constraints = []

    def add(doc, constraint):
        children.append(doc)
        constraints.append(constraint)

    add({"type": "paragraph", "text": f"Result: {value}"}, {"Length": 1})
    add({"type": "separator"}, {"Length": 1})

    item_type = metadata.get("type")
    if item_type == "integer":
        dec = metadata.get("decimal", value)
        hex_val = metadata.get("hex", "")
        bin_val = metadata.get("bin", "")
        oct_val = metadata.get("oct", "")

        conversions = (
            f"Decimal:     {dec}\n"
            f"Hexadecimal: {hex_val}\n"
            f"Binary:      {bin_val}\n"
            f"Octal:       {oct_val}"
        )
        add({"type": "paragraph", "text": conversions}, {"Fill": 1})
    else:
        dec = metadata.get("decimal", value)
        details = f"Decimal:     {dec}\nType:        Floating point"
        add({"type": "paragraph", "text": details}, {"Fill": 1})

    preview = {
        "type": "layout",
        "direction": "vertical",
        "constraints": constraints,
        "children": children,
    }

    json.dump({"version": 1, "preview": preview}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
