#!/bin/sh
set -eu

exec python3 -c '
import json
import os
import subprocess
import sys


def workflow_owner():
    """Member id this package is mounted as; the suite decides it, not us."""
    root = os.path.realpath(os.environ.get("TFLOW_WORKFLOW_DIR") or "")
    try:
        import tomllib

        with open(os.environ["TFLOW_SUITE"], "rb") as handle:
            workflows = tomllib.load(handle).get("workflows") or {}
        base = os.path.dirname(os.path.abspath(os.environ["TFLOW_SUITE"]))
        for member, entry in workflows.items():
            target = (entry or {}).get("dir") or (entry or {}).get("file")
            if isinstance(target, str) and os.path.realpath(
                os.path.join(base, target)
            ) == root:
                return member
    except Exception:
        pass
    name = os.path.basename(root)
    if not name:
        raise SystemExit("cannot resolve the workflow owner; set TFLOW_WORKFLOW_DIR")
    return name


request = json.load(sys.stdin)
context = request.get("context", {})
engine = context.get("engine", {})
state = engine.get("state", {}) if isinstance(engine, dict) else {}
query = state.get("input", "") if isinstance(state, dict) else ""
if not isinstance(query, str):
    raise SystemExit("view selector query must be a string")

suite = os.environ.get("TFLOW_SUITE")
if not suite:
    raise SystemExit("view selector requires TFLOW_SUITE")
binary = os.environ.get("TFLOW_BIN") or "tflow"
try:
    inspected = subprocess.run(
        [binary, "--suite", suite, "--inspect", "--all"],
        check=True,
        capture_output=True,
        text=True,
    )
    catalog = json.loads(inspected.stdout)
except (OSError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
    raise SystemExit(f"could not inspect configured views: {error}")

if not isinstance(catalog, dict) or not isinstance(catalog.get("views"), list):
    raise SystemExit("--inspect --all returned an invalid view catalog")

own_prefix = f"{workflow_owner()}:"
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
    # The selector workflow owns this UI; its own Views are not targets.
    if view_ref.startswith(own_prefix):
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
