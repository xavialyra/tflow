#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
assert request["entrypoint"] == "form-content"
assert request["context"]["engine"]["type"] == "form"
# This convention belongs to the script, not to the engine or query schema.
content = request["context"]["parameters"]["spec"]
json.dump({"version": 1, "content": content}, sys.stdout)
