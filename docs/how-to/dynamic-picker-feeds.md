---
title: "How to Build Dynamic Picker Feeds and Previews"
type: "guide"
tags:
  - picker
  - preview
  - producers
  - scripts
  - feed
description: "Configure a version-1 script producer for Picker items and a static metadata-backed preview."
---

# How to Build Dynamic Picker Feeds and Previews

Use a Picker items producer when the complete item list must be computed at request time. The launcher sends a JSON request on stdin and expects one version-1 JSON response on stdout.

## Problem

You want to show items computed from a repository, service, or file tree and filter them as the user types.

## Solution

### 1. Configure the Items Producer

In a directory workflow, declare the producer under the Picker Engine:

```toml
[views.branches.engine]
type = "picker"

[views.branches.engine.config.items]
producer = "script"

[views.branches.engine.config.items.handler]
file = "scripts/get_branches.py"
```

The handler must define exactly one non-empty `file` or inline `script` field. Relative files are resolved inside the workflow package.

### 2. Read the Request and Write the Complete Collection

Create `scripts/get_branches.py`:

```python
#!/usr/bin/env python3
import json
import subprocess
import sys

request = json.load(sys.stdin)
context = request["context"]
needle = context["engine"]["state"].get("input", "")
branches = subprocess.check_output(
    ["git", "for-each-ref", "--format=%(refname:short)", "refs/heads/"],
    text=True,
).splitlines()
items = [
    {"display": branch, "value": branch, "metadata": {}}
    for branch in branches
    if needle.lower() in branch.lower()
]
json.dump({"version": 1, "items": items}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
```

Make it executable:

```bash
chmod +x ~/.config/tui-launcher/workflows/branches/scripts/get_branches.py
```

The request includes `entrypoint = "picker-items"` and a unified `context` object. Use `context.parameters` for bound feed parameters, `context.input` for the explicit launch input descriptor, and `context.engine.state.input` for the current Picker query. The response must contain the complete `items` array:

```json
{"version":1,"items":[{"display":"main","value":"main","metadata":{}}]}
```

The host normalizes display values and owns feed composition, selection state, and feed provenance. Feed identity and scheduling data are not sent automatically. A producer cannot return navigation or other View configuration. Item stdout is limited to 64 MiB so large feeds remain usable.

### 3. Run a Command from the Selected Feed Owner

An aggregate Picker can expose commands declared by the View that owns the currently selected feed item. Declare the command on the owner View, and give it a physical `key` so the host can project it into the aggregate command area:

```toml
# In the apps workflow, whose View is mounted as a feed.
[views.main.commands.open]
key = "ctrl+o"
label = "Open application"
type = "run"
producer = "script"

[views.main.commands.open.handler]
file = "scripts/open.py"
```

The command script reads the selected item through the public Engine state:

```python
import json
import sys

request = json.load(sys.stdin)
context = request["context"]
parameters = context["parameters"]
item = context["engine"]["state"].get("item")
value = item.get("value") if isinstance(item, dict) else None
if not isinstance(value, str):
    raise SystemExit("select an application first")

json.dump({
    "version": 1,
    "operation": {
        "type": "run",
        "mode": "foreground",
        "argv": ["printf", "parameters=%s selected=%s\\n" % (json.dumps(parameters), value)],
        "exit": True,
    },
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\\n")
```

For a projected owner command, `context.parameters` is the independent parameter snapshot for the selected feed, not the aggregate Picker's page parameters. `context.engine.state.item` is still the aggregate Picker's public selected-item projection, and `context.input` remains the explicit launch input descriptor. Owner View names, feed IDs, mounted View identity, and scheduling data are not included in the request. The projection is recomputed as selection changes and is revalidated against the current result before dispatch.

A command without a physical `key` cannot appear in the command area or be invoked through a projected key binding. Global bindings retain precedence when a physical key conflicts. The command's `scope` does not replace owner resolution; the selected item's feed provenance determines which owner context the script receives.

### 4. Use a Declared List for Small Fixed Feeds

No script is needed when the list is static:

```toml
[views.actions.engine]
type = "picker"

[views.actions.engine.config.items]
producer = "declared"

[views.actions.engine.config.items.handler]
items = [
  { display = "Show date", value = "date" },
  { display = "System information", value = "info" },
]
```

The plain `items = [...]` array is also supported as a declared shorthand.

### 5. Configure a Preview

Preview layout and content sources are static Engine configuration. A preview block reads a JSON Pointer from the selected item's metadata; it does not execute a script or interpolate a selected value.

```toml
[views.branches.engine.config.layout]
direction = "horizontal"
gap = 1

[[views.branches.engine.config.layout.panes]]
slot = "items"
grow = 1
min = 28

[[views.branches.engine.config.layout.panes]]
slot = "preview"
size = 36
min = 24

[views.branches.engine.config.preview]

[[views.branches.engine.config.preview.blocks]]
type = "text"
source = "/metadata/summary"
grow = 1
```

The items producer should place preview data under `metadata`, for example `{"metadata":{"summary":"recent commits"}}`. Image blocks use a metadata JSON Pointer to an image path. Missing or invalid preview data is rendered as a preview diagnostic.

### 6. Toggle the Preview Pane

You can bind a key to the built-in preview action:

```toml
[views.branches.keymap]
"ctrl+p" = "toggle_preview"
```

## Troubleshooting

- Put diagnostics on stderr. Any extra stdout text makes the response invalid.
- End stdout with an actual newline, not the two characters `\\n`.
- Return one JSON object only. Unknown response fields, a missing `version`, malformed items, or a second JSON document fail the request.
- `--check` validates the handler shape and confined file path before the workflow starts.

## Resource Limits

The standard script timeout is 10 seconds. Producer stdout defaults to 1 MiB, except Picker item producers, which use a 64 MiB bound. Stderr is capped at 64 KiB. Process groups are cancelled and reaped by the shared execution layer. These controls do not sandbox trusted workflow code.
