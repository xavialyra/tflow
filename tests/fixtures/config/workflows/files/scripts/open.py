#!/usr/bin/env python3
import json
import os
import sys

def main():
    try:
        raw = sys.stdin.read()
        request = json.loads(raw) if raw.strip() else {}
    except Exception:
        request = {}

    context = request.get("context", {})
    parameters = context.get("parameters", {})
    if isinstance(parameters, str):
        try:
            parameters = json.loads(parameters)
        except Exception:
            parameters = {}

    selection = context.get("selection") or {}
    metadata = selection.get("metadata") or {}

    path = metadata.get("path") or selection.get("value") or ""
    is_dir = metadata.get("is_dir")
    if is_dir is None and path:
        is_dir = os.path.isdir(path)

    if is_dir and path:
        response = {
            "version": 1,
            "operation": {
                "type": "navigate",
                "target": "main",
                "clear_input": True,
                "query": {
                    "dir": path,
                    "search": "",
                },
            },
        }
    elif path:
        editor = os.environ.get("EDITOR") or os.environ.get("VISUAL") or "xdg-open"
        response = {
            "version": 1,
            "operation": {
                "type": "run",
                "argv": [editor, path],
            },
        }
    else:
        response = {"version": 1, "output": None}

    json.dump(response, sys.stdout)
    sys.stdout.write("\n")

if __name__ == "__main__":
    main()
