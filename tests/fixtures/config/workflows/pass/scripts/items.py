#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys


def emit(items):
    json.dump({"version": 1, "items": items}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


def main():
    request = json.load(sys.stdin)
    if request.get("entrypoint") != "picker-items":
        raise ValueError("expected a picker-items producer request")

    state = request.get("context", {}).get("engine", {}).get("state", {})
    query = state.get("input", "") if isinstance(state, dict) else ""
    if not isinstance(query, str):
        raise ValueError("picker query must be a string")
    tokens = query.casefold().split()

    configured_store = os.environ.get("PASSWORD_STORE_DIR")
    store = (
        Path(configured_store).expanduser()
        if configured_store
        else Path.home() / ".password-store"
    )
    if not store.is_dir():
        emit([])
        return

    entries = []
    for root, directories, filenames in os.walk(store):
        directories[:] = [directory for directory in directories if directory != ".git"]
        for filename in filenames:
            if not filename.endswith(".gpg"):
                continue
            path = Path(root) / filename
            try:
                entry = path.relative_to(store).as_posix()[:-4]
            except ValueError:
                continue
            if entry and all(token in entry.casefold() for token in tokens):
                entries.append(entry)

    entries.sort(key=str.casefold)
    emit(
        [
            {
                "display": entry,
                "value": entry,
                "metadata": {"source": "pass"},
            }
            for entry in entries
        ]
    )


if __name__ == "__main__":
    try:
        main()
    except (OSError, TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"pass: {error}", file=sys.stderr)
        raise SystemExit(2)
