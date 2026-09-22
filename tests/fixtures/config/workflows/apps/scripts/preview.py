#!/usr/bin/env python3
import json
import os
import sys


RASTER_EXTENSIONS = (".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp")
ICON_SIZES = ("48x48", "64x64", "32x32", "128x128", "256x256", "16x16")


def desktop_entry(path):
    values = {}
    if not isinstance(path, str) or not path:
        return values
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as stream:
            in_entry = False
            for raw_line in stream:
                line = raw_line.strip()
                if line == "[Desktop Entry]":
                    in_entry = True
                    continue
                if line.startswith("["):
                    in_entry = False
                    continue
                if not in_entry or not line or line.startswith("#"):
                    continue
                key, separator, value = line.partition("=")
                if separator and key in {"Name", "GenericName", "Comment", "Icon", "Exec"}:
                    values.setdefault(key, value.strip())
    except OSError:
        pass
    return values


def data_roots():
    home = os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share")
    raw_dirs = os.environ.get("XDG_DATA_DIRS") or "/usr/local/share:/usr/share"
    return [home] + [directory for directory in raw_dirs.split(":") if directory]


def is_image_file(path, allow_unknown_extension=False):
    if not os.path.isfile(path):
        return False
    return allow_unknown_extension or os.path.splitext(path)[1].lower() in RASTER_EXTENSIONS


def icon_names(icon):
    filename = os.path.basename(icon)
    stem, extension = os.path.splitext(filename)
    names = []
    if extension.lower() in RASTER_EXTENSIONS:
        names.append(filename)
        stem = stem
    else:
        stem = filename if not extension else stem
    for suffix in RASTER_EXTENSIONS:
        candidate = stem + suffix
        if candidate not in names:
            names.append(candidate)
    return names


def get_cache_dir():
    cache_dir = os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache")
    return os.path.join(cache_dir, "tflow")


def load_icon_cache():
    cache_file = os.path.join(get_cache_dir(), "icon-cache.json")
    if os.path.isfile(cache_file):
        try:
            with open(cache_file, "r", encoding="utf-8") as f:
                data = json.load(f)
                if isinstance(data, dict):
                    return data
        except (OSError, ValueError):
            pass
    return {}


def save_icon_cache(cache):
    if not cache:
        return
    cache_dir = get_cache_dir()
    cache_file = os.path.join(cache_dir, "icon-cache.json")
    try:
        os.makedirs(cache_dir, exist_ok=True)
        tmp = f"{cache_file}.{os.getpid()}"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(cache, f)
        os.replace(tmp, cache_file)
    except OSError:
        pass


def find_icon(icon, desktop_file, cache=None):
    if not isinstance(icon, str) or not icon.strip():
        return None
    icon = os.path.expanduser(icon.strip())
    if cache is not None and icon in cache:
        cached_path = cache[icon]
        if cached_path is None or os.path.isfile(cached_path):
            return cached_path
    result = _find_icon_uncached(icon, desktop_file)
    if cache is not None:
        cache[icon] = result
    return result


def _find_icon_uncached(icon, desktop_file):
    names = icon_names(icon)

    direct_paths = []
    if os.path.isabs(icon):
        direct_paths.append(icon)
    elif "/" in icon:
        direct_paths.extend([
            os.path.join(os.path.dirname(desktop_file), icon),
            os.path.abspath(icon),
        ])
    for candidate in direct_paths:
        extension = os.path.splitext(candidate)[1].lower()
        if is_image_file(candidate, allow_unknown_extension=not extension):
            return os.path.realpath(candidate)

    for root in data_roots():
        icon_root = os.path.join(root, "icons")
        pixmap_root = os.path.join(root, "pixmaps")
        for base in (icon_root, pixmap_root):
            for name in names:
                candidate = os.path.join(base, name)
                if is_image_file(candidate):
                    return os.path.realpath(candidate)
            for size in ICON_SIZES:
                for category in ("apps", "places", "devices", "status", "mimetypes"):
                    for name in names:
                        candidate = os.path.join(base, "hicolor", size, category, name)
                        if is_image_file(candidate):
                            return os.path.realpath(candidate)

        if not os.path.isdir(icon_root):
            continue
        for current, directories, files in os.walk(icon_root, followlinks=False):
            directories.sort()
            for name in names:
                if name in files:
                    candidate = os.path.join(current, name)
                    if is_image_file(candidate):
                        return os.path.realpath(candidate)
    return None


def text_value(value):
    return value if isinstance(value, str) else ""


def main():
    request = json.load(sys.stdin)
    if request.get("version") != 1 or request.get("entrypoint") != "picker-preview":
        raise ValueError("expected a version-1 picker-preview request")

    state = request.get("context", {}).get("engine", {}).get("state", {})
    item = state.get("item") if isinstance(state, dict) else None
    if not isinstance(item, dict):
        json.dump({"version": 1, "preview": None}, sys.stdout)
        sys.stdout.write("\n")
        return

    metadata = item.get("metadata")
    metadata = metadata if isinstance(metadata, dict) else {}
    desktop_file = text_value(metadata.get("desktop_file"))
    entry = desktop_entry(desktop_file)
    name = text_value(item.get("text")) or text_value(entry.get("Name")) or "Application"
    icon_name = text_value(entry.get("Icon"))
    icon_cache = load_icon_cache()
    icon_path = find_icon(icon_name, desktop_file, icon_cache)
    save_icon_cache(icon_cache)

    details = []
    generic_name = text_value(entry.get("GenericName"))
    comment = text_value(entry.get("Comment"))
    exec_command = text_value(entry.get("Exec"))
    if generic_name and generic_name != name:
        details.append("Type: " + generic_name)
    if comment:
        details.append(comment)
    if icon_name:
        details.append("Icon: " + icon_name)
    if exec_command:
        details.append("Command: " + exec_command)
    if desktop_file:
        details.append("Desktop file: " + desktop_file)
    if not details:
        details.append("No additional metadata.")

    body = {
        "type": "paragraph",
        "text": "\n".join(details),
    }
    if icon_path:
        body = {
            "type": "layout",
            "direction": "vertical",
            "constraints": [{"Length": 4}, {"Length": 1}, {"Fill": 1}],
            "children": [
                {"type": "image", "path": icon_path},
                {"type": "separator"},
                body,
            ],
        }

    preview = {
        "type": "layout",
        "direction": "vertical",
        "constraints": [{"Length": 1}, {"Length": 1}, {"Fill": 1}],
        "children": [
            {"type": "display", "display": {"cells": [{"text": name, "slot": "accent"}]}},
            {"type": "separator"},
            body,
        ],
    }
    json.dump({"version": 1, "preview": preview}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"apps preview: {error}", file=sys.stderr)
        raise SystemExit(2)
