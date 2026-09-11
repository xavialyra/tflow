#!/usr/bin/env python3
import json
import os
import sys
import time

def get_cache_file():
    cache_dir = os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache")
    return os.path.join(cache_dir, "tlaunch", "desktop-apps-v3.list")


def get_weights_file():
    state_dir = os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state")
    return os.path.join(state_dir, "tlaunch", "app-weights.json")


def load_weights():
    try:
        with open(get_weights_file(), "r", encoding="utf-8") as f:
            value = json.load(f)
        return value if isinstance(value, dict) else {}
    except (OSError, ValueError):
        return {}


def app_id(path):
    return os.path.basename(path)

def scan_desktop_files():
    data_home = os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share")
    raw_dirs = os.environ.get("XDG_DATA_DIRS") or "/usr/local/share:/usr/share"
    data_dirs = [d for d in raw_dirs.split(":") if d]
    bases = [data_home] + data_dirs

    seen = set()
    lines = []

    for base in bases:
        dir_path = os.path.join(base, "applications")
        if not os.path.isdir(dir_path):
            continue
        try:
            entries = sorted(os.listdir(dir_path))
        except OSError:
            continue
        for fname in entries:
            if not fname.endswith(".desktop"):
                continue
            full_path = os.path.join(dir_path, fname)
            if not os.path.isfile(full_path):
                continue
            try:
                name = None
                with open(full_path, "r", encoding="utf-8", errors="replace") as f:
                    for line in f:
                        if line.startswith("Name="):
                            name = line[5:].strip()
                            break
                if name and full_path not in seen:
                    seen.add(full_path)
                    item_json = json.dumps(
                        {
                            "display": name,
                            "value": full_path,
                            "metadata": {"desktop_file": full_path, "description": name},
                        },
                        separators=(",", ":"),
                    )
                    lines.append(f"{name}\t{item_json}\n")
            except OSError:
                continue

    return lines

def sort_items(items):
    weights = load_weights()
    return sorted(
        items,
        key=lambda item: (
            -(1 if weights.get(app_id(item.get("value", "")), {}).get("pinned", False) else 0),
            -int(weights.get(app_id(item.get("value", "")), {}).get("launch_count", 0) or 0),
            item.get("display", "").casefold(),
        ),
    )


def refresh_cache(cache_file):
    lines = scan_desktop_files()
    try:
        os.makedirs(os.path.dirname(cache_file), exist_ok=True)
        tmp_file = f"{cache_file}.{os.getpid()}"
        with open(tmp_file, "w", encoding="utf-8") as f:
            f.writelines(lines)
        os.replace(tmp_file, cache_file)
    except OSError:
        pass
    return lines

def load_cache(cache_file):
    if os.path.isfile(cache_file):
        try:
            mtime = os.path.getmtime(cache_file)
            if time.time() - mtime < 300:
                with open(cache_file, "r", encoding="utf-8", errors="replace") as f:
                    return f.readlines()
        except OSError:
            pass
    return refresh_cache(cache_file)

def main():
    request = json.load(sys.stdin)
    if request.get("entrypoint") != "picker-items":
        raise ValueError("expected a picker-items producer request")
    context = request.get("context", {})
    engine = context.get("engine", {})
    state = engine.get("state", {}) if isinstance(engine, dict) else {}
    query = state.get("input", "") if isinstance(state, dict) else ""
    if not isinstance(query, str):
        raise ValueError("picker-items engine state input must be a string")
    cache_file = get_cache_file()
    lines = load_cache(cache_file)

    query = query.strip()
    tokens = [t.lower() for t in query.split()] if query else []

    result = []
    for line in lines:
        parts = line.split("\t", 1)
        if len(parts) != 2:
            continue
        name, item_str = parts[0], parts[1].strip()
        if tokens:
            name_lower = name.lower()
            if not all(token in name_lower for token in tokens):
                continue
        try:
            result.append(json.loads(item_str))
        except json.JSONDecodeError:
            continue

    json.dump({"version": 1, "items": sort_items(result)}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")

if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"apps: {error}", file=sys.stderr)
        raise SystemExit(2)
