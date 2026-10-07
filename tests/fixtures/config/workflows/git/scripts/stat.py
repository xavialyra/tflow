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


def format_size(size_bytes):
    for unit in ["B", "KB", "MB", "GB"]:
        if size_bytes < 1024.0:
            return f"{size_bytes:.1f} {unit}" if unit != "B" else f"{size_bytes} B"
        size_bytes /= 1024.0
    return f"{size_bytes:.1f} TB"


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        request = {}

    context = request.get("context", {})
    target = extract_target(context)

    if not target:
        print(json.dumps({"version": 1, "output": "Select a file to inspect statistics"}))
        return

    lines = [f"File: {target}"]

    if os.path.exists(target):
        stat = os.stat(target)
        lines.append(f"Size: {format_size(stat.st_size)}")
        lines.append(f"Mode: {oct(stat.st_mode)}")
        try:
            with open(target, "r", errors="ignore") as f:
                lines.append(f"Total Lines: {sum(1 for _ in f)}")
        except Exception:
            pass

    lines.append("")
    lines.append("Diff Statistics (HEAD vs Working Tree):")
    try:
        res = subprocess.run(
            ["git", "--no-pager", "diff", "--stat", "HEAD", "--", target],
            capture_output=True,
            text=True,
            check=True,
        )
        diff_stat = res.stdout.strip()
        lines.append(diff_stat if diff_stat else "  (clean, no working changes)")
    except Exception as e:
        lines.append(f"  Error: {e}")

    lines.append("")
    lines.append("Last Commit Summary:")
    try:
        res = subprocess.run(
            ["git", "--no-pager", "log", "-1", "--stat", "--", target],
            capture_output=True,
            text=True,
            check=True,
        )
        log_stat = res.stdout.strip()
        lines.append(log_stat if log_stat else "  (no commit history)")
    except Exception as e:
        lines.append(f"  Error: {e}")

    print(json.dumps({"version": 1, "output": "\n".join(lines)}))


if __name__ == "__main__":
    main()
