#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
context = request["context"]
json.dump({"version": 1, "preview": {
    "type": "paragraph", "border": True, "title": "Page override",
    "text": f"Provider owner: {context['parameters']['owner']}\nItem: {context['engine']['state']['item']['text']}",
}}, sys.stdout)
sys.stdout.write("\n")
