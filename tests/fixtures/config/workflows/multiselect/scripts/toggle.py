#!/usr/bin/env python3
import json
import os
import sys


def workflow_owner():
    """Member id this package is mounted as; the suite decides it, not us."""
    root = os.environ.get("TFLOW_WORKFLOW_DIR") or os.path.dirname(
        os.path.dirname(os.path.realpath(__file__))
    )
    root = os.path.realpath(root)
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


def main():
    request = json.load(sys.stdin)
    context = request.get("context", {})
    parameters = context.get("parameters", {})
    selected = list(parameters.get("selected", []))

    engine = context.get("engine", {})
    state = engine.get("state", {}) if isinstance(engine, dict) else {}
    current_input = state.get("input", "")
    item = state.get("item")

    if isinstance(item, dict):
        val = item.get("value")
        if val:
            if val in selected:
                selected.remove(val)
            else:
                selected.append(val)

    query = {
        "search": current_input,
        "selected": selected,
    }
    if isinstance(item, dict) and item.get("value"):
        query["__engine"] = {
            "focus": item["value"],
        }

    json.dump(
        {
            "version": 1,
            "operation": {
                "type": "navigate",
                "target": f"{workflow_owner()}:main",
                "replace": True,
                "query": query,
            },
        },
        sys.stdout,
        separators=(",", ":"),
    )
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
