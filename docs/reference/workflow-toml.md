---
title: "workflow.toml Specification"
type: "reference"
tags:
  - workflow
  - views
  - engine
  - commands
  - producers
  - schema
description: "Authoritative reference for workflow manifests, static View configuration, producer handlers, and the version-1 JSON protocol."
---

# workflow.toml Specification

Workflows define custom Views, keybindings, actions, and Engines. They reside under `$XDG_CONFIG_HOME/tlaunch/workflows/` and support two physical layouts:

1. **Single-file workflow**: `workflows/<id>.toml`. The workflow ID is the file stem. Relative external script files are not allowed; use inline producer scripts or absolute host binaries.
2. **Directory workflow**: `workflows/<id>/workflow.toml`. The directory is the workflow root. Relative script files are confined to that root, and `$WORKFLOW_DIR` is provided to child processes.

## Workflow Header

```toml
[workflow]
api = 1
name = "Applications"
```

- `api` is an optional integer and defaults to `1`.
- `name` is the required human-readable workflow name.

## Style Slots

`[styles.<slot>]` declares a workflow custom style, with an optional `[styles.<slot>.selected]` table. Color fields accept `ansi:NAME`, `#RRGGBB`, or `scheme:NAME`; for example, `foreground = "scheme:accent"`. The active theme merges `[workflows.<workflow-id>.styles.<slot>]` over these defaults field by field before resolving colors. Omitted selected fields inherit, and explicit `false` or `ansi:reset` values override defaults. See the [theme specification](theme-toml.md) for fields and selection background rules.

## Views

A workflow defines one or more Views referenced as `<workflow-id>:<name>`:

```toml
[views.main]
alias = "apps"

[views.main.query]
type = "object"
mode = { type = "string", default = "normal" }
```

`alias` is optional and must be globally unique. Query fields are validated before a target View is mounted.

## Engine Configuration

Every View has one of the three built-in Engines. Route definitions, query schemas, Engine types, Picker pane sizing, and keymaps are host-owned static configuration. The initial producer protocol cannot redefine them. There is no workflow `title` configuration; the footer uses the host-owned View alias or canonical reference.

### Picker

A static Picker list uses an array of item objects:

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

A dynamic list uses an explicit producer:

```toml
[views.main.engine.config.items]
producer = "script"

[views.main.engine.config.items.handler]
file = "scripts/items.sh"
```

A declared list uses the same envelope and keeps the complete list in TOML:

```toml
[views.main.engine.config.items]
producer = "declared"

[views.main.engine.config.items.handler]
items = [
  { display = "Show date", value = "date" },
]
```

The Picker items script receives a `picker-items` request on stdin and writes one response object to stdout:

```json
{"version":1,"entrypoint":"picker-items","context":{"parameters":{},"input":{"stdin":{"path":null,"length":0,"is_tty":true}},"engine":{"type":"picker","state":{"input":"","item":null,"text":null,"value":null,"metadata":null,"selected_index":0}}}}
```

```json
{"version":1,"items":[{"display":"Show date","value":"date","metadata":{}}]}
```

The response replaces the complete collection for that feed request. The host owns feed composition, selection state, and feed provenance. Aggregate picker pages automatically render the source feed alias as a right-aligned `badge` on the first row of each external feed item. Set `source_badge = false` on the aggregate View to disable this decoration. Feed identity and scheduling data are not sent automatically. Picker item stdout is bounded at 64 MiB to support large candidate sets.

Every Picker provides a preview pane, initially collapsed and toggled with `Ctrl+P` by default. Without a custom provider it displays the selected item's plain-text display and value. Metadata remains available to custom preview providers. Preview data sources are configured independently from document rendering. `preview = { producer = "script", handler = { file = "scripts/preview.py" } }` receives a `picker-preview` request and returns `{"version":1,"preview":...}`. `preview = { producer = "declared", document = "Fixed text" }` supplies a static document. An aggregate page may use `preview = { inherit = true }` to select the source feed’s provider.

`preview_ratio` and `preview_min_width` control the automatic outer items/preview split. `preview_default_open` controls initial visibility and defaults to false. There is no user-defined outer layout. Omitting `preview` is equivalent to `preview = { inherit = true }`: aggregate pages use the selected feed's provider when present, otherwise the host displays built-in details. Non-aggregate pages can also use omission or explicit inheritance for built-in details. An explicit page provider overrides the feed and built-in details; null responses stay empty and errors stay visible. Parameters, relative script/image paths, and custom styles resolve in the provider owner's workflow. Documents support strings, item displays, wrapped rich paragraphs, images, separators, and nested internal layouts. See [Picker Preview Documents and Producers](picker-preview.md) for exact fields, validation, ownership, and resource limits.

