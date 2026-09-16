#!/usr/bin/env python3
import json
import os
import sys


def main():
    request = json.load(sys.stdin)
    context = request.get("context", {})
    parameters = context.get("parameters", {})
    selected = parameters.get("selected", [])

    result_file = os.environ.get("MULTISELECT_RESULT")
    if result_file:
        with open(result_file, "w") as f:
            json.dump({"selected": selected}, f)

    json.dump(
        {
            "version": 1,
            "operation": {
                "type": "run",
                "mode": "foreground",
                "argv": [
                    "printf",
                    "Submitted selected: %s\\n",
                    ",".join(selected),
                ],
                "exit": True,
            },
        },
        sys.stdout,
        separators=(",", ":"),
    )
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
