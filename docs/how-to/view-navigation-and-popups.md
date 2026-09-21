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

`tlaunch` keeps Views in a Router-owned stack. Producer operations are validated before they are applied, then use the same target binding and transition path as built-in navigation. For the shared command request context and script response contract, see [Commands and Producer Scripts](commands-and-producers.md).

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

When the target or query depends on the current selection, use a script producer. The script reads the selected item from `context.engine.state.item` and returns a typed operation. If the command is declared by the selected item's feed owner and projected into an aggregate Picker, `context.parameters` is the feed owner's bound parameter snapshot; otherwise it is the command's mounted View parameters:

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
state = request["context"]["engine"]["state"]
item = state.get("item") if isinstance(state, dict) else None
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

A return handler must contain an explicit `value`; omitting it is invalid. A script response may contain `"value": null`, which is a successful null result, not a close/cancel decision. Use `type = "return"` with `producer = "declared"` or `producer = "script"`; the response type from a script must match the command type.

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

The processor runs only after the child has closed and the recorded caller has been activated. Its request contains `entrypoint = "return"`, the caller's owner parameters captured at call time, and the unified `context` object containing `context.result`. Its response is the same version-1 operation or error envelope as a command.

A processor may inspect a selected result like this:

```python
request = json.load(sys.stdin)
result = request["context"]["result"]
value = result.get("value") if isinstance(result, dict) else None
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

### 6. Restore a Known Selection After Replace

A `replace` re-mounts the target, so a Picker starts on its first row again. Carry the previous item identity as the host-reserved `__focus` query key (or `__engine = { focus = "..." }`); the Picker selects the first loaded item whose `value` or `text` matches. Reserved `__`-prefixed keys are stripped before the target View's parameters are published.

```toml
[views.main.commands.refresh]
key = "ctrl+r"
label = "Refresh"
type = "navigate"
producer = "script"

[views.main.commands.refresh.handler]
file = "scripts/refresh.py"
```

The script reads the current selection from `context.engine.state.item` and re-enters the same View with `replace = true`:

```python
request = json.load(sys.stdin)
state = request["context"]["engine"]["state"]
item = state.get("item") if isinstance(state, dict) else None
focus = item.get("value") if isinstance(item, dict) else None
operation = {"type": "navigate", "target": "main", "replace": True}
if isinstance(focus, str):
    operation["query"] = {"__focus": focus}
json.dump({"version": 1, "operation": operation}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
```

## Troubleshooting

- Put script diagnostics on stderr; stdout must contain one JSON response object.
- Use an actual newline after JSON. Writing the literal characters `\\n` produces invalid protocol output.
- Check the workflow with `tlaunch --check` before testing transitions.
- A successful call return with no processor restores the caller and discards the unconsumed result.
