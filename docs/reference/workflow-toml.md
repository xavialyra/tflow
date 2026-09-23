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

Workflows define custom Views, keybindings, actions, and Engines. They are mounted explicitly by a [Suite Manifest](suite-toml.md) or run directly in standalone mode with `tflow -w <PATH>`.

---

## Workflow Layouts

`tflow` supports two physical workflow packaging layouts:

| Layout | File Location | Scripts & Assets | Child Env Var | Best For |
| :--- | :--- | :--- | :--- | :--- |
| **Single-File** | `workflows/<id>.toml` | Relative external scripts are **prohibited**. Use inline scripts or host system binaries. | None | Simple tools, quick shortcuts, self-contained shell snippets. |
| **Directory** | `workflows/<id>/workflow.toml` | Relative script files and assets allowed, strictly confined to the workflow directory. | `TFLOW_WORKFLOW_DIR` | Complex multi-file workflows, custom scripts (Python, Bash), local assets. |

---

## Workflow Header (`[workflow]`)

Every workflow manifest must begin with the `[workflow]` section:

```toml
[workflow]
api = 1
name = "Applications"
entrypoint = "main"
```

| Field | Type | Required / Default | Description |
| :--- | :--- | :--- | :--- |
| `api` | integer | Optional (default: `1`) | Workflow API specification version. Must be `1`. |
| `name` | string | **Required** | Descriptive, human-readable name of the workflow. |
| `entrypoint` | string | **Required** | The ID of the default View within this workflow to open first. |

*Invariants: Workflow imports, direct inter-workflow code dependencies, and workflow-declared global aliases are strictly prohibited.*

---

## View Declarations (`[views.<name>]`)

A workflow defines one or more Views referenced as `<workflow-id>:<name>`.

```toml
[views.main]
keymap_mode = "view"

[views.main.query]
type = "object"
mode = { type = "enum", options = ["normal", "compact"], default = "normal" }
```

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `engine` | table | `{ type = "picker" }` | Defines the View's UI engine and its engine-specific configuration. |
| `query` | table | `{}` | Parameter schema defining the view's expected launch parameters. Validated before mounting. |
| `keymap` | table | `{}` | Key-to-command mappings active when this View is focused. |
| `keymap_mode` | string | `"view"` | Keymap resolution strategy: `"view"` or `"item_merge"`. |

### Query Parameter Schemas (`[views.<name>.query]`)

The query schema declares the expected parameters for launching the View (via CLI flags or navigation `query` objects):

- `type`: Either `"string"` (for unstructured plain input) or `"object"`.
- `input`: Optional string naming the field that receives positional CLI input and interactive text input. Must be of type `string`.
- Field definitions:
  - `type`: Parameter type. Accepted types are `string`, `integer`, `number`, `boolean`, `enum`, `object`, and `array<T>`.
  - `options`: Required array of unique, nonempty strings when `type = "enum"`; disallowed for other types.
  - `default`: Optional default value when omitted.
  - `nullable`: Optional boolean (`false` by default).

CLI arguments matching fields in an object query (e.g. `--mode=compact`) are parsed and validated against this schema. Invalid options or values are rejected with diagnostic errors.

### Keymap Modes (`keymap_mode`)

- `"view"` (Default): The View's `[views.<name>.keymap]` completely defines its keybindings. If the table is omitted or empty, fallback keys declared on the workflow's commands (`[commands.<id>].key`) are published.
- `"item_merge"`: Designed for aggregate pickers. The View's keymap serves as a base layer. The focused item's dynamic `bindings` override keys individually. Command-level fallback keys do not apply.

### View Chrome & Presentation Controls

For Picker views, the following presentation options can be configured directly under the View:

