---
title: "How to Configure Picker Views"
type: "guide"
tags:
  - picker
  - views
  - aggregation
  - item-bindings
  - preview
description: "Configure Picker Views with static items, script producers, multi-source aggregation, and script-produced preview documents."
---

# How to Configure Picker Views

Use the `picker` Engine for a selectable list with an editable query. A Picker can use literal items, a version-1 item producer, or an aggregation script composing multiple member Views.

## Problem

You want to present selectable data, filter it as the user types, and optionally show metadata in a preview pane.

## Solution

### 1. Configure the Picker Engine

Declare a Picker View and start with a small literal list:

```toml
[views.main]
engine = "picker"

[views.main.picker]
items = [
  { display = "Show date", value = "date", metadata = {} },
  { display = "System information", value = "info", metadata = {} },
]
```

`display` may be a plain string or a structured display value. `value` is optional and is exposed as a string when present. `metadata` defaults to an empty JSON object.

Add `input_placeholder` to show a muted hint while the query is empty. It is purely presentational and can be combined with `show_input`, `show_divider`, `show_left_prefix`, and a `[picker] left_prefix` marker:

```toml
[views.main]
engine = "picker"

[views.main.picker]
input_placeholder = "Type to filter…"
items = [
  { display = "Show date", value = "date", metadata = {} },
]
```

Style the hint with `[picker.placeholder]` in the theme. The terminal cursor appears at the input position; with a left prefix the hint is rendered after the prefix, and typing replaces the hint without ever entering the query value.

### 2. Load Items from a Producer

Use an item producer when the complete collection must be computed at request time:

```toml
[views.branches]
engine = "picker"

[views.branches.picker.items]
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

### 3. Compose Multiple Sources in an Aggregation View

An aggregate Picker combines items from multiple member Views dynamically via a producer script and item-driven bindings:

```toml
[views.default]
engine = "picker"
binding_mode = "item_merge"

[views.default.query]
type = "object"
input = "search"
search = { type = "string", default = "" }
sources = { type = "array<string>", default = [] }

[views.default.picker.items]
file = "scripts/items.py"

[views.default.bindings]
enter = "open"
```

In the Suite manifest, inject the targets into the aggregate View:

```toml
[suite.entrypoint]
target = "core:default"
query = { sources = ["apps:main", "calculator:main", "sys:main"] }
```

The aggregator script fetches items headlessly from each source via `tflow -s $TFLOW_SUITE --items <view> "$query"` and attaches their inspected bindings to `item.bindings`. With `binding_mode = "item_merge"`, the host dispatches keys dynamically according to the selected item's attached bindings, on top of any bindings the View itself declared.

Precedence inside one View is **focused item → View base bindings → Engine bindings**. Because the View's own bindings do not come from item data, a command the View owns stays reachable while the list is empty, still loading, or filtered down to nothing. Declare permanent keys in `[views.<name>.bindings]` instead of relying on every item to carry them; the development fixture's `core` View declares its own base keys there. Item bindings name commands, so an item can rebind a base key but cannot take one away — releasing an inherited key is the View's own decision, expressed with `[views.<name>.unbind]`. Base bindings for Engine default keys shadow that Engine binding for the View, exactly as in the default `binding_mode = "view"`.

### 4. Use a Declared List for Small Static Collections

No script is needed when the list is static:

```toml
[views.actions]
engine = "picker"

[views.actions.picker]
items = [
  { display = "Show date", value = "date" },
  { display = "System information", value = "info" },
]
```

The plain `items = [...]` array is also supported as a declared shorthand.

### 5. Attach a Companion View (ADR 0010)

Views can attach an interactive companion view (such as a `capture` inspection pane) rendered side-by-side:

```toml
[views.branches]
engine = "picker"
companion = "preview"

[views.preview]
engine = "capture"

[views.preview.capture.output]
file = "scripts/preview.py"
```

Create `scripts/preview.py`:

```python
#!/usr/bin/env python3
import json
import sys

raw = sys.stdin.read()
print(f"Details: {raw}")
```

The companion view attaches side-by-side to display details live. Return or close through the foreground View's normal back/close command (`Esc`).

### 6. Toggle Companion Views

Use a normal workflow command and bind it at the View layer. `Ctrl-P` has no special meaning and is not reserved:

```toml
[commands.toggle_details]
label = "Toggle Details"
companion = "preview"

[views.branches.bindings]
"ctrl+p" = "toggle_details"
```

The command toggles the companion view on or off. When `query` is omitted in the command, the companion tracks the primary view's live selection. Specify an explicit `query` in the command when custom parameters are intended. Change the key or release it through `[views.branches.unbind]` like any other command.
## Troubleshooting

- Put diagnostics on stderr. Any extra stdout text makes the response invalid.
- End stdout with an actual newline, not the two characters `\\n`.
- Return one JSON object only. Unknown response fields, a missing `version`, malformed items, or a second JSON document fail the request.
- `--check` validates the handler shape and confined file path before the workflow starts.

## Resource Limits

The standard script timeout is 10 seconds. Producer stdout defaults to 1 MiB, except Picker item producers, which use a 64 MiB bound. Stderr is capped at 64 KiB. Process groups are cancelled and reaped by the shared execution layer. These controls do not sandbox trusted workflow code.
