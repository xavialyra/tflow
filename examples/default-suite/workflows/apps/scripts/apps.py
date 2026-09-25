#!/usr/bin/env python3
"""Desktop application scanner and search provider.

Scans standard XDG application directories, indexes Name, GenericName,
Keywords, and Comments, and provides ranked fuzzy token search with launch
weight weighting.
"""
import json
import os
import sys
import time


def get_cache_file():
    cache_dir = os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache")
    return os.path.join(cache_dir, "tflow", "desktop-apps-v4.list")


def get_weights_file():
    state_dir = os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state")
    return os.path.join(state_dir, "tflow", "app-weights.json")


def load_weights():
    try:
        with open(get_weights_file(), "r", encoding="utf-8") as f:
            value = json.load(f)
        return value if isinstance(value, dict) else {}
    except (OSError, ValueError):
        return {}


def app_id(path):
    return os.path.basename(path)


def parse_desktop_file(path: str) -> dict | None:
    """Parse essential keys from a .desktop file, respecting [Desktop Entry]."""
    data = {}
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as f:
            in_section = False
            for line in f:
                line = line.strip()
                if line == "[Desktop Entry]":
                    in_section = True
                    continue
                if line.startswith("["):
                    in_section = False
                    continue
                if not in_section or not line or line.startswith("#"):
                    continue
                if "=" not in line:
                    continue
                k, v = line.split("=", 1)
                k = k.strip()
                v = v.strip()
                if k not in data:
                    data[k] = v
    except OSError:
        return None

    # Hidden or NoDisplay entries should not be presented in launchers
    if data.get("NoDisplay", "").lower() == "true":
        return None
    if data.get("Hidden", "").lower() == "true":
        return None
    if data.get("Type", "Application") != "Application":
        return None

    name = data.get("Name")
    if not name:
        return None

    return {
        "name": name,
        "generic_name": data.get("GenericName", ""),
        "comment": data.get("Comment", ""),
        "keywords": data.get("Keywords", "").replace(";", " "),
        "exec": data.get("Exec", ""),
        "terminal": data.get("Terminal", "").lower() == "true",
        "icon": data.get("Icon", ""),
    }


def scan_desktop_files():
    data_home = os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share")
    raw_dirs = os.environ.get("XDG_DATA_DIRS") or "/usr/local/share:/usr/share"
    data_dirs = [d for d in raw_dirs.split(":") if d]
    bases = [data_home] + data_dirs

    seen_ids = set()
    records = []

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
            if fname in seen_ids:
                # Earlier directory takes precedence over later in XDG hierarchy
                continue

            full_path = os.path.join(dir_path, fname)
            if not os.path.isfile(full_path):
                continue

            info = parse_desktop_file(full_path)
            if not info:
                continue

            seen_ids.add(fname)

            desc = info["generic_name"] or info["comment"] or info["name"]
            item_obj = {
                "display": info["name"],
                "value": full_path,
                "metadata": {
                    "desktop_file": full_path,
                    "description": desc,
                    "generic_name": info["generic_name"],
                    "comment": info["comment"],
                    "keywords": info["keywords"],
                },
            }

            # Pre-combine search keywords: Name + GenericName + Keywords + Comment
            searchable = (
                f"{info['name']} {info['generic_name']} {info['keywords']} {info['comment']}"
            ).lower()

            record = {
                "name": info["name"],
                "searchable": searchable,
                "item": item_obj,
            }
            records.append(json.dumps(record, separators=(",", ":")) + "\n")

    return records


def get_weight(entry):
    weight = entry.get("weight")
    if weight is not None:
        try:
            return int(weight)
        except (TypeError, ValueError):
            pass
    return int(entry.get("launch_count", 0) or 0)


def rank_item(item_obj, query, weights):
    fname = app_id(item_obj.get("value", ""))
    weight_entry = weights.get(fname, {})
    pinned = 1 if weight_entry.get("pinned", False) else 0
    weight = get_weight(weight_entry)

    name = item_obj.get("display", "").lower()
    q = query.lower()

    # Exact or prefix match boost
    name_score = 0
    if q:
        if name == q:
            name_score = 100
        elif name.startswith(q):
            name_score = 50
        elif q in name:
            name_score = 20

    return (-pinned, -name_score, -weight, name)


def refresh_cache(cache_file):
    records = scan_desktop_files()
    try:
        os.makedirs(os.path.dirname(cache_file), exist_ok=True)
        tmp_file = f"{cache_file}.{os.getpid()}"
        with open(tmp_file, "w", encoding="utf-8") as f:
            f.writelines(records)
        os.replace(tmp_file, cache_file)
    except OSError:
        pass
    return records


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

    weights = load_weights()
    result = []

    for line in lines:
        try:
            record = json.loads(line)
        except json.JSONDecodeError:
            continue

        if tokens:
            searchable = record.get("searchable", "")
            if not all(token in searchable for token in tokens):
                continue

        result.append(record["item"])

    # Rank results
    result.sort(key=lambda item: rank_item(item, query, weights))

    json.dump({"version": 1, "items": result}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"apps: {error}", file=sys.stderr)
        raise SystemExit(2)