| Option | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `show_input` | boolean | `true` | When `false`, hides the query input bar entirely. |
| `show_divider` | boolean | `true` | When `false`, removes the divider line beneath the input bar. |
| `show_left_prefix` | boolean | `true` | When `false`, hides the left prefix and disables `left_prefix_backspace`. Useful for popup views. |
| `input_placeholder`| string | Unset | Literal placeholder rendered in muted styling when the query buffer is empty. Presentation only. |
| `source_badge` | boolean | `true` | In aggregate Pickers, controls whether the source feed badge is displayed on external items. |

---

## Engine Configuration (`[views.<name>.engine]`)

Every View is powered by one of four built-in engines: `picker`, `capture`, `form`, or `embedded`.

### 1. Picker Engine (`type = "picker"`)

The Picker engine renders a query-driven candidate list with an optional preview pane. Query input is dispatched to the items producer; filtering and ranking are handled by producer scripts.

#### Items Configuration (`[views.<name>.engine.config.items]`)

##### Option A: Static Item Array
```toml
[views.main.engine]
type = "picker"

[views.main.engine.config]
items = [
  { display = "Show date", value = "date", metadata = {} },
  { display = "System info", value = "info", metadata = {} },
]
```

##### Option B: Dynamic Script Producer
```toml
[views.main.engine.config.items]
producer = "script"

[views.main.engine.config.items.handler]
file = "scripts/items.sh"
```

##### Option C: Declared Producer
```toml
[views.main.engine.config.items]
producer = "declared"

[views.main.engine.config.items.handler]
items = [
  { display = "Show date", value = "date" },
]
```

#### Preview Pane Configuration

| Option | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `preview` | table | `{ inherit = true }` | Preview producer definition (`script`, `declared`, or `{ inherit = true }`). |
| `preview_default_open` | boolean | `false` | When `true`, the preview pane starts open instead of collapsed. |
| `preview_ratio` | float | `0.5` | Ratio of the terminal allocated to preview (when visible). |
| `preview_min_width` | integer | `30` | Minimum column width required to display the preview pane. |

---

### 2. Capture Engine (`type = "capture"`)

Renders read-only text output or command results.

```toml
[views.output.engine]
type = "capture"

[views.output.engine.config.output]
producer = "script"

[views.output.engine.config.output.handler]
file = "scripts/output.sh"
```

The script receives a `capture-output` JSON request and returns `{"version": 1, "output": "..."}`.

---

### 3. Form Engine (`type = "form"`)

Renders interactive editable form fields from a `content` producer:

```toml
[views.input.engine]
type = "form"

[views.input.engine.config.content]
producer = "declared"

[views.input.engine.config.content.handler]
fields = [
  { id = "name", label = "Name", type = "text", required = true },
]
```

See [Form Content and State](form.md) for full field schemas and validation rules.

---

### 4. Embedded Engine (`type = "embedded"`)

Spawns and renders an interactive child terminal (PTY) inside `tflow`:

```toml
[views.terminal.engine]
type = "embedded"

[views.terminal.engine.config]
command = ["btop"]
```

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `command` | array of strings | **Required** | The executable and arguments to spawn inside the PTY. |

*Note: By default, `Escape` triggers the engine's `cancel` action to close the view. To let Escape pass through into the child process, disable the binding in the View's keymap: `[views.<name>.keymap] "escape" = false`.*

---

## Workflow Commands (`[commands.<id>]`)

Commands represent executable operations owned by the workflow root.

```toml
[commands.open]
label = "Open Selected"
type = "run"
producer = "declared"

[commands.open.handler]
mode = "foreground"
argv = ["xdg-open"]
exit = true
```

### Command Definition Schema

| Field | Type | Required / Default | Description |
| :--- | :--- | :--- | :--- |
| `label` | string | **Required** | Display name shown in command palettes and UI chrome. |
| `type` | string | **Required** | Operation type: `"navigate"`, `"call"`, `"return"`, or `"run"`. |
| `producer` | string | **Required** | Producer kind: `"declared"` (static payload) or `"script"` (dynamic output). |
| `handler` | table | **Required** | Handler payload corresponding to `producer` and `type`. |
| `key` | string | Optional | Fallback physical keybinding (e.g. `"enter"`). Only used when View keymap is empty and `keymap_mode = "view"`. |
| `return_processor`| table | Optional | Handler invoked after a `call` operation returns to this caller. |

