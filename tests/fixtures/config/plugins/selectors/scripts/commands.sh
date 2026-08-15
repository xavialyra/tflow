#!/bin/sh
python3 -c '
import json, sys
request = json.load(sys.stdin)
tokens = str(request.get("query", "")).casefold().split()
items = []
for command in request.get("commands", []):
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
'
