#!/usr/bin/env python3
import json
import os
import sys

request = json.load(sys.stdin)
state = request["context"]["engine"]["state"]
if not state["valid"]:
    raise SystemExit("Please correct the form fields")
if path := os.environ.get("NATIVE_FORM_RESULT"):
    with open(path, "w") as result:
        json.dump(request, result)
json.dump({"version": 1, "operation": {"type": "return", "value": state["values"]}}, sys.stdout)
