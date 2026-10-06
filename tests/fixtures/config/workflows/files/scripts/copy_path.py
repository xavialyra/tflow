#!/usr/bin/env python3
import json
import os
import shutil
import subprocess
import sys

def copy_to_clipboard(text):
    if shutil.which("wl-copy"):
        subprocess.run(["wl-copy"], input=text.encode("utf-8"), check=False)
    elif shutil.which("xclip"):
        subprocess.run(["xclip", "-selection", "clipboard"], input=text.encode("utf-8"), check=False)
    sys.stdout.write(f"\x1b]52;c;{__import__('base64').b64encode(text.encode('utf-8')).decode('ascii')}\x07")
    sys.stdout.flush()

def main():
    try:
        raw = sys.stdin.read()
        request = json.loads(raw) if raw.strip() else {}
    except Exception:
        request = {}

    context = request.get("context", {})
    selection = context.get("selection") or {}
    metadata = selection.get("metadata") or {}

    path = metadata.get("path") or selection.get("value") or ""
    if path:
        copy_to_clipboard(path)

    json.dump({"version": 1, "output": f"Copied path: {path}"}, sys.stdout)
    sys.stdout.write("\n")

if __name__ == "__main__":
    main()
