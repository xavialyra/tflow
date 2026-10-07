#!/usr/bin/env python3
import json
import os
import subprocess
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


def extract_target(item):
    if not item:
        return None, None
    if isinstance(item, str):
        val = item.strip()
        if not val:
            return None, None
        return "value", val
    if not isinstance(item, dict):
        return None, None

    meta = item.get("metadata") or item.get("meta") or {}
    if isinstance(meta, dict):
        if meta.get("path"):
            return "path", str(meta["path"]).strip()
        if meta.get("commit"):
            return "commit", str(meta["commit"]).strip()
        if meta.get("is_commit"):
            return "commit", str(item.get("value", "")).strip()

    val = str(item.get("value", "")).strip()
    if val:
        if len(val) >= 7 and all(c in "0123456789abcdefABCDEF" for c in val):
            return "commit", val
        return "path", val

    return None, None


def main():
    try:
        raw_input = sys.stdin.read()
        request = json.loads(raw_input) if raw_input.strip() else {}
    except Exception:
        request = {}

    item = extract_item(request)
    kind, target = extract_target(item)

    if not target:
        print(json.dumps({"version": 1, "output": "Select a file or commit to preview diff"}))
        return

    output = ""
    if kind == "commit":
        res = subprocess.run(
            ["git", "--no-pager", "show", "--color=always", "--stat", "-p", target],
            capture_output=True,
            text=True,
            check=False,
        )
        if res.returncode == 0:
            output = res.stdout
        else:
            output = res.stderr or f"Could not inspect commit {target}"
    else:
        res = subprocess.run(
            ["git", "--no-pager", "diff", "--color=always", "HEAD", "--", target],
            capture_output=True,
            text=True,
            check=False,
        )
        diff_text = res.stdout if res.returncode == 0 else ""
        if not diff_text and os.path.exists(target):
            res_untracked = subprocess.run(
                ["git", "--no-pager", "diff", "--color=always", "--no-index", "/dev/null", target],
                capture_output=True,
                text=True,
                check=False,
            )
            diff_text = res_untracked.stdout

        if not diff_text and os.path.exists(target):
            try:
                with open(target, "r", errors="replace") as f:
                    diff_text = f.read()
            except Exception:
                pass

        output = diff_text or f"No working changes detected for {target}"

    print(json.dumps({"version": 1, "output": output}))


if __name__ == "__main__":
    main()
