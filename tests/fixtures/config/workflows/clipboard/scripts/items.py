#!/usr/bin/env python3
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile

MAX_PREVIEW_CHARS = 16_384


def run(argv):
    return subprocess.run(argv, capture_output=True, check=False)


def compact(value, limit):
    value = " ".join(value.split())
    return value if len(value) <= limit else f"{value[:limit - 1]}…"


def image_format(data):
    if data.startswith(b"\x89PNG\r\n\x1a\n"):
        return "png"
    if data.startswith(b"\xff\xd8\xff"):
        return "jpeg"
    if data.startswith((b"GIF87a", b"GIF89a")):
        return "gif"
    if data.startswith(b"RIFF") and data[8:12] == b"WEBP":
        return "webp"
    if data.startswith(b"BM"):
        return "bmp"
    return None


def cache_directory():
    # Keep clipboard content private, including the search index.
    root = Path(os.environ.get("XDG_RUNTIME_DIR") or tempfile.gettempdir())
    directory = root / f"tflow-clipboard-{os.getuid()}"
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    stat = directory.lstat()
    if directory.is_symlink() or stat.st_uid != os.getuid() or stat.st_mode & 0o077:
        raise RuntimeError("clipboard preview cache must be a private directory")
    return directory


def cache_image(data, extension):
    directory = cache_directory()
    path = directory / f"{hashlib.sha256(data).hexdigest()}.{extension}"
    if not path.exists():
        with tempfile.NamedTemporaryFile(dir=directory, delete=False) as output:
            temporary = Path(output.name)
            try:
                output.write(data)
                output.close()
                temporary.replace(path)
            finally:
                temporary.unlink(missing_ok=True)
    return str(path)


def size_label(size):
    if size < 1024:
        return f"{size} B"
    if size < 1024 * 1024:
        return f"{size / 1024:.1f} KiB"
    return f"{size / (1024 * 1024):.1f} MiB"


def format_display(item):
    icons = {"IMAGE": "▧", "TEXT": "Ｔ", "FILE": "□"}
    rows = item["display"]["rows"]
    kind = item["metadata"]["title"].split(" · ", 1)[0]
    rows[0]["cells"][0]["text"] = icons.get(kind, "□")
    rows[0]["cells"][0]["slot"] = "history_icon"
    for row in rows:
        row["constraints"][0] = {"Length": 3}
    return item


def make_item(entry_id, label, data):
    extension = image_format(data)
    metadata = {}
    if extension:
        kind = "IMAGE"
        summary = f"{extension.upper()} image"
        # cliphist's binary description includes dimensions when available.
        description = label.removeprefix("[[ binary data ").removesuffix(" ]]")
        metadata["thumbnail"] = cache_image(data, extension)
        metadata["mime"] = f"image/{extension}"
        details = compact(description, 100)
    else:
        try:
            content = data.decode("utf-8")
            if "\x00" in content:
                raise ValueError("binary content")
        except (UnicodeDecodeError, ValueError):
            kind, summary = "FILE", "Binary data"
            details = compact(label, 100)
            metadata["content"] = "Preview unavailable for this binary format.\nPress Enter to restore the original data."
        else:
            kind = "TEXT"
            summary = compact(content, 120) or "(empty text)"
            lines = len(content.splitlines()) or 1
            details = f"{lines} {'line' if lines == 1 else 'lines'}"
            metadata["content"] = content[:MAX_PREVIEW_CHARS]
            if len(content) > MAX_PREVIEW_CHARS:
                metadata["content"] += "\n… (preview truncated)"
    info = f"{size_label(len(data))} · {details}"
    metadata["title"] = f"{kind} · #{entry_id}"
    metadata["summary"] = info
    return format_display({
        "display": {
            "rows": [
                {
                    "constraints": [{"Length": 7}, {"Fill": 1}],
                    "cells": [
                        {"text": f"{kind} ", "slot": "muted"},
                        {"text": summary, "slot": "primary"},
                    ],
                },
                {
                    "constraints": [{"Length": 7}, {"Fill": 1}],
                    "cells": [
                        {"text": "", "slot": "muted"},
                        {"text": info, "slot": "secondary"},
                    ],
                },
            ]
        },
        "value": entry_id,
        "metadata": metadata,
    })


