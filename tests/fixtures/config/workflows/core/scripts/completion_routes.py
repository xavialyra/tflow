#!/usr/bin/env python3
"""Shared route lookup for the core completion and route-separator commands.

`complete.py` (Tab) and `route_separator.py` (space) must agree on the
candidate set, so both import this module. On Linux the host executes confined
file scripts through an inherited descriptor, so callers seed ``sys.path`` from
``os.path.realpath(__file__)`` before importing.

Nothing here hardcodes a workflow or View name: a package can be mounted under
any member id, so the routes and this workflow's own references are all
resolved from the suite the host is running.

Candidates are the suite's aliased, jumpable routes: every public View that
declares an alias, excluding the `core:` workflow that owns the completion UI.
Alias-less derived Views (for example `sys:output`) are internal and are not
offered.
"""
import json
import os
import subprocess

try:
    import tomllib
except ImportError:  # Python < 3.11: fall back to the directory name.
    tomllib = None

_ROUTES = None
_OWNER = None


def _read_toml(path):
    if tomllib is None:
        return None
    try:
        with open(path, "rb") as handle:
            return tomllib.load(handle)
    except Exception:
        return None


def owner_workflow():
    """Member id this workflow is mounted as in the running suite.

    A member id is the manifest key, which need not match the directory name, so
    resolve it by comparing workflow roots. Without a readable manifest the
    directory name is the best available answer.
    """
    global _OWNER
    if _OWNER is not None:
        return _OWNER
    workflow_dir = os.environ.get("TFLOW_WORKFLOW_DIR")
    if not workflow_dir:
        workflow_dir = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))
    _OWNER = os.path.basename(os.path.realpath(workflow_dir)) or "core"

    suite = os.environ.get("TFLOW_SUITE")
    if workflow_dir and suite:
        manifest = _read_toml(suite)
        if isinstance(manifest, dict):
            base = os.path.dirname(os.path.abspath(suite))
            root = os.path.realpath(workflow_dir)
            for member, entry in (manifest.get("workflows") or {}).items():
                if not isinstance(entry, dict):
                    continue
                target = entry.get("dir") or entry.get("file")
                if isinstance(target, str) and os.path.realpath(
                    os.path.join(base, target)
                ) == root:
                    _OWNER = member
                    break
    return _OWNER


def view_ref(name):
    """Canonical reference for a View of this workflow, however it is mounted."""
    return f"{owner_workflow()}:{name}"


def own_view_ref():
    """Canonical reference of this workflow's own entrypoint View."""
    entrypoint = "default"
    workflow_dir = os.environ.get("TFLOW_WORKFLOW_DIR")
    if workflow_dir:
        manifest = _read_toml(os.path.join(workflow_dir, "workflow.toml"))
        if isinstance(manifest, dict):
            entrypoint = str(
                (manifest.get("workflow") or {}).get("entrypoint") or entrypoint
            )
    return view_ref(entrypoint)


def collect_routes():
    """Every aliased route in the mounted suite, as picker items."""
    global _ROUTES
    if _ROUTES is not None:
        return _ROUTES

    _ROUTES = []
    suite = os.environ.get("TFLOW_SUITE")
    if not suite or not os.path.exists(suite):
        return _ROUTES
    binary = os.environ.get("TFLOW_BIN") or "tflow"
    try:
        result = subprocess.run(
            [binary, "-s", suite, "--inspect", "--all"],
            capture_output=True,
            text=True,
            timeout=10,
        )
    except Exception:
        return _ROUTES
    if result.returncode != 0 or not result.stdout.strip():
        return _ROUTES
    try:
        contracts = json.loads(result.stdout)
    except Exception:
        return _ROUTES

    prefix = f"{owner_workflow()}:"
    for view in contracts.get("views", []):
        if not isinstance(view, dict):
            continue
        reference = view.get("view")
        # The launcher workflow owns this completion UI; it is not a route.
        if not isinstance(reference, str) or reference.startswith(prefix):
            continue
        alias = view.get("alias")
        if not isinstance(alias, str) or not alias:
            # Derived, alias-less Views are not user-facing entry points.
            continue
        _ROUTES.append(
            {
                "value": reference,
                "display": f"{alias} ({reference})",
                "metadata": {"alias": alias},
            }
        )
    return _ROUTES


def alias_of(route):
    metadata = route.get("metadata")
    alias = metadata.get("alias") if isinstance(metadata, dict) else None
    return alias if isinstance(alias, str) else ""


def candidates(prefix):
    """Routes whose alias or reference contains every typed token."""
    tokens = [token for token in prefix.lower().split() if token]
    routes = collect_routes()
    if not tokens:
        return routes
    return [
        route
        for route in routes
        if all(
            token in f"{route['value']} {alias_of(route)}".lower() for token in tokens
        )
    ]


def find_route(selector):
    """Exact-match a typed selector against a route's alias or reference.

    An exact match is unambiguous, so it wins even when other routes merely
    contain the selector as a substring (for example `pass` inside
    `pass:unlock`).
    """
    if not selector:
        return None
    for route in collect_routes():
        if selector in (route.get("value"), alias_of(route)):
            return route
    return None
