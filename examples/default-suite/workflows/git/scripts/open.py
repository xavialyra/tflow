#!/usr/bin/env python3
import json
import os
import sys


def extract_item(request):
    context = request.get("context", {})
    params = context.get("parameters", {})
    if isinstance(params, str):
        try:
            params = json.loads(params)
        except Exception:
            params = {}
    if isinstance(params, dict) and params.get("item"):
        return params["item"]
    if context.get("item"):
        return context["item"]
    eng_item = context.get("engine", {}).get("state", {}).get("item")
    if eng_item:
        return eng_item
    return None


def extract_path(item):
    if not item:
        return None
    if isinstance(item, str):
        val = item.strip()
        return val if val else None
    if not isinstance(item, dict):
        return None

    meta = item.get("metadata") or item.get("meta") or {}
    if isinstance(meta, dict) and meta.get("path"):
        return str(meta["path"]).strip()

    val = str(item.get("value", "")).strip()
    return val if val else None


def main():
    try:
        raw_input = sys.stdin.read()
        request = json.loads(raw_input) if raw_input.strip() else {}
    except Exception:
        request = {}

    item = extract_item(request)
    path = extract_path(item)

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
