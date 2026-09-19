---
title: "Build a Native Form View"
type: "guide"
tags:
  - forms
  - workflows
  - return
description: "Define a typed native form, validate its fields, and return structured values to a caller or shell."
---

# Build a Native Form View

## Problem

You want to collect typed values interactively and return them as JSON to a calling view or shell command.

## Solution

Declare a form engine and its fields in the workflow. A form field can be required, typed, initialized with a value, and edited with ordinary form navigation. Enter submits when every field is valid; Escape cancels the view.

The complete example is in [the form workflow](../../tests/fixtures/config/workflows/form/workflow.toml). Its [content.py](../../tests/fixtures/config/workflows/form/scripts/content.py) demonstrates dynamic content: the script receives a `form-content` request and returns the field specification from `context.parameters.spec`. The [submit.py](../../tests/fixtures/config/workflows/form/scripts/submit.py) script receives the validated engine state and returns its typed values.

```toml
[views.input.query]
type = "object"
input_order = ["name", "environment", "enabled"]
name = { type = "string", default = "" }
environment = { type = "string", default = "dev" }
enabled = { type = "boolean", default = true }

[views.input.engine]
type = "form"
[views.input.engine.config.content]
producer = "script"
handler = { file = "scripts/content.py" }

[views.input.commands.submit]
key = "enter"
scope = "view"
type = "return"
producer = "script"
handler = { file = "scripts/submit.py" }
```

The query schema supplies typed defaults and `input_order` controls the positional order used by routes and CLI input. The form engine supplies the interactive fields. The submit command reads `context.engine.state.values`; those values are typed JSON, so a boolean field returns `true` or `false` rather than text. A failed validation keeps the form open and displays the field error.

To invoke the example directly:

```sh
cargo run --quiet -- --suite tests/fixtures/config/default.toml \
  form:input --name="Ada's project" --environment=prod --enabled=false
```

The successful JSON result is written to stdout. Redirect it when calling the launcher from a shell:

```sh
cargo run --quiet -- --suite tests/fixtures/config/default.toml \
  form:input --name="Ada's project" --environment=prod --enabled=false \
  > /tmp/tlaunch-form-result.json
```

Initial values can also come from a route, and omitted trailing values use the query defaults. Escape cancels with the view's configured `cancel_exit_code` of 1 and produces no result.

## Consume a returned form value

A caller can use a `call` command and a return processor:

```toml
[views.main.commands.open]
key = "enter"
type = "call"
producer = "declared"
[views.main.commands.open.handler]
target = "form:input"
query = { name = "demo", environment = "dev", enabled = true }

[views.main.commands.open.return_processor]
type = "navigate"
producer = "script"
[views.main.commands.open.return_processor.handler]
file = "scripts/received.py"
```

The processor receives the submitted object in `request["context"]["result"]`. A cancelled call skips the return processor and restores the caller.

## Troubleshooting

- Keep diagnostics off stdout when returning JSON; stdout must contain one JSON value.
- Use `scope = "view"` for form commands that operate on the active form.
- A required field or a field with an invalid type prevents submission until corrected.
- Dynamic content scripts must return a version 1 `content` response and should derive fields from the request context.
