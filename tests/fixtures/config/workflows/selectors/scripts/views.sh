#!/bin/sh
set -eu

exec python3 -c '
import json
import os
import subprocess
import sys

request = json.load(sys.stdin)
context = request.get("context", {})
engine = context.get("engine", {})
state = engine.get("state", {}) if isinstance(engine, dict) else {}
query = state.get("input", "") if isinstance(state, dict) else ""
if not isinstance(query, str):
    raise SystemExit("view selector query must be a string")

workflow_dir = os.environ.get("WORKFLOW_DIR")
if not workflow_dir:
    raise SystemExit("view selector requires WORKFLOW_DIR")
config_root = os.path.abspath(os.path.join(workflow_dir, os.pardir, os.pardir))
suite_path = os.path.join(config_root, "default.toml")
if not os.path.exists(suite_path):
    suite_path = os.path.join(config_root, "suite.toml")
if not os.path.exists(suite_path):
    suite_path = os.path.join(config_root, "config.toml")
try:
    inspected = subprocess.run(
        ["tlaunch", "--suite", suite_path, "inspect", "--all"],
        check=True,
        capture_output=True,
        text=True,
    )
    catalog = json.loads(inspected.stdout)
except (OSError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
    raise SystemExit(f"could not inspect configured views: {error}")

if not isinstance(catalog, dict) or not isinstance(catalog.get("views"), list):
    raise SystemExit("inspect --all returned an invalid view catalog")

tokens = query.casefold().split()
items = []
for view in catalog["views"]:
    if not isinstance(view, dict):
        continue
    view_ref = view.get("view")
    alias = view.get("alias")
    engine_type = view.get("engine")
    if not isinstance(view_ref, str) or not view_ref or not isinstance(engine_type, str):
        continue
    if view_ref == "selectors:views":
        continue
    searchable = " ".join(
        value for value in (alias if isinstance(alias, str) else "", view_ref, engine_type)
        if value
    ).casefold()
    if not all(token in searchable for token in tokens):
        continue
    label = f"{alias} ({view_ref})" if isinstance(alias, str) else view_ref
    items.append({"display": label, "value": view_ref, "metadata": view})

json.dump({"version": 1, "items": items}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
'
