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


def format_size(size_bytes):
    for unit in ["B", "KB", "MB", "GB"]:
        if size_bytes < 1024.0:
            return f"{size_bytes:.1f} {unit}" if unit != "B" else f"{size_bytes} B"
        size_bytes /= 1024.0
    return f"{size_bytes:.1f} TB"


def main():
    try:
        raw_input = sys.stdin.read()
        request = json.loads(raw_input) if raw_input.strip() else {}
    except Exception:
        request = {}

    item = extract_item(request)
    target = extract_target(item)

    if not target:
        print(json.dumps({"version": 1, "output": "Select a file to inspect statistics"}))
        return

    lines = [f"Target: {target}"]

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
    res = subprocess.run(
        ["git", "--no-pager", "diff", "--stat", "HEAD", "--", target],
        capture_output=True,
        text=True,
        check=False,
    )
    if res.returncode == 0:
        diff_stat = res.stdout.strip()
        lines.append(diff_stat if diff_stat else "  (clean, no working changes)")
    else:
        lines.append(f"  {res.stderr.strip() or 'No diff info'}")

    lines.append("")
    lines.append("Last Commit Summary:")
    res = subprocess.run(
        ["git", "--no-pager", "log", "-1", "--stat", "--", target],
        capture_output=True,
        text=True,
        check=False,
    )
    if res.returncode == 0:
        log_stat = res.stdout.strip()
        lines.append(log_stat if log_stat else "  (no commit history)")
    else:
        lines.append(f"  {res.stderr.strip() or 'No commit history'}")

    print(json.dumps({"version": 1, "output": "\n".join(lines)}))


if __name__ == "__main__":
    main()