### Capture

Capture requires an `output` field and accepts a literal string:

```toml
[views.output.engine]
type = "capture"

[views.output.engine.config]
output = "fixed text"
```

A declared output or a script-backed output uses the producer envelope:

```toml
[views.output.engine.config.output]
producer = "script"

[views.output.engine.config.output.handler]
file = "scripts/output.sh"
```

The script runs after the Capture View has mounted. It receives a `capture-output` request and writes one response:

```json
{"version":1,"entrypoint":"capture-output","context":{"parameters":{"action":"date"},"input":{"stdin":{"path":null,"length":0,"is_tty":true}},"engine":{"type":"capture","state":null}}}
```

```json
{"version":1,"output":"System date output\n"}
```

A provider can change only the displayed output. On provider failure, the Capture View remains mounted and displays the diagnostic.

### Embedded

Embedded owns an interactive child process and renders its PTY output:

```toml
[views.terminal.engine]
type = "embedded"

[views.terminal.engine.config]
command = ["btop"]
escape-cancels = true
```

Embedded process configuration is static View configuration. Dynamic Embedded argv is not part of the initial producer protocol.

### Child Process Environment

Child processes inherit the caller's environment. The host adds only these workflow-facing variables:

- `WORKFLOW_DIR`: the absolute root of a directory workflow, provided to workflow scripts, foreground commands, and Embedded processes.
- `LAUNCHER_INPUT`: the current rendered input text, provided to Embedded processes only.

Producer scripts receive runtime data through their documented JSON request on stdin. They do not receive query, selection, command, or View state through launcher-specific environment variables.

## Commands

Commands are attached to a View and use one of `navigate`, `call`, `return`, `run`, `edit-input`, or `invoke`:

```toml
[views.main.commands.open]
key = "enter"
label = "Open"
type = "navigate"
producer = "declared"

[views.main.commands.open.handler]
target = "sys:output"
query = { action = "date" }
replace = false
```

A declared handler is parsed as the operation payload for the command's declared `type`. A producer command contains only its binding metadata, operation `type`, `producer`, and matching `handler`.

### Operation Payloads

The following fields are supported in declared handlers:

- `navigate`: `target` (required string), optional `query` JSON/TOML value, optional `presentation` table, and optional `replace` boolean.
- `call`: `target` (required string), optional `query`, and optional `presentation` table. A call creates a return boundary.
- `return`: required `value`. An omitted value is invalid; a script response may use `"value": null` for a successful null result.
- `run`: `mode = "foreground"`, non-empty `argv`, optional `exit` boolean, and optional `success_message` string. After successful execution, the host records this message at `INFO` level and displays it in the source View's footer (or popup bottom border) if that View remains active. Failed or cancelled execution does not emit the success message. With `exit = true`, the message is logged before exit; the UI does not pause to display it. Errors take display priority. Informational messages expire after 3 seconds; the next input or a change of active View clears them earlier. Each new informational message replaces the previous one and restarts the timeout. Expiration only clears the display; recorded logs are retained.
- `edit-input`: `value` string and optional non-negative `cursor` byte offset at a UTF-8 boundary.
- `invoke`: `command` object containing the opaque command reference `{ view = "...", id = "..." }`.

Popup presentation is declared in the operation handler:

```toml
[views.main.commands.actions]
key = "ctrl+o"
label = "Actions"
type = "call"
producer = "declared"

[views.main.commands.actions.handler]
target = "selectors:actions"
presentation = { mode = "popup", width = 70, height = 18 }
```

### Script Command Producers

A script command uses a literal script handler. It receives one JSON request on stdin and must write exactly one version-1 response on stdout:

```toml
[views.main.commands.open]
key = "enter"
label = "Open selected item"
type = "run"
producer = "script"

[views.main.commands.open.handler]
file = "scripts/open.sh"
```

The request contains a `command` object and unified `context` fields: `context.parameters` is the command owner's bound parameter object, `context.input` is the explicit launch input descriptor, and `context.engine = {type, state}` is the carrying Engine's public state projection. Picker selection is available as `context.engine.state.item`; internal feed ownership and scheduling fields are omitted.

