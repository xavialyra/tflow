#!/bin/sh
set -eu

exec python3 -c '
import json
import sys

request = json.load(sys.stdin)
parameters = request.get("parameters", {})
commands = parameters.get("commands", [])
query = request.get("request", {}).get("input", "")
if not isinstance(commands, list) or not isinstance(query, str):
    raise SystemExit("invalid command selector request")
tokens = query.casefold().split()
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
    item = {"value": key, "metadata": {"command": ref}}
    if key:
        item["display"] = {
            "constraints": [{"Fill": 1}, {"Length": len(key)}],
            "cells": [
                {"text": label},
                {"text": key, "align": "right", "slot": "badge"},
            ],
        }
    else:
        item["display"] = label
    items.append(item)
json.dump({"version": 1, "items": items}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
'