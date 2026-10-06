#!/usr/bin/env python3
import hashlib
import json
import os
import shutil
import subprocess
import sys

IMAGE_EXTENSIONS = {".png", ".jpg", ".jpeg", ".webp", ".bmp", ".gif"}

def is_text_file(path):
    try:
        with open(path, "rb") as f:
            chunk = f.read(1024)
            return b"\0" not in chunk
    except Exception:
        return False

def format_directory_preview(path):
    lines = []
    try:
        entries = list(os.scandir(path))
        entries.sort(key=lambda e: (not e.is_dir(), e.name.lower()))
        total = len(entries)
        lines.append(f"Total entries: {total}")
        lines.append("")
        for entry in entries[:18]:
            tag = "📁" if entry.is_dir() else "📄"
            lines.append(f"{tag} {entry.name}")
        if total > 18:
            lines.append(f"... and {total - 18} more")
    except Exception as e:
        lines.append(f"Could not read directory: {e}")
    return "\n".join(lines)

def format_text_preview(path, max_lines=60):
    lines = []
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as f:
            for idx, line in enumerate(f, start=1):
                if idx > max_lines:
                    lines.append(f"... (truncated after {max_lines} lines)")
                    break
                clean_line = line.rstrip("\r\n")
                lines.append(f"{idx:3d} │ {clean_line}")
    except Exception as e:
        lines.append(f"Error reading file: {e}")
    return "\n".join(lines)

def format_binary_preview(path, size):
    lines = []
    file_info = "Unknown binary file"
    if shutil.which("file"):
        try:
            res = subprocess.run(["file", "-b", path], capture_output=True, text=True, check=False)
            if res.stdout.strip():
                file_info = res.stdout.strip()
        except Exception:
            pass

    sha256 = ""
    try:
        hasher = hashlib.sha256()
        with open(path, "rb") as f:
            hasher.update(f.read(65536))
        sha256 = hasher.hexdigest()[:16] + "..."
    except Exception:
        pass

    lines.append(f"Type: {file_info}")
    lines.append(f"Size: {size} bytes")
    if sha256:
        lines.append(f"SHA-256 (head): {sha256}")
    return "\n".join(lines)

def main():
    try:
        raw = sys.stdin.read()
        request = json.loads(raw) if raw.strip() else {}
    except Exception:
        request = {}

    context = request.get("context", {})
    parameters = context.get("parameters", {})
    if isinstance(parameters, str):
        try:
            parameters = json.loads(parameters)
        except Exception:
            parameters = {}

    item = parameters.get("item")
    if not isinstance(item, dict):
        inp = context.get("input")
        if isinstance(inp, dict):
            item = inp

    if not isinstance(item, dict) or not item:
        json.dump({"version": 1, "output": None}, sys.stdout)
        sys.stdout.write("\n")
        return

    metadata = item.get("metadata") or {}
    path = metadata.get("path") or item.get("value") or ""
    name = metadata.get("name") or os.path.basename(path)
    is_dir = metadata.get("is_dir", False)
    size = metadata.get("size", 0)
    size_str = metadata.get("size_str", "")
    mtime = metadata.get("mtime", "")

    children = []
    constraints = []

    def add(doc, constraint):
        children.append(doc)
        constraints.append(constraint)

    if not path or not os.path.exists(path):
        json.dump({"version": 1, "output": None}, sys.stdout)
        sys.stdout.write("\n")
        return

    ext = os.path.splitext(name)[1].lower()

    if is_dir:
        add({"type": "paragraph", "text": f"📁 Directory: {name}\nPath: {path}\nModified: {mtime}"}, {"Length": 3})
        add({"type": "separator"}, {"Length": 1})
        body = format_directory_preview(path)
        add({"type": "paragraph", "text": body}, {"Fill": 1})
    elif ext in IMAGE_EXTENSIONS:
        add({"type": "paragraph", "text": f"🖼️ Image: {name} ({size_str})\nPath: {path}\nModified: {mtime}"}, {"Length": 3})
        add({"type": "separator"}, {"Length": 1})
        add({"type": "image", "path": path}, {"Fill": 1})
    elif is_text_file(path):
        add({"type": "paragraph", "text": f"📄 Text: {name} ({size_str})\nPath: {path}\nModified: {mtime}"}, {"Length": 3})
        add({"type": "separator"}, {"Length": 1})
        body = format_text_preview(path)
        add({"type": "paragraph", "text": body}, {"Fill": 1})
    else:
        add({"type": "paragraph", "text": f"⚙️ Binary: {name} ({size_str})\nPath: {path}\nModified: {mtime}"}, {"Length": 3})
        add({"type": "separator"}, {"Length": 1})
        body = format_binary_preview(path, size)
        add({"type": "paragraph", "text": body}, {"Fill": 1})

    preview = {
        "type": "layout",
        "direction": "vertical",
        "constraints": constraints,
        "children": children,
    }

    json.dump({"version": 1, "output": preview}, sys.stdout)
    sys.stdout.write("\n")

if __name__ == "__main__":
    main()
