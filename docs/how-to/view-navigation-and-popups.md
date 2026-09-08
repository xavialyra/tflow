---
title: "How to Configure View Navigation and Popups"
type: "guide"
tags:
  - navigation
  - popup
  - modal
  - commands
  - producers
description: "Configure static and script-produced navigation, popup calls, return values, and post-commit return processors."
---

# How to Configure View Navigation and Popups

`tui-launcher` keeps Views in a Router-owned stack. Producer operations are validated before they are applied, then use the same target binding and transition path as built-in navigation.

## Problem

You want to open a secondary View as a popup, return a value, and optionally decide what happens next in the caller.

## Solution

### 1. Call a Popup with a Declared Operation

For a fixed target, use a declared call handler:

```toml
[views.main.commands.select_action]
key = "ctrl+o"
label = "Actions"
type = "call"
producer = "declared"

[views.main.commands.select_action.handler]
target = "selectors:actions"
presentation = { mode = "popup", width = 70, height = 18 }
```

The target must be a configured View or alias. Popup width and height are terminal-cell dimensions and are clamped to the available terminal area. While the popup is active, only its top View receives input.

### 2. Produce Dynamic Navigation from a Selected Item

When the target or query depends on the current selection, use a script producer. The script receives `engine_output.selected_item` in its command request and returns a typed operation:

```toml
[views.main.commands.open]
key = "enter"
label = "Open selected item"
type = "call"
producer = "script"

[views.main.commands.open.handler]
file = "scripts/open-selected.py"
```

```python
#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
item = request.get("engine_output", {}).get("selected_item")
value = item.get("value") if isinstance(item, dict) else None
if not isinstance(value, str):
    raise SystemExit("a selected item with a string value is required")

json.dump({
    "version": 1,
    "operation": {
        "type": "call",
        "target": "files:preview",
        "query": {"path": value},
        "presentation": {"mode": "popup", "width": 80, "height": 20},
    },
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
```

The host resolves the target, validates its query schema, and commits the call. A malformed query or unavailable target leaves the caller unchanged.

### 3. Return from the Child

Define a typed return command in the child View:

```toml
[views.actions.commands.confirm]
key = "enter"
label = "Confirm"
type = "return"
producer = "declared"

[views.actions.commands.confirm.handler]
value = "confirmed"
```

An omitted `value` returns the current Engine output. An explicit `value = null` is a successful null result, not a close/cancel decision. Use `type = "return"` with `producer = "declared"` or `producer = "script"`; the response type from a script must match the command type.

To close without a result, bind the Engine's `back`/close action or a command that produces the host close behavior. A close does not run a return processor.

### 4. Process the Result After Restoring the Caller

A call can declare a post-commit return processor:

```toml
[views.main.commands.select_action.return_processor]
type = "navigate"
producer = "script"

[views.main.commands.select_action.return_processor.handler]
file = "scripts/process-action.py"
```

The processor runs only after the child has closed and the recorded caller has been activated. Its request contains `entrypoint = "return"`, the caller's owner parameters captured at call time, the caller command reference, and a typed `result` object. Its response is the same version-1 operation envelope as a command.

A processor may inspect a selected result like this:

```python
request = json.load(sys.stdin)
result = request["result"]
item = result.get("item", {}) if result.get("kind") == "selected" else {}
value = item.get("value")
if not isinstance(value, str):
    raise SystemExit("expected a selected item")
json.dump({
    "version": 1,
    "operation": {"type": "navigate", "target": "sys:output", "query": {"action": value}},
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
```

The caller remains mounted if processor execution or response validation fails. The child is not reopened and the script is not retried automatically.

### 5. Replace Instead of Push

Use a declared navigate operation with `replace = true` when the current View should not remain underneath the target:

```toml
[views.step1.commands.next]
key = "enter"
type = "navigate"
producer = "declared"

[views.step1.commands.next.handler]
target = "workflow:step2"
replace = true
```

With `replace = false` or an omitted field, the operation pushes a new stack entry.

## Troubleshooting

- Put script diagnostics on stderr; stdout must contain one JSON response object.
- Use an actual newline after JSON. Writing the literal characters `\\n` produces invalid protocol output.
- Check the workflow with `tui-launcher --check` before testing transitions.
- A successful call return with no processor restores the caller and discards the unconsumed result.
