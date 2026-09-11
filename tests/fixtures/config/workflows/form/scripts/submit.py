#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
state = request["context"]["engine"]["state"]
values = state["values"]
if values.get("name", "") and not values["name"].isprintable():
    raise SystemExit("name must contain printable characters")
if values.get("environment") not in ("dev", "prod"):
    raise SystemExit("environment must be dev or prod")
if not state["valid"]:
    raise SystemExit("Please correct the form fields")
json.dump({"version": 1, "operation": {"type": "return", "value": state["values"]}}, sys.stdout)
sys.stdout.write("\n")