def history_source():
    default = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "cliphist/db"
    path = Path(os.environ.get("CLIPHIST_DB_PATH", default)).absolute()
    try:
        stat = path.stat()
        # IDs are immutable within a cliphist database. A replaced database
        # gets its own index even if it reuses IDs and list descriptions.
        identity = f"{path}:{stat.st_dev}:{stat.st_ino}"
    except FileNotFoundError:
        identity = str(path)
    return hashlib.sha256(os.fsencode(identity)).hexdigest()


def store_item(connection, entry_id, label):
    result = run(["cliphist", "decode", entry_id])
    if result.returncode != 0:
        connection.execute("DELETE FROM entries WHERE id = ?", (entry_id,))
        return None
    item = make_item(entry_id, label, result.stdout)
    searchable = " ".join([
        label, entry_id, item["metadata"]["title"],
        item["metadata"].get("content", ""),
        item["metadata"].get("mime", ""),
    ]).casefold()
    connection.execute(
        "INSERT OR REPLACE INTO entries VALUES (?, ?, ?, ?)",
        (entry_id, label, searchable, json.dumps(item, separators=(",", ":"))),
    )
    return item


def search_history(tokens):
    directory = cache_directory()
    source = history_source()
    # Serialize refreshes so a cancelled/older query cannot overwrite a newer
    # history snapshot. Committed entries survive producer cancellation.
    lock_path = directory / f"index-v3-{source}.lock"
    with os.fdopen(os.open(lock_path, os.O_CREAT | os.O_RDWR, 0o600), "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        result = run(["cliphist", "list"])
        if result.returncode != 0:
            detail = result.stderr.decode("utf-8", errors="replace").strip()
            raise RuntimeError(f"cliphist list failed: {detail or result.returncode}")
        entries = {}
        for line in result.stdout.decode("utf-8", errors="replace").splitlines():
            entry_id, separator, label = line.partition("\t")
            if separator and entry_id.isascii() and entry_id.isdecimal():
                entries[entry_id] = label

        path = directory / f"index-v3-{source}.sqlite3"
        path.touch(mode=0o600, exist_ok=True)
        connection = sqlite3.connect(path, isolation_level=None)
        try:
            connection.execute("PRAGMA journal_mode=WAL")
            connection.execute("PRAGMA synchronous=NORMAL")
            connection.execute(
                "CREATE TABLE IF NOT EXISTS entries "
                "(id TEXT PRIMARY KEY, label TEXT NOT NULL, searchable TEXT NOT NULL, item TEXT NOT NULL)"
            )
            cached = dict(connection.execute("SELECT id, label FROM entries"))
            for entry_id, label in entries.items():
                if cached.get(entry_id) != label:
                    store_item(connection, entry_id, label)
            connection.executemany(
                "DELETE FROM entries WHERE id = ?",
                ((entry_id,) for entry_id in cached.keys() - entries.keys()),
            )
            # instr keeps punctuation, %, and _ literal, matching the existing
            # case-folded substring search without loading every preview JSON.
            condition = " AND ".join("instr(searchable, ?) > 0" for _ in tokens) or "1"
            matches = []
            for entry_id, serialized in connection.execute(
                f"SELECT id, item FROM entries WHERE {condition}", tokens
            ).fetchall():
                item = format_display(json.loads(serialized))
                thumbnail = item["metadata"].get("thumbnail")
                if thumbnail and not Path(thumbnail).is_file():
                    item = store_item(connection, entry_id, entries[entry_id])
                if item is not None:
                    matches.append(item)
            positions = {entry_id: position for position, entry_id in enumerate(entries)}
            matches.sort(key=lambda item: positions[item["value"]])
            return matches
        finally:
            connection.close()


def main():
    request = json.load(sys.stdin)
    if request.get("entrypoint") != "picker-items":
        raise ValueError("expected a picker-items producer request")
    if shutil.which("cliphist") is None:
        raise RuntimeError("clipboard history requires cliphist")

    state = request.get("context", {}).get("engine", {}).get("state", {})
    query = state.get("input", "") if isinstance(state, dict) else ""
    if not isinstance(query, str):
        raise ValueError("picker query must be a string")
    tokens = query.casefold().split()

    matches = search_history(tokens)
    json.dump({"version": 1, "items": matches}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ValueError, sqlite3.Error) as error:
        print(f"clipboard: {error}", file=sys.stderr)
        raise SystemExit(2)
