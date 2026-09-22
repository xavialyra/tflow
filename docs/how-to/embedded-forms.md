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
name = { type = "string", default = "" }
environment = { type = "string", default = "dev" }
enabled = { type = "boolean", default = true }

[views.input.engine]
type = "form"
[views.input.engine.config.content]
producer = "script"
handler = { file = "scripts/content.py" }

[views.input.keymap]
enter = "submit"

[commands.submit]
label = "Submit"
type = "return"
producer = "script"
handler = { file = "scripts/submit.py" }
```

The query schema supplies typed defaults, and the form engine supplies the interactive fields. The submit command reads `context.engine.state.values`; those values are typed JSON, so a boolean field returns `true` or `false` rather than text. A failed validation keeps the form open and displays the field error.

To invoke the example directly:

```sh
cargo run --quiet -- --suite tests/fixtures/config/default.toml \
  form:input --name="Ada's project" --environment=prod --enabled=false
```

The successful JSON result is written to stdout. Redirect it when calling the launcher from a shell:

```sh
cargo run --quiet -- --suite tests/fixtures/config/default.toml \
  form:input --name="Ada's project" --environment=prod --enabled=false \
  > /tmp/tflow-form-result.json
```

Initial values can also come from a route, and omitted trailing values use the query defaults. Escape cancels with the view's configured `cancel_exit_code` of 1 and produces no result.

## Consume a returned form value

A caller can use a `call` command and a return processor:

```toml
[views.main.keymap]
enter = "open"

[commands.open]
label = "Open form"
type = "call"
producer = "declared"
[commands.open.handler]
target = "form:input"
query = { name = "demo", environment = "dev", enabled = true }

[commands.open.return_processor]
type = "navigate"
producer = "script"
[commands.open.return_processor.handler]
file = "scripts/received.py"
```

The processor receives the submitted object in `request["context"]["result"]`. A cancelled call skips the return processor and restores the caller.

## Troubleshooting

- Keep diagnostics off stdout when returning JSON; stdout must contain one JSON value.
- Bind a form command in the form View's keymap so it registers with View scope, which is what the footer and the command selector show.
- A required field or a field with an invalid type prevents submission until corrected.
- Dynamic content scripts must return a version 1 `content` response and should derive fields from the request context.
