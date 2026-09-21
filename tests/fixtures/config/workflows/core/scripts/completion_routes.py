#!/usr/bin/env python3
"""Shared route lookup for the core completion and route-separator commands.

`complete.py` (Tab) and `route_separator.py` (space) must agree on the
candidate set, so both import this module. On Linux the host executes confined
file scripts through an inherited descriptor, so callers seed ``sys.path`` from
``os.path.realpath(__file__)`` before importing.

Candidates are the suite's aliased, jumpable routes: every public View that
declares an alias, excluding the `core:` workflow that owns the completion UI.
Alias-less derived Views (for example `sys:output`) are internal and are not
offered.
"""
import json
import os
import subprocess


def collect_routes():
    suite = os.environ.get("TLAUNCH_SUITE")
    if not suite or not os.path.exists(suite):
        return []
    binary = os.environ.get("TLAUNCH_BIN") or "tlaunch"
    try:
        result = subprocess.run(
            [binary, "-s", suite, "--inspect", "--all"],
            capture_output=True,
            text=True,
            timeout=10,
        )
    except Exception:
        return []
    if result.returncode != 0 or not result.stdout.strip():
        return []
    try:
        contracts = json.loads(result.stdout)
    except Exception:
        return []

    routes = []
    for view in contracts.get("views", []):
        if not isinstance(view, dict):
            continue
        reference = view.get("view")
        if not isinstance(reference, str) or reference.startswith("core:"):
            continue
        alias = view.get("alias")
        if not isinstance(alias, str) or not alias:
            # Derived, alias-less Views are not user-facing entry points.
            continue
        routes.append(
            {
                "value": reference,
                "display": f"{alias} ({reference})",
                "metadata": {"alias": alias},
            }
        )
    return routes


def alias_of(route):
    metadata = route.get("metadata")
    alias = metadata.get("alias") if isinstance(metadata, dict) else None
    return alias if isinstance(alias, str) else ""


def candidates(prefix):
    tokens = [token for token in prefix.lower().split() if token]
    routes = collect_routes()
    if not tokens:
        return routes
    return [
        route
        for route in routes
        if all(token in f"{route['value']} {alias_of(route)}".lower() for token in tokens)
    ]


def find_route(selector):
    """Exact-match a typed selector against a route's alias or reference."""
    if not selector:
        return None
    for route in collect_routes():
        if selector in (route.get("value"), alias_of(route)):
            return route
    return None
