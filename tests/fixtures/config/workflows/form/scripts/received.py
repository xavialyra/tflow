#!/usr/bin/env python3
"""Consume the child result in the caller, then display it in a Capture view."""

import json
import sys

request = json.load(sys.stdin)
if request["entrypoint"] == "return":
    response = {
        "version": 1,
        "operation": {
            "type": "navigate",
            "target": "form:result",
            "query": {"result": request["result"]},
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