---

## Operation Payloads

Declared handlers provide the operation payload directly in TOML. Script handlers return this payload as JSON in `operation`:

### 1. `navigate`
Transitions the session to another View:

| Parameter | Type | Required / Default | Description |
| :--- | :--- | :--- | :--- |
| `target` | string | **Required** | Destination view selector (`"<view>"` or `"<workflow>:<view>"`). |
| `query` | value | Optional (`{}`) | Parameters passed to the destination View's query schema. |
| `presentation` | table | Optional | `{ mode = "popup", width = 70, height = 18 }`. Mounts as a modal popup. |
| `replace` | boolean | `false` | When `true`, replaces current View in history instead of pushing onto the stack. |
| `clear_input` | boolean | `false` | When `true`, clears the active query buffer before navigating. |

*Focus Hint*: Setting `__focus = "<item_value>"` inside `query` instructs a target Picker to automatically scroll to and select that item.

### 2. `call`
Transitions to a child View and establishes a return boundary:

| Parameter | Type | Required / Default | Description |
| :--- | :--- | :--- | :--- |
| `target` | string | **Required** | Destination child view. |
| `query` | value | Optional (`{}`) | Parameters for the child view. |
| `presentation` | table | Optional | Modal presentation options (`mode`, `width`, `height`). |

### 3. `return`
Closes the current View and returns a value to the caller boundary:

| Parameter | Type | Required / Default | Description |
| :--- | :--- | :--- | :--- |
| `value` | value | **Required** | JSON/TOML value passed back to caller's `return_processor`. Can be explicit `null`. |

### 4. `run`
Executes an external system command:

| Parameter | Type | Required / Default | Description |
| :--- | :--- | :--- | :--- |
| `mode` | string | `"foreground"` | Execution mode. Currently `"foreground"`. |
| `argv` | array of strings | **Required** | Command and arguments to execute. |
| `exit` | boolean | `false` | When `true`, terminates `tflow` upon completion. |
| `success_message` | string | Optional | Message displayed in the footer for 3 seconds upon successful return. |

---

## Producer Handlers (`handler`)

A producer handler specifies how dynamic data is generated:

### File Handler
```toml
[commands.open.handler]
file = "scripts/open.sh"
```
*Note: In directory workflows, `file` paths are relative to the workflow root. In single-file workflows, external relative paths are forbidden.*

### Inline Script Handler
```toml
[commands.open.handler]
script = '''#!/usr/bin/env python3
import json, sys
req = json.load(sys.stdin)
json.dump({
    "version": 1,
    "operation": {
        "type": "run",
        "mode": "foreground",
        "argv": ["notify-send", "Launched", req["context"]["parameters"].get("action", "")],
        "exit": False,
    }
}, sys.stdout)
'''
```

---

## Return Processors (`return_processor`)

When a `call` operation finishes, the parent view can execute a return processor before resuming:

```toml
[commands.choose.return_processor]
type = "navigate"
producer = "script"

[commands.choose.return_processor.handler]
file = "scripts/process-result.sh"
```

The processor script receives a JSON payload on stdin containing `context.result` (the child's return value) and outputs a version-1 operation.

---

## Runtime Limits & Child Process Environment

### Environment Variables
- `TFLOW_WORKFLOW_DIR`: Set to the absolute root directory of a directory-based workflow.
- `TFLOW_INPUT`: Rendered input buffer text (provided to Embedded processes only).

### Execution Resource Limits
- Default script timeout: **10 seconds**.
- Standard stdout limit: **1 MiB** (expanded to **64 MiB** for Picker items producers).
- Standard stderr limit: **64 KiB**.
- Multi-line inline scripts are materialized under `$XDG_RUNTIME_DIR/tflow/scripts/` with `0600` permissions.
