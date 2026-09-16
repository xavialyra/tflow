#!/usr/bin/env python3
import json
import sys


def main():
    request = json.load(sys.stdin)
    if request.get("entrypoint") != "picker-preview":
        raise ValueError("expected a picker-preview producer request")

    context = request.get("context", {})
    parameters = context.get("parameters", {})
    selected = parameters.get("selected", [])

    engine = context.get("engine", {})
    state = engine.get("state", {}) if isinstance(engine, dict) else {}
    item = state.get("item")

    children = []

    if isinstance(item, dict):
        metadata = item.get("metadata", {})
        name = metadata.get("name", item.get("text", "Unknown"))
        desc = metadata.get("desc", "")
        is_sel = metadata.get("selected", False)

        children.append({
            "type": "paragraph",
            "spans": [
                {"text": f"{name}\n", "slot": "checked"},
                {
                    "text": (
                        f"Status: {'[✓] Selected' if is_sel else '[ ] Not selected'}\n"
                    ),
                    "slot": "checked" if is_sel else "unchecked",
                },
                {"text": f"{desc}\n", "slot": "secondary"},
            ],
            "title": "Item Details",
            "border": True,
        })
    else:
        children.append({
            "type": "paragraph",
            "text": "No item highlighted",
            "title": "Item Details",
            "border": True,
        })

    summary_text = (
        f"Selected ({len(selected)} items):\n"
        + (", ".join(selected) if selected else "(none)")
        + "\n\nKeybindings:\n"
        + "  Tab:    Toggle selection\n"
        + "  Ctrl+A: Select all\n"
        + "  Ctrl+R: Clear all\n"
        + "  Enter:  Confirm selection"
    )

    children.append({
        "type": "paragraph",
        "text": summary_text,
        "title": "Multi-Select Summary",
        "border": True,
    })

    doc = {
        "version": 1,
        "preview": {
            "type": "layout",
            "direction": "vertical",
            "constraints": [{"Length": 6}, {"Fill": 1}],
            "children": children,
        },
    }

    json.dump(doc, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
