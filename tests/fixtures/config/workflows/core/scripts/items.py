#!/usr/bin/env python3
import json
import os
import subprocess
import sys
try:
    import tomllib
except ImportError:
    tomllib = None

def main():
    try:
        raw_input = sys.stdin.read()
        request = json.loads(raw_input) if raw_input.strip() else {}
    except Exception:
        request = {}

    context = request.get("context", {})
    parameters = context.get("parameters") or {}
    if not isinstance(parameters, dict):
        parameters = {}
    state = context.get("engine", {}).get("state") or {}

    query = parameters.get("search")
    if not query or not isinstance(query, str):
        query = state.get("input", "")
    query = query.strip() if isinstance(query, str) else ""

    sources = parameters.get("sources") or []

    suite_file = os.environ.get("TLAUNCH_SUITE")
    tlaunch_bin = os.environ.get("TLAUNCH_BIN") or "tlaunch"

    script_dir = os.path.dirname(os.path.abspath(__file__))
    core_wf_dir = os.path.dirname(script_dir)
    fixtures_wf_dir = os.path.dirname(core_wf_dir)
    config_dir = os.path.dirname(fixtures_wf_dir)
    if not suite_file:
        suite_file = os.path.join(config_dir, "default.toml")

    # Locate debug binary if tlaunch is not in PATH
    if not os.path.isabs(tlaunch_bin) and not any(
        os.access(os.path.join(p, tlaunch_bin), os.X_OK)
        for p in os.environ.get("PATH", "").split(os.pathsep)
    ):
        repo_root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(config_dir))))
        candidate = os.path.join(repo_root, "target", "debug", "tlaunch")
        if os.path.isfile(candidate):
            tlaunch_bin = candidate

    child_req = {
        "version": 1,
        "entrypoint": "picker-items",
        "context": {
            "parameters": {"query": query, "search": query, "app": query},
            "engine": {"type": "picker", "state": {"input": query}},
        },
    }
    child_req_bytes = json.dumps(child_req).encode("utf-8")

    def view_badge(view_ref, source):
        if isinstance(source, dict):
            alias = source.get("alias")
            if isinstance(alias, str) and alias:
                return alias

        if tomllib is not None and os.path.isfile(suite_file):
            try:
                with open(suite_file, "rb") as f:
                    aliases = tomllib.load(f).get("aliases", {})
                for alias, target in aliases.items():
                    if target == view_ref:
                        return alias
            except Exception:
                pass
        return view_ref

    def add_view_badge(item, badge):
        display = item.get("display")
        badge_cell = {"text": badge, "slot": "badge", "align": "right"}
        badge_width = max(1, len(badge))

        if isinstance(display, str):
            item["display"] = {
                "constraints": [{"Fill": 1}, {"Length": badge_width}],
                "cells": [{"text": display, "slot": "primary"}, badge_cell],
            }
            return

        if not isinstance(display, dict):
            return

        if isinstance(display.get("rows"), list) and display["rows"]:
            first_row = display["rows"][0]
            if not isinstance(first_row, dict):
                return
            cells = first_row.setdefault("cells", [])
            constraints = first_row.setdefault("constraints", [])
            if not isinstance(cells, list) or not isinstance(constraints, list):
                return
            if len(constraints) < len(cells):
                constraints.extend({"Fill": 1} for _ in range(len(cells) - len(constraints)))
            cells.append(badge_cell)
            constraints.append({"Length": badge_width})
            return

        cells = display.get("cells")
        if isinstance(cells, list):
            constraints = display.setdefault("constraints", [])
            if not isinstance(constraints, list):
                constraints = []
                display["constraints"] = constraints
            if len(constraints) < len(cells):
                constraints.extend({"Fill": 1} for _ in range(len(cells) - len(constraints)))
            cells.append(badge_cell)
            constraints.append({"Length": badge_width})

    def inspect_view_keymap(view_ref):
        if tlaunch_bin and os.path.exists(suite_file):
            cmd = [tlaunch_bin, "-s", suite_file, "--inspect", view_ref]
            try:
                res = subprocess.run(cmd, capture_output=True, text=True, timeout=5)
                if res.returncode == 0 and res.stdout.strip():
                    contract = json.loads(res.stdout.strip())
                    if isinstance(contract, dict) and "keymap" in contract:
                        km = contract["keymap"]
                        if isinstance(km, dict):
                            return {k: v for k, v in km.items() if k}
            except Exception:
                pass

        # General fallback: reflectively read workflow.toml
        parts = view_ref.split(":")
        wf_name = parts[0]
        v_name = parts[1] if len(parts) > 1 else "main"
        wf_toml = os.path.join(fixtures_wf_dir, wf_name, "workflow.toml")
        if os.path.exists(wf_toml):
            try:
                with open(wf_toml, "r", encoding="utf-8") as f:
                    content = f.read()
                in_keymap = False
                keymap = {}
                for line in content.splitlines():
                    line = line.strip()
                    if line.startswith("[") and line.endswith("]"):
                        in_keymap = line == f"[views.{v_name}.keymap]"
                        continue
                    if in_keymap and "=" in line and not line.startswith("#"):
                        k, v = [p.strip() for p in line.split("=", 1)]
                        k = k.strip("\"'")
                        v = v.strip("\"'")
                        if k:
                            keymap[k] = f"{wf_name}:{v}"
                return keymap
            except Exception:
                pass
        return {}

    def fetch_items_for_view(view_ref):
        # 1. Primary: Use headless CLI extraction with query propagation
        if tlaunch_bin and os.path.exists(suite_file):
            cmd = [tlaunch_bin, "-s", suite_file, "--items", view_ref]
            if query:
                cmd.append(query)
            try:
                res = subprocess.run(cmd, capture_output=True, text=True, timeout=5)
                if res.returncode == 0 and res.stdout.strip():
                    data = json.loads(res.stdout.strip())
                    if isinstance(data, list):
                        return data
                    if isinstance(data, dict) and "items" in data:
                        return data["items"]
            except Exception:
                pass

        # 2. General directory-reflection fallback
        parts = view_ref.split(":")
        wf_name = parts[0]
        wf_dir = os.path.join(fixtures_wf_dir, wf_name)
        if os.path.isdir(wf_dir):
            for script_name in ["items.py", "items.sh", f"{wf_name}.py", f"{wf_name}.sh"]:
                script_path = os.path.join(wf_dir, "scripts", script_name)
                if os.path.exists(script_path):
                    env = dict(os.environ)
                    env["WORKFLOW_DIR"] = wf_dir
                    try:
                        res = subprocess.run(
                            [script_path],
                            input=child_req_bytes,
                            capture_output=True,
                            env=env,
                            timeout=5,
                        )
                        if res.returncode == 0 and res.stdout.strip():
                            data = json.loads(res.stdout.strip())
                            if isinstance(data, list):
                                return data
                            if isinstance(data, dict) and "items" in data:
                                return data["items"]
                    except Exception:
                        pass
        return []

    def item_matches_query(item):
        if not query:
            return True
        display = item.get("display")
        # Structure cards (e.g. calculator evaluation) are dynamically produced for this query
        if isinstance(display, dict):
            return True
        if isinstance(display, str):
            tokens = [t.lower() for t in query.split()]
            if tokens and all(token in display.lower() for token in tokens):
                return True
        val = item.get("value")
        if isinstance(val, str) and query.lower() in val.lower():
            return True
        return False

    all_items = []

    for source in sources:
        view_ref = source if isinstance(source, str) else source.get("view")
        if not view_ref:
            continue

        raw_items = fetch_items_for_view(view_ref)
        keymap = inspect_view_keymap(view_ref)

        for item in raw_items:
            if keymap:
                current_bindings = item.get("bindings") or {}
                combined = dict(keymap)
                combined.update(current_bindings)
                item["bindings"] = combined
            add_view_badge(item, view_badge(view_ref, source))
            all_items.append(item)

    filtered_items = [item for item in all_items if item_matches_query(item)]

    # Fallback for standalone/isolated test runs where no sources are provided
    if not query and not sources:
        filtered_items.append({"display": "Item", "value": "value"})

    output = {"version": 1, "items": filtered_items}
    print(json.dumps(output))

if __name__ == "__main__":
    main()