For an aggregate Picker, a command with a physical `key` from the View that owns the current selected feed item is projected into the aggregate command area. The projected command receives that feed owner's bound parameter snapshot in `context.parameters`; a command declared by the aggregate Picker View receives the aggregate View's parameters instead. The public `context.engine.state.item` remains the normalized selected item in both cases. Owner and feed identifiers are host-owned and are not added to the script request. The host revalidates the current result and provenance before dispatching a projected command.

A script response must have this shape:

```json
{
  "version": 1,
  "operation": {
    "type": "run",
    "mode": "foreground",
    "argv": ["printf", "selected:%s\\n", "value"],
    "exit": true
  }
}
```

The response operation type must match the command's declared `type`. Command scripts do not write interactive terminal output; generated `run` operations enter the existing foreground execution path.

### Return Processors

A `call` command may declare a processor that runs after the child has returned and the caller has been restored:

```toml
[views.main.commands.choose]
key = "enter"
label = "Choose"
type = "call"
producer = "declared"

[views.main.commands.choose.handler]
target = "selectors:actions"

[views.main.commands.choose.return_processor]
type = "navigate"
producer = "script"

[views.main.commands.choose.return_processor.handler]
file = "scripts/process-result.sh"
```

The processor script receives a `return` request. Its `result` is raw protocol data:

```json
{
  "version": 1,
  "entrypoint": "return",
  "context": {
    "parameters": {},
    "input": {"stdin":{"path":null,"length":0,"is_tty":true}},
    "engine": {"type":"picker","state":{"input":"","item":{"text":"Show date","value":"date","metadata":{}},"text":"Show date","value":"date","metadata":{},"selected_index":0}}
  },
  "result": {"text":"Show date","value":"date","metadata":{}}
}
```

The processor returns the same versioned operation envelope used by commands. Return values are raw JSON, including strings, objects, arrays, numbers, booleans, and explicit JSON `null`. A missing return `value` is invalid. A close/cancel decision has no result and does not run the processor.

## Script Handlers

All new producer script handlers use exactly one of these forms:

```toml
[views.main.commands.open.handler]
file = "scripts/open.sh"
```

```toml
[views.main.commands.open.handler]
script = '''#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
json.dump({
    "version": 1,
    "operation": {
        "type": "run",
        "mode": "foreground",
        "argv": ["printf", "parameters:%s\\n", json.dumps(request["context"].get("parameters", {}))],
        "exit": True,
    },
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
'''
```

`file` and `script` cannot be combined, must be non-empty, and are literal configuration. In directory workflows, file paths are confined to the workflow root. In single-file workflows, use inline scripts or absolute file paths.

## Strict Protocol Rules

Producer stdout must contain exactly one complete JSON object. Surrounding whitespace is allowed, but diagnostics, a second JSON document, malformed JSON, unknown fields, unsupported `version`, a mismatched operation type, a nonzero exit status, or an invalid operation schema is failure. Diagnostics belong on stderr.

Requests use JSON stdin. The stable request fields are `version`, `entrypoint`, and a unified `context` containing `parameters`, `input`, and `engine`. Command requests add `command`; return-processor requests add raw `result`. The host applies navigation target resolution, query binding, command visibility, Engine support, cancellation, timeouts, output bounds, and stale-result checks.

The default script policy is a 10-second timeout, 1 MiB stdout, and 64 KiB stderr. Picker item producers use the 64 MiB stdout bound. Managed child processes are cancelled and reaped through the shared execution layer.

## Static Boundaries

Configuration without a producer is static: its values are deserialized and validated at startup. Runtime data is available through the documented request fields of a producer protocol. JSON is UTF-8 text, so NUL-delimited or non-UTF-8 command protocols require a separate binary-safe interface and are outside this contract.

## Multi-line Inline Scripts

Inline producer handlers are materialized under `$XDG_RUNTIME_DIR/tlaunch/scripts/` (falling back to `$XDG_CACHE_HOME/tlaunch/scripts/`) with `0600` permissions. The host parses shebang arguments, preserves the caller's `$PWD`, and injects `$WORKFLOW_DIR` for directory workflows.

For root configuration, see [config.toml Specification](config-toml.md). For literal values and runtime data boundaries, see [Producer Protocol](producer-protocol.md).
