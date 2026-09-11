#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
# This deliberately small convention is owned entirely by this producer.
# Example: a:string:dd,b:number:null
fields = []
for declaration in request["context"]["parameters"].split(","):
    name, kind, initial = declaration.split(":", 2)
    value = initial if kind == "string" else json.loads(initial)
    fields.append({"name": name, "label": f"{name} ({kind})", "type": kind, "value": value})
json.dump({"version": 1, "content": {"fields": fields}}, sys.stdout)
