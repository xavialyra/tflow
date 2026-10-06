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

    current_dir = parameters.get("dir")
    if not current_dir or not isinstance(current_dir, str):
        current_dir = os.getcwd()
    current_dir = os.path.abspath(os.path.expanduser(current_dir))

    parent_dir = os.path.dirname(current_dir)
    response = {
        "version": 1,
        "operation": {
            "type": "navigate",
            "target": "main",
            "clear_input": True,
            "query": {
                "dir": parent_dir,
                "search": "",
            },
        },
    }

    json.dump(response, sys.stdout)
    sys.stdout.write("\n")

if __name__ == "__main__":
    main()
