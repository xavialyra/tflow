#!/bin/sh
set -eu

python3 -c '
import json
import os
import subprocess
import sys

request = json.load(sys.stdin)
context = request.get("context", {})
parameters = context.get("parameters", {})

if isinstance(parameters, str):
    action = parameters
elif isinstance(parameters, dict):
    action = parameters.get("action") or parameters.get("query")
else:
    action = None

commands = {
    "logout": ["loginctl", "terminate-user", os.environ.get("USER", "")],
    "reboot": ["systemctl", "reboot"],
    "poweroff": ["systemctl", "poweroff"],
}
command = commands.get(action)
if not command or (action == "logout" and not command[-1]):
    raise SystemExit("unknown or incomplete system action")

result = subprocess.run(command, capture_output=True, text=True)
output = (result.stdout + result.stderr).strip()
if result.returncode != 0 and not output:
    output = f"command exited with status {result.returncode}"
if result.returncode != 0:
    output = f"Unable to perform {action}: {output}"

json.dump({"version": 1, "output": output + "\n"}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
'
