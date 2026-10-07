#!/usr/bin/env python3
import json
import subprocess
import sys


def extract_info(context):
    state_item = (
        context.get("engine", {})
        .get("state", {})
        .get("item", {})
    )
    if isinstance(state_item, dict):
        meta = state_item.get("meta", {})
        if isinstance(meta, dict) and meta.get("path"):
            return meta["path"], meta.get("status", "")
        if state_item.get("value"):
            return state_item["value"], ""

    raw_item = context.get("item")
    if isinstance(raw_item, dict):
        meta = raw_item.get("meta", {})
        if isinstance(meta, dict) and meta.get("path"):
            return meta["path"], meta.get("status", "")
        if raw_item.get("value"):
            return raw_item["value"], ""

    return None, ""


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        request = {}

    context = request.get("context", {})
    path, status = extract_info(context)

    if not path:
        return

    is_staged = len(status) >= 1 and status[0] in ["A", "M", "D", "R"]

    if is_staged:
        subprocess.run(["git", "restore", "--staged", "--", path], capture_output=True)
    else:
        subprocess.run(["git", "add", "--", path], capture_output=True)


if __name__ == "__main__":
    main()
