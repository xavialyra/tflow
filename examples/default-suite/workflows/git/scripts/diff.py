#!/usr/bin/env python3
import json
import os
import subprocess
import sys


def extract_target(context):
    state_item = (
        context.get("engine", {})
        .get("state", {})
        .get("item", {})
    )
    if isinstance(state_item, dict):
        meta = state_item.get("meta", {})
        if isinstance(meta, dict) and meta.get("path"):
            return "path", meta["path"]
        if isinstance(meta, dict) and meta.get("commit"):
            return "commit", meta["commit"]
        if state_item.get("value"):
            return "value", state_item["value"]

    raw_item = context.get("item")
    if isinstance(raw_item, dict):
        meta = raw_item.get("meta", {})
        if isinstance(meta, dict) and meta.get("path"):
            return "path", meta["path"]
        if isinstance(meta, dict) and meta.get("commit"):
            return "commit", meta["commit"]
        if raw_item.get("value"):
            return "value", raw_item["value"]
    elif isinstance(raw_item, str) and raw_item:
        return "value", raw_item

    return None, None


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        request = {}

    context = request.get("context", {})
    kind, target = extract_target(context)

    if not target:
        print(json.dumps({"version": 1, "output": "Select a file or commit to preview diff"}))
        return

    output = ""
    if kind == "commit":
        try:
            res = subprocess.run(
                ["git", "--no-pager", "show", "--color=always", "--stat", "-p", target],
                capture_output=True,
                text=True,
                check=True,
            )
            output = res.stdout
        except Exception as e:
            output = f"Could not inspect commit {target}: {e}"
    else:
        try:
            res = subprocess.run(
                ["git", "--no-pager", "diff", "--color=always", "HEAD", "--", target],
                capture_output=True,
                text=True,
                check=True,
            )
            diff_text = res.stdout
            if not diff_text and os.path.exists(target):
                try:
                    res_untracked = subprocess.run(
                        ["git", "--no-pager", "diff", "--color=always", "--no-index", "/dev/null", target],
                        capture_output=True,
                        text=True,
                    )
                    diff_text = res_untracked.stdout
                except Exception:
                    pass

            if not diff_text and os.path.exists(target):
                with open(target, "r", errors="replace") as f:
                    diff_text = f.read()

            output = diff_text or f"No diff detected for {target}"
        except Exception as e:
            output = f"Could not compute diff for {target}: {e}"

    print(json.dumps({"version": 1, "output": output}))


if __name__ == "__main__":
    main()
