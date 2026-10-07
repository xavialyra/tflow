#!/usr/bin/env python3
import json
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
    if isinstance(meta, dict):
        if meta.get("path"):
            return str(meta["path"]).strip()
        if meta.get("commit"):
            return str(meta["commit"]).strip()

    val = str(item.get("value", "")).strip()
    return val if val else None


def main():
    try:
        raw_input = sys.stdin.read()
        request = json.loads(raw_input) if raw_input.strip() else {}
    except Exception:
        request = {}

    item = extract_item(request)
    target = extract_target(item)

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

    res = subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        check=False,
    )
    if res.returncode == 0:
        output = res.stdout or (f"No commit history found for {target}" if target else "No commits available")
    else:
        output = res.stderr or "Could not retrieve commit log"

    print(json.dumps({"version": 1, "output": output}))


if __name__ == "__main__":
    main()
