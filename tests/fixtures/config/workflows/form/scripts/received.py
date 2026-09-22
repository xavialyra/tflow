#!/usr/bin/env python3
"""Consume the child result in the caller, then display it in a Capture view."""

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


request = json.load(sys.stdin)
if request["entrypoint"] == "return":
    response = {
        "version": 1,
        "operation": {
            "type": "navigate",
            "target": f"{workflow_owner()}:result",
            "query": {"result": request["context"]["result"]},
        },
    }
elif request["entrypoint"] == "capture-output":
    result = request["context"]["parameters"]["result"]
    response = {
        "version": 1,
        "output": "Caller received:\n" + json.dumps(result, ensure_ascii=False, indent=2),
    }
else:
    raise SystemExit("expected a return or capture-output request")
json.dump(response, sys.stdout, ensure_ascii=False)
sys.stdout.write("\n")
