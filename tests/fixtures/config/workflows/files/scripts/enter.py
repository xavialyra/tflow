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
    engine = context.get("engine", {}) if isinstance(context, dict) else {}
    state = engine.get("state", {}) if isinstance(engine, dict) else {}

    item = state.get("item") if isinstance(state, dict) else {}
    if not isinstance(item, dict):
        item = {}

    metadata = state.get("metadata") if isinstance(state, dict) else {}
    if not isinstance(metadata, dict) or not metadata:
        metadata = item.get("metadata") if isinstance(item, dict) else {}
    if not isinstance(metadata, dict):
        metadata = {}

    path = metadata.get("path") or state.get("value") or item.get("value") or ""
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
        json.dump(response, sys.stdout)
        sys.stdout.write("\n")
    elif path:
        editor = os.environ.get("EDITOR") or os.environ.get("VISUAL") or "xdg-open"
        response = {
            "version": 1,
            "operation": {
                "type": "run",
                "argv": [editor, path],
            },
        }
        json.dump(response, sys.stdout)
        sys.stdout.write("\n")
    else:
        # Exit cleanly with empty output, which tflow parses as ProtocolOutcome::Noop
        sys.exit(0)

if __name__ == "__main__":
    main()
