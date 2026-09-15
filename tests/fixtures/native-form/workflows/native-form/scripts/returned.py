#!/usr/bin/env python3
import json
import os
import sys

request = json.load(sys.stdin)
assert request["entrypoint"] == "return"
# The restored caller publishes its own drafts; the child explicitly returns
# its edited values through the ordinary return command.
if path := os.environ.get("NATIVE_FORM_RETURN"):
    with open(path, "w") as result:
        json.dump(request, result)
json.dump({
    "version": 1,
    "operation": {
        "type": "return",
        "value": {
            "caller": request["context"]["engine"]["state"]["values"],
            "child": request["context"]["result"],
        },
    },
}, sys.stdout)
