#!/usr/bin/env python3
"""Capture provider for the calculator workflow.

Renders detailed representation breakdowns (decimal, hex, binary, octal)
and mathematical properties when the preview pane is open.
"""
import json
import sys


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        json.dump({"version": 1, "output": None}, sys.stdout)
        sys.stdout.write("\n")
        return

    if request.get("version") != 1:
        json.dump({"version": 1, "output": None}, sys.stdout)
        sys.stdout.write("\n")
        return

    context = request.get("context", {})
    raw_params = context.get("parameters") if isinstance(context, dict) else {}
    if isinstance(raw_params, str):
        try:
            params = json.loads(raw_params)
        except Exception:
            params = {}
    elif isinstance(raw_params, dict):
        params = raw_params
    else:
        params = {}

    item = params.get("item")
    if not isinstance(item, dict) and isinstance(context, dict):
        inp = context.get("input")
        if isinstance(inp, dict):
            item = inp
    if not isinstance(item, dict):
        json.dump({"version": 1, "output": None}, sys.stdout)
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

    json.dump({"version": 1, "output": preview}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    try:
        main()
    except Exception:
        json.dump({"version": 1, "output": None}, sys.stdout)
        sys.stdout.write("\n")
