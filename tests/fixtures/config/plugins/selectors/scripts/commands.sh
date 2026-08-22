#!/bin/sh
python3 - "$@" <<'PY'
import json
import sys

commands = json.loads(sys.argv[1]) if len(sys.argv) > 1 else []
query = sys.argv[2] if len(sys.argv) > 2 else ""
tokens = str(query).casefold().split()
items = []
for command in commands:
    ref = command.get("ref")
    if not isinstance(ref, dict):
        continue
    label = str(command.get("label", ref.get("id", "")))
    key = str(command.get("key", ""))
    owner = str(command.get("owner", ""))
    searchable = " ".join([label, key, owner]).casefold()
    if not all(token in searchable for token in tokens):
        continue
    items.append({
        "label": label,
        "value": key,
        "metadata": {"command": ref},
    })
json.dump(items, sys.stdout, separators=(",", ":"))
print()
PY
