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

### 3. Compose Multiple Sources in an Aggregation View

An aggregate Picker combines items from multiple member Views dynamically via a producer script and item-driven bindings:

```toml
[views.default.query]
type = "object"
input = "search"
search = { type = "string", default = "" }
sources = { type = "array<string>", default = [] }

[views.default.engine]
type = "picker"

[views.default.engine.config.items]
producer = "script"
[views.default.engine.config.items.handler]
file = "scripts/items.py"

[views.default.keymap]
mode = "item"
```

In the Suite manifest, inject the targets into the aggregate View:

```toml
[suite.entrypoint]
target = "core:default"
query = { sources = ["apps:main", "calculator:main", "sys:main"] }
```

The aggregator script fetches items headlessly from each source via `tlaunch -s $TLAUNCH_SUITE --items <view> "$query"` and attaches their inspected keymaps to `item.bindings`. With `mode = "item"`, the host dispatches keys dynamically according to the selected item's attached bindings.

### 4. Use a Declared List for Small Static Collections

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

### 5. Customize a Preview

Press `Ctrl+P` in any Picker to view the selected item's display and value. Every Picker starts with its preview collapsed; no preview configuration is needed for these built-in details. Metadata remains available to custom preview providers.

To customize the content, declare a preview provider. The following example sets the automatic preview sizing and uses a script to turn selected-item metadata into a document:

```toml
[views.branches.engine.config]
preview_ratio = 0.35
preview_min_width = 24

[views.branches.engine.config.preview]
producer = "script"
[views.branches.engine.config.preview.handler]
file = "scripts/preview.py"
```

Create `scripts/preview.py`:

```python
#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
item = request["context"]["engine"]["state"]["item"]
json.dump({"version": 1, "preview": {
    "type": "paragraph",
    "spans": [{"text": item["text"] + "\n", "slot": "accent"},
              item["metadata"].get("summary", "No summary")],
}}, sys.stdout)
sys.stdout.write("\n")
```

For fixed content, replace the preview source with `preview = { producer = "declared", document = "About this list" }` under `engine.config`.

For an aggregate page, omit its `preview` field to use the selected feed's provider, falling back to built-in details when the feed has no provider. Preview sizing is controlled by `preview_ratio` and `preview_min_width`; the pane starts collapsed unless `preview_default_open = true`. Add an explicit page provider to override the content. For scripts and declared documents, the selected provider's workflow supplies its parameters, relative script/image paths, and custom styles. Plain document text uses the theme’s `picker.preview.text`; explicit slots override it. See the [preview reference](../reference/picker-preview.md) for declared documents and nested layouts.

Run the preview example from the repository root:

```sh
cargo run -- --suite tests/fixtures/config/default.toml --check
cargo run -- --suite tests/fixtures/config/default.toml images:menu
```

Press `Ctrl+P` and select **Mixed preview** to see display rows, rich wrapped text, an image, and scrollable content. Select **Empty preview** to exercise a null response. The fixture's `browser:override` View demonstrates a page-owned provider and `browser:declared` demonstrates a static document.

### 6. Toggle and Scroll the Preview Pane

`Ctrl+P` toggles the preview by default. Use the View keymap to customize the toggle binding or add scrolling keys:

```toml
[views.branches.keymap]
"ctrl+p" = "toggle_preview"
"alt+k" = "preview_scroll_up"
"alt+j" = "preview_scroll_down"
```

Scroll actions move three rows and clamp the stored offset immediately, including after resize. Query, divider, and completion rows reduce the available preview body; an empty body or unmet pane minimum cancels preview work. Preview scripts share one pending slot: the same mount can replace its pending request, while overflow from another mount fails in that incoming preview pane.

## Troubleshooting

- Put diagnostics on stderr. Any extra stdout text makes the response invalid.
- End stdout with an actual newline, not the two characters `\\n`.
- Return one JSON object only. Unknown response fields, a missing `version`, malformed items, or a second JSON document fail the request.
- `--check` validates the handler shape and confined file path before the workflow starts.

## Resource Limits

The standard script timeout is 10 seconds. Producer stdout defaults to 1 MiB, except Picker item producers, which use a 64 MiB bound. Stderr is capped at 64 KiB. Process groups are cancelled and reaped by the shared execution layer. These controls do not sandbox trusted workflow code.
