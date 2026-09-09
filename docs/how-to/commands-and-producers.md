---
title: "How to Configure Commands and Producer Scripts"
type: "guide"
tags:
  - commands
  - producers
  - scripts
  - protocol
description: "Configure View commands and read the version-1 producer context across Picker, Capture, and return flows."
---

# How to Configure Commands and Producer Scripts

Use a command producer when a command's operation depends on runtime data. TOML remains literal; the host sends one JSON request on stdin and validates one JSON response on stdout.

## Problem

You need a command to inspect the current View state, selected Picker item, or feed parameters without embedding runtime expressions in configuration.

## Solution

### 1. Declare the Command

The command declares its key, label, operation type, producer kind, and matching handler:

```toml
[views.main.commands.open]
key = "enter"
label = "Open selected item"
type = "navigate"
producer = "script"

[views.main.commands.open.handler]
file = "scripts/open.py"
```

Declared handlers use the same operation schema without starting a script. Use `producer = "script"` when the operation must be computed at runtime.

### 2. Read the Shared Context

Every producer request uses the same public context fields:

| Field | Meaning |
| :--- | :--- |
| `context.parameters` | Bound parameters for the command's owner or the current provider feed. |
| `context.input` | Explicit launch input descriptor. It is not a substitute for Picker query state. |
| `context.engine.type` | Carrying Engine type: `picker`, `capture`, or another registered Engine. |
| `context.engine.state` | Public state projection for that Engine. Picker selection is `state.item`; the current query is `state.input`. |
| `result` | Raw JSON result, present only for a return processor. |

The host does not add View names, feed IDs, mounted instance identity, task generations, cancellation handles, or scheduling data to the request.

### 3. Use a Selected Picker Item

A command script reads the normalized selected item from `context.engine.state.item`:

```python
import json
import sys

request = json.load(sys.stdin)
state = request["context"]["engine"]["state"]
item = state.get("item") if isinstance(state, dict) else None
value = item.get("value") if isinstance(item, dict) else None
if not isinstance(value, str):
    raise SystemExit("select an item first")

json.dump({
    "version": 1,
    "operation": {
        "type": "run",
        "mode": "foreground",
        "argv": ["printf", "selected:%s\n" % value],
        "exit": True,
    },
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
```

The item is a public projection containing display text, value, and metadata. Internal provenance is intentionally absent.

### 4. Use a Feed Owner's Parameters in an Aggregate Picker

When an aggregate Picker selects an item from `apps:main`, a command declared by `apps:main` can be projected into the aggregate footer:

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
        "argv": ["printf", "parameters=%s selected=%s\n" % (json.dumps(parameters), value)],
        "exit": True,
    },
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
```

For this projected command, `context.parameters` is the selected feed's independent bound parameter snapshot. It is not the aggregate Picker page's parameter object. `context.engine.state.item` remains the aggregate Picker's public selected item. The host recomputes the projection when selection changes and revalidates the current result before dispatch.

Commands must have a physical `key` to appear in the aggregate footer. Global bindings retain precedence on conflicts. The command's configured `scope` does not replace owner resolution; the selected item's feed provenance determines which owner context the script receives. Owner and feed identifiers are not available to scripts; use the public item fields and the parameters supplied by the host.

### 5. Return One Typed Response

Command responses must contain one complete version-1 operation matching the command's declared `type`:

```json
{
  "version": 1,
  "operation": {
    "type": "run",
    "mode": "foreground",
    "argv": ["printf", "done\n"],
    "exit": true
  }
}
```

Use stderr for diagnostics. Extra stdout, unknown fields, a second JSON document, malformed JSON, or a mismatched operation type fails the command without applying a partial operation.

## Environment and Limits

Producer scripts inherit the caller's environment. Directory workflows receive `WORKFLOW_DIR`; producer runtime data arrives through JSON stdin rather than launcher-specific environment variables. The standard timeout is 10 seconds, default producer stdout is 1 MiB, Picker item stdout is 64 MiB, and stderr is 64 KiB.

For exact request and response schemas, see [Producer Protocol](../reference/producer-protocol.md). For operation targets, query binding, calls, and returns, see [View Navigation and Popups](view-navigation-and-popups.md).
