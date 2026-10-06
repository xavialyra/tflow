#!/usr/bin/env python3
import json
import os
import sys
import time

EXTENSION_ICONS = {
    ".png": "🖼️",
    ".jpg": "🖼️",
    ".jpeg": "🖼️",
    ".gif": "🖼️",
    ".webp": "🖼️",
    ".svg": "🖼️",
    ".bmp": "🖼️",
    ".rs": "⚡",
    ".py": "⚡",
    ".go": "⚡",
    ".c": "⚡",
    ".cpp": "⚡",
    ".js": "⚡",
    ".ts": "⚡",
    ".sh": "⚡",
    ".bash": "⚡",
    ".toml": "⚙️",
    ".json": "⚙️",
    ".yaml": "⚙️",
    ".yml": "⚙️",
    ".xml": "⚙️",
    ".csv": "⚙️",
    ".md": "📝",
    ".txt": "📝",
    ".rst": "📝",
    ".pdf": "📝",
    ".zip": "📦",
    ".tar": "📦",
    ".gz": "📦",
    ".xz": "📦",
    ".7z": "📦",
}

def human_size(size_bytes):
    if size_bytes < 1024:
        return f"{size_bytes} B"
    elif size_bytes < 1024 * 1024:
        return f"{size_bytes / 1024:.1f} KB"
    elif size_bytes < 1024 * 1024 * 1024:
        return f"{size_bytes / (1024 * 1024):.1f} MB"
    else:
        return f"{size_bytes / (1024 * 1024 * 1024):.1f} GB"

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

    current_dir = parameters.get("dir")
    if not current_dir or not isinstance(current_dir, str):
        current_dir = os.getcwd()
    current_dir = os.path.abspath(os.path.expanduser(current_dir))

    search_query = parameters.get("search", "")
    if not isinstance(search_query, str):
        search_query = ""
    search_query = search_query.strip().lower()

    items = []

    parent_dir = os.path.dirname(current_dir)
    if parent_dir != current_dir and not search_query:
        items.append({
            "display": "📁 .. (parent directory)",
            "value": parent_dir,
            "metadata": {
                "name": "..",
                "path": parent_dir,
                "is_dir": True,
                "is_parent": True,
                "target_dir": parent_dir,
            },
        })

    try:
        entries = os.scandir(current_dir)
        dir_entries = []
        file_entries = []

        for entry in entries:
            name = entry.name
            if search_query and search_query not in name.lower():
                continue

            try:
                stat = entry.stat(follow_symlinks=False)
                is_dir = entry.is_dir(follow_symlinks=True)
                size = stat.st_size
                mtime = time.strftime("%Y-%m-%d %H:%M", time.localtime(stat.st_mtime))
            except OSError:
                is_dir = False
                size = 0
                mtime = "-"

            info = {
                "name": name,
                "path": entry.path,
                "is_dir": is_dir,
                "is_parent": False,
                "target_dir": entry.path if is_dir else current_dir,
                "size": size,
                "size_str": human_size(size) if not is_dir else "dir",
                "mtime": mtime,
            }

            if is_dir:
                dir_entries.append(info)
            else:
                file_entries.append(info)

        dir_entries.sort(key=lambda x: x["name"].lower())
        file_entries.sort(key=lambda x: x["name"].lower())

        for d in dir_entries:
            items.append({
                "display": f"📁 {d['name']}/",
                "value": d["path"],
                "metadata": d,
            })

        for f in file_entries:
            ext = os.path.splitext(f["name"])[1].lower()
            icon = EXTENSION_ICONS.get(ext, "📄")
            items.append({
                "display": f"{icon} {f['name']} ({f['size_str']})",
                "value": f["path"],
                "metadata": f,
            })

    except Exception:
        pass

    response = {
        "version": 1,
        "items": items,
    }
    json.dump(response, sys.stdout)
    sys.stdout.write("\n")

if __name__ == "__main__":
    main()
