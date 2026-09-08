exec python3 -c '
import json
import sys

request = json.load(sys.stdin)
context = request.get("context", {})
engine = context.get("engine", {})
state = engine.get("state", {}) if isinstance(engine, dict) else {}
item = state.get("item") if isinstance(state, dict) else None
if not isinstance(item, dict) or not isinstance(item.get("value"), str):
    raise SystemExit("apps command requires a selected item with a value")
json.dump({
    "version": 1,
    "operation": {
        "type": "run",
        "mode": "foreground",
        "argv": ["setsid", "--wait", "gio", "launch", item["value"]],
        "exit": True,
    },
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
'
