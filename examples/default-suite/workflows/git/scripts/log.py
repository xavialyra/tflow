#!/usr/bin/env python3
import json
import subprocess
import sys


def extract_target(context):
    state_item = (
        context.get("engine", {})
        .get("state", {})
        .get("item", {})
    )
    if isinstance(state_item, dict):
        meta = state_item.get("metadata") or state_item.get("meta") or {}
        if isinstance(meta, dict) and meta.get("path"):
            return meta["path"]
        if isinstance(meta, dict) and meta.get("commit"):
            return meta["commit"]
        if state_item.get("value"):
            return state_item["value"]

    raw_item = context.get("item")
    if isinstance(raw_item, dict):
        meta = raw_item.get("metadata") or raw_item.get("meta") or {}
        if isinstance(meta, dict) and meta.get("path"):
            return meta["path"]
        if isinstance(meta, dict) and meta.get("commit"):
            return meta["commit"]
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
    target = extract_target(context)

    cmd = [
        "git",
        "--no-pager",
        "log",
        "-n",
        "15",
        "--color=always",
        "--graph",
        "--pretty=format:%C(yellow)%h%Creset -%C(auto)%d%Creset %s %Cgreen(%cr)%Creset %C(bold blue)<%an>%Creset",
    ]
    if target:
        cmd.extend(["--", target])

    try:
        res = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            check=True,
        )
        output = res.stdout or f"No commit history found for {target}"
    except Exception as e:
        output = f"Could not retrieve commit log: {e}"

    print(json.dumps({"version": 1, "output": output}))


if __name__ == "__main__":
    main()
