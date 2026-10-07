#!/usr/bin/env python3
import json
import subprocess
import sys


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        request = {}

    query_str = (
        request.get("context", {})
        .get("engine", {})
        .get("state", {})
        .get("input", "")
        .strip()
    )

    try:
        res = subprocess.run(
            ["git", "status", "--porcelain", "-uall"],
            capture_output=True,
            text=True,
            check=True,
        )
        lines = res.stdout.splitlines()
    except Exception:
        lines = []

    items = []
    for line in lines:
        if len(line) < 4:
            continue
        status = line[:2].strip()
        path = line[3:].strip()
        if " -> " in path:
            path = path.split(" -> ")[1].strip()

        if query_str and query_str.lower() not in path.lower():
            continue

        if "?" in status:
            icon = "❓"
        elif "A" in status:
            icon = "➕"
        elif "M" in status:
            icon = "📝"
        elif "D" in status:
            icon = "🗑️"
        elif "R" in status:
            icon = "🔄"
        else:
            icon = "📄"

        items.append({
            "display": f"{icon} {path} [{status}]",
            "value": path,
            "metadata": {
                "path": path,
                "status": status,
                "is_commit": False,
            },
        })

    if not items and not query_str:
        try:
            log_res = subprocess.run(
                ["git", "log", "-n", "15", "--oneline"],
                capture_output=True,
                text=True,
                check=True,
            )
            log_lines = log_res.stdout.splitlines()
            for l in log_lines:
                parts = l.split(" ", 1)
                commit_hash = parts[0]
                subject = parts[1] if len(parts) > 1 else ""
                items.append({
                    "display": f"🔖 {commit_hash} {subject}",
                    "value": commit_hash,
                    "metadata": {
                        "commit": commit_hash,
                        "subject": subject,
                        "is_commit": True,
                    },
                })
        except Exception:
            pass

    if not items:
        items.append({
            "display": "Working tree clean",
            "value": "",
            "metadata": {},
        })

    print(json.dumps({"version": 1, "items": items}))


if __name__ == "__main__":
    main()
