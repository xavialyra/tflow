#!/usr/bin/env python3
"""Capture provider for the sys workflow.

Displays host details, user, kernel, uptime, and selected action details.
"""
import json
import os
import platform
import sys


def get_uptime_string() -> str:
    try:
        with open("/proc/uptime", "r", encoding="ascii") as f:
            total_seconds = float(f.readline().split()[0])
        days = int(total_seconds // 86400)
        hours = int((total_seconds % 86400) // 3600)
        minutes = int((total_seconds % 3600) // 60)
        parts = []
        if days > 0:
            parts.append(f"{days}d")
        if hours > 0 or days > 0:
            parts.append(f"{hours}h")
        parts.append(f"{minutes}m")
        return " ".join(parts)
    except Exception:
        return "unknown"


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        json.dump({"version": 1, "output": None}, sys.stdout)
        sys.stdout.write("\n")
        return

    if request.get("version") != 1:
        json.dump({"version": 1, "output": None}, sys.stdout)
        sys.stdout.write("\n")
        return

    context = request.get("context", {})
    raw_params = context.get("parameters") if isinstance(context, dict) else {}
    if isinstance(raw_params, str):
        try:
            params = json.loads(raw_params)
        except Exception:
            params = {}
    elif isinstance(raw_params, dict):
        params = raw_params
    else:
        params = {}

    item = params.get("item")
    if not isinstance(item, dict) and isinstance(context, dict):
        inp = context.get("input")
        if isinstance(inp, dict):
            item = inp
    if not isinstance(item, dict):
        json.dump({"version": 1, "output": None}, sys.stdout)
        sys.stdout.write("\n")
        return

    metadata = item.get("metadata") or {}
    title = metadata.get("title", item.get("value", ""))
    desc = metadata.get("description", "")

    children = []
    constraints = []

    def add(doc, constraint):
        children.append(doc)
        constraints.append(constraint)

    add({"type": "paragraph", "text": f"Action: {title}"}, {"Length": 1})
    if desc:
        add({"type": "paragraph", "text": desc}, {"Length": 1})

    add({"type": "separator"}, {"Length": 1})

    # Host summary
    host_info = (
        f"Host:    {platform.node()}\n"
        f"User:    {os.environ.get('USER', 'current')}\n"
        f"Kernel:  {platform.release()}\n"
        f"Uptime:  {get_uptime_string()}"
    )
    add({"type": "paragraph", "text": host_info}, {"Fill": 1})

    preview = {
        "type": "layout",
        "direction": "vertical",
        "constraints": constraints,
        "children": children,
    }

    json.dump({"version": 1, "output": preview}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    try:
        main()
    except Exception:
        json.dump({"version": 1, "output": None}, sys.stdout)
        sys.stdout.write("\n")
