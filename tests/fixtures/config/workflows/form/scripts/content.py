#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
assert request["entrypoint"] == "form-content"
assert request["context"]["engine"]["type"] == "form"
spec = request["context"]["parameters"]["spec"]
parameters = request["context"]["parameters"]
fields = []
for field in spec["fields"]:
    field = dict(field)
    if field["name"] in parameters:
        field["value"] = parameters[field["name"]]
    fields.append(field)
json.dump({"version": 1, "content": {"fields": fields}}, sys.stdout)
