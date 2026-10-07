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


def extract_info(item):
    if not item:
        return None, ""
    if isinstance(item, str):
        val = item.strip()
        return (val, "") if val else (None, "")
    if not isinstance(item, dict):
        return None, ""

    meta = item.get("metadata") or item.get("meta") or {}
    if isinstance(meta, dict) and meta.get("path"):
        return str(meta["path"]).strip(), str(meta.get("status", ""))

    val = str(item.get("value", "")).strip()
    return (val, "") if val else (None, "")


def main():
    try:
        raw_input = sys.stdin.read()
        request = json.loads(raw_input) if raw_input.strip() else {}
    except Exception:
        request = {}

    item = extract_item(request)
    path, status = extract_info(item)

    if not path:
        return

    is_staged = len(status) >= 1 and status[0] in ["A", "M", "D", "R"]

    if is_staged:
        subprocess.run(["git", "restore", "--staged", "--", path], capture_output=True, check=False)
    else:
        subprocess.run(["git", "add", "--", path], capture_output=True, check=False)


if __name__ == "__main__":
    main()
