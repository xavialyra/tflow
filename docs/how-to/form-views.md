---
title: "Build Native Dynamic Forms"
type: "guide"
tags:
  - form
  - query
  - commands
description: "Run native forms, generate editable fields from query parameters, and return results through ordinary commands."
---

# Build Native Dynamic Forms

## Goal

Collect structured values in a native terminal form whose fields depend on launch parameters. Keep the View query static, interpret its contents in a producer script, and use a normal command to return the edited values.

## Run the Examples

From the repository root:

```sh
cargo run -- --suite tests/fixtures/config/default.toml form:input
```

Enter a project name, use Tab to move between fields, and press Enter to submit. Space toggles a boolean field; Ctrl+U clears the current field. Esc returns to the preserved draft.

Run the form example with structured arguments:

```sh
cargo run -- --suite tests/fixtures/config/default.toml form:input --environment=prod --enabled=false
```

The runnable files are in [the form fixture](../../tests/fixtures/config/workflows/form/workflow.toml).

## Generate Content from a Fixed Query

Declare an object parameter and a script content producer:

```toml
[views.edit.query]
type = "object"
spec = { type = "object" }

[views.edit.engine]
type = "form"

[views.edit.engine.config.content]
producer = "script"
handler = { file = "scripts/content.py" }
```

In a directory workflow, create `scripts/content.py`:

```python
#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
content = request["context"]["parameters"]["spec"]
json.dump({"version": 1, "content": content}, sys.stdout)
```

A caller passes a `spec` object containing a `fields` array. It can build that object from a selected item, a known query contract, or other runtime data. The Form Engine receives only the resulting editable content.

For a string query, the script can instead interpret its own convention:

```python
spec = request["context"]["parameters"]
fields = []
for part in spec.split(","):
    name, kind, raw = part.split(":", 2)
    value = raw if kind == "string" else json.loads(raw)
    fields.append({"name": name, "type": kind, "value": value})
json.dump({"version": 1, "content": {"fields": fields}}, sys.stdout)
```

This accepts `a:string:dd,b:number:null`. The compact example has no comma escaping; use an object parameter or a richer script parser when values need that delimiter. The launcher does not reserve or parse this string syntax.

## Return Edited Values with a Command

Add a standard return command:

```toml
[views.edit.commands.submit]
key = "enter"
label = "Submit"
scope = "view"
type = "return"
producer = "script"
handler = { file = "scripts/submit.py" }
```

Create `scripts/submit.py`:

```python
#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
state = request["context"]["engine"]["state"]
if not state["valid"]:
    raise SystemExit("Please correct the form fields")
json.dump({
    "version": 1,
    "operation": {"type": "return", "value": state["values"]},
}, sys.stdout)
```

The query in `context.parameters` remains the original input. The edited result comes from `context.engine.state.values`; partial input remains available in `drafts`. Checking `valid` is part of this command's policy, so other commands can still navigate while a field is invalid.

To edit another View's query, have its command call this form with field descriptions and current values, then use a return processor to navigate with the returned query. See [View Navigation & Popups](view-navigation-and-popups.md) for the call/return configuration and [Form Content and State](../reference/form.md) for the complete field and state contract.
