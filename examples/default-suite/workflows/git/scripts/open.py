#!/usr/bin/env python3
import json
import os
import sys


def extract_path(context):
    state_item = (
        context.get("engine", {})
        .get("state", {})
        .get("item", {})
    )
    if isinstance(state_item, dict):
        meta = state_item.get("meta", {})
        if isinstance(meta, dict) and meta.get("path"):
            return meta["path"]
        if state_item.get("value"):
            return state_item["value"]

    raw_item = context.get("item")
    if isinstance(raw_item, dict):
        meta = raw_item.get("meta", {})
        if isinstance(meta, dict) and meta.get("path"):
            return meta["path"]
        if raw_item.get("value"):
            return raw_item["value"]
    elif isinstance(raw_item, str) and raw_item:
        return raw_item

    return None


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        request = {}

    context = request.get("context", {})
    path = extract_path(context)

    if not path or not os.path.exists(path):
        return

    editor = os.environ.get("EDITOR", "nvim")
    print(json.dumps({
        "version": 1,
        "operation": {
            "type": "run",
            "command": [editor, path],
        },
    }))


if __name__ == "__main__":
    main()
