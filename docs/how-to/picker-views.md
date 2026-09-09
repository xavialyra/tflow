---
title: "How to Configure Picker Views"
type: "guide"
tags:
  - picker
  - views
  - feeds
  - preview
description: "Configure Picker Views with static items, dynamic feeds, aggregate feeds, and metadata-backed previews."
---

# How to Configure Picker Views

Use the `picker` Engine for a selectable list with an editable query. A Picker can use literal items, a version-1 item producer, or several feed Views composed by the host.

## Problem

You want to present selectable data, filter it as the user types, and optionally show metadata in a preview pane.

## Solution

### 1. Configure the Picker Engine

Declare a Picker View and start with a small literal list:

```toml
[views.main.engine]
type = "picker"

[views.main.engine.config]
items = [
  { display = "Show date", value = "date", metadata = {} },
  { display = "System information", value = "info", metadata = {} },
]
```

`display` may be a plain string or a structured display value. `value` is optional and is exposed as a string when present. `metadata` defaults to an empty JSON object.

### 2. Load Items from a Producer

Use an item producer when the complete collection must be computed at request time:

```toml
[views.branches.engine]
type = "picker"

[views.branches.engine.config.items]
producer = "script"

[views.branches.engine.config.items.handler]
file = "scripts/get_branches.py"
```

A script handler has exactly one non-empty `file` or inline `script` field. Relative files are resolved within the workflow package. Create `scripts/get_branches.py`:

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

The request uses `entrypoint = "picker-items"` and the shared `context` object. `context.parameters` contains the bound parameters for this feed, `context.input` contains the explicit launch input descriptor, and `context.engine.state.input` contains the current Picker query. The response must replace the complete collection:

```json
{"version":1,"items":[{"display":"main","value":"main","metadata":{}}]}
```

The host owns feed composition, selection state, and provenance. Feed identity and scheduling data are not sent to the producer. An item producer cannot return navigation or other View configuration. See [Commands and Producer Scripts](commands-and-producers.md) for command context and response rules.

### 3. Compose Multiple Feeds

An aggregate Picker can combine configured feed Views:

```toml
[views.default.engine]
type = "picker"

[views.default.engine.config]
[[views.default.engine.config.feeds]]
view = "apps:main"

[[views.default.engine.config.feeds]]
view = "sys:main"
```

Each feed keeps its own parameter binding and producer request. The host assigns item provenance internally and exposes only the normalized public item under `context.engine.state.item` to command producers. Aggregate pages automatically add the feed alias as a right-aligned `badge` on the first row of items from external feeds; when no alias is configured, the workflow ID is used. Set `source_badge = false` in the aggregate View's engine config to disable this behavior. Commands declared by a selected feed owner can be projected into the aggregate footer when they have a physical key; their parameter context remains the selected feed's independent snapshot.

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

Bind a key to the built-in preview action:

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
