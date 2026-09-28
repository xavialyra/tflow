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

*Invariants: Workflow imports, direct inter-workflow code dependencies, and workflow-declared global aliases are strictly prohibited. A workflow's owner id (the suite member ID, or the file stem for a standalone `-w` workflow) is the first segment of every command FQID it declares; see [Workflow Commands](#workflow-commands-commandsid) for how that FQID index stays unique.*

---

## View Declarations (`[views.<name>]`)

A workflow defines one or more Views referenced as `<workflow-id>:<name>`.

```toml
[views.main]
binding_mode = "view"

[views.main.query]
type = "object"
mode = { type = "enum", options = ["normal", "compact"], default = "normal" }
```

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `engine` | table | `{ type = "picker" }` | Defines the View's UI engine and its engine-specific configuration. |
| `query` | table | `{}` | Parameter schema defining the view's expected launch parameters. Validated before mounting. |
| `bindings` | table | `{}` | Key-centric bindings: the physical key maps to a workflow command or an explicit `@engine:<engine>.<action>` engine action. |
| `unbind` | table | `{ keys = [], commands = [], layers = [] }` | Per-View unbinding: physical keys, command addresses, and priority layers. |
| `binding_mode` | string | `"view"` | Bindings resolution strategy: `"view"` or `"item_merge"`. |

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

### Binding Modes (`binding_mode`)

- `"view"` (Default): The View's `[views.<name>.bindings]` completely defines its own layer on top of the Engine and Host layers. An omitted or empty table simply contributes no View-layer bindings.
- `"item_merge"`: Designed for aggregate pickers. The View's bindings serve as a base layer. The focused item's dynamic `bindings` overlay (`item.bindings`) overrides keys individually per focused item. Because item bindings originate from runtime producer JSON rather than static configuration, they are evaluated leniently so that malformed item entries do not crash the Picker:
  - Values must be command address strings (e.g. `"enter": "apps.open"`, `"escape": "@engine:picker.exit"`). Non-string values (such as `false` or numbers) and unresolvable targets are ignored without breaking the view.
  - Keys that cannot be parsed as valid physical keys degrade gracefully: the command remains registered as a keyless View-layer entry (invokable by ID and in the command palette), but no key triggers it.
  - Removing an inherited key remains the View's own prerogative via `[views.<name>.unbind]`. Items can rebind existing keys or bind new ones, but cannot suppress base or engine keys.

### View Bindings and Unbinding (`[views.<name>.bindings]`)

A View binding table is key-centric: each TOML key is a physical key and each value identifies a command. Four spellings resolve to the same workflow command — a bare local name (`"enter" = "open"`), a fully-qualified id (`"enter" = "mytools.open"`), or either with the owner made explicit (`"enter" = "@workflow:open"` / `"enter" = "@workflow:mytools.open"`). A bare value resolves **only** to a command of the current workflow and never to an engine action. To target a built-in engine action, use the `@engine:<engine>.<action>` form. The same command may be reached by several keys:

```toml
[views.main.bindings]
"enter" = "open"
"ctrl+o" = "open_detached"
"ctrl+p" = "@engine:picker.toggle_preview"
```

Every value names a command or an engine action, so a View binding always runs something. To take a key *away* from a lower layer, release it with `unbind` below — a View binding is never a bare boolean.

Three statements about a key are easy to confuse, so they stay separate mechanisms:

| Written as | Where | Meaning |
| :--- | :--- | :--- |
| `"enter" = "open"` | `[views.<name>.bindings]` | This View runs the named command on that key. |
| `unbind` with `keys` / `commands` / `layers` | `[views.<name>.unbind]` | The key↔command association is released. The key is no longer claimed by any layer, while the command keeps its identity: it stays discoverable and invokable by id. |
| `"ctrl+k" = false` | Engine default tables (`[picker.bindings]`, `[capture.bindings]`, `[embedded.bindings]`, `[form.bindings]`) and `[host.bindings]` | That layer declares no binding for the key. There is no entry to shadow or release, so the key falls through to raw input; if that was the command's only binding, it is no longer published by that layer and can no longer be invoked by id either. |

`unbind` is a per-View table whose three fields map to three independent axes. It no longer overloads one string list with a sigil: a physical key is not a command, and a priority layer is not an address.

```toml
[views.main.unbind]
keys = ["ctrl+u"]
commands = ["@engine:picker.clear_input", "core.page"]
layers = ["host"]
```

- `keys`: the command bound to that physical key loses it. That key then reaches raw input handling (text entry, or the child process of an `embedded` View) instead of being claimed by anything — `unbind` releases a key, it never swallows it.
- `commands`: the addressed command (`core.page`, `@engine:picker.clear_input`, or a bare current-workflow name — the same grammar as binding values) loses all of its keys. The command stays discoverable and invokable by identity from command selectors, and a lower layer's binding for that key can take over.
- `layers`: the priority layer (`view`, `engine`, or `host`) is ignored in this View.

`layers = ["host"]` is the recommended isolation for modal views (such as `__commands:main` and `__parameters:main`): it drops the host layer without hardcoding external workflow FQIDs or guessing user key remappings, so host shortcuts (`ctrl+k`, `ctrl+g`) cannot re-enter a modal layer. To release a single key, list it in `keys` — for example `keys = ["escape"]` lets an `embedded` child process receive Escape instead of cancelling the View.

### View Chrome & Presentation Controls

For Picker views, the following presentation options can be configured directly under the View:

| Option | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `show_input` | boolean | `true` | When `false`, hides the query input bar entirely. |
| `show_divider` | boolean | `true` | When `false`, removes the divider line beneath the input bar. |
| `show_left_prefix` | boolean | `true` | When `false`, hides the left prefix and disables `left_prefix_backspace`. Useful for popup views. |
| `chrome_commands_show` | array of strings | Inherited (`["enter", "ctrl+k"]`) | Physical keys whose bound command labels are advertised in the footer, in order. Keys with no bound command — including keys removed by `unbind` — are skipped. Set to `[]` to hide all footer hints, or a subset (e.g. `["enter"]`) to further constrain modal hints. |
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

*Note: By default, `Escape` triggers the engine's `cancel` action to close the view. To let Escape pass through into the child process, release the inherited binding in the View: `[views.<name>.unbind] keys = ["escape"]`.*

---

## Workflow Commands (`[commands.<id>]`)

Commands represent executable operations owned by the workflow root. A workflow-level command is addressed by its fully qualified ID: `<workflow-id>.<command-id>` (dot-delimited). Command IDs may not contain dots, whitespace, or `@`.

The command index is the set of FQIDs, and every FQID must name exactly one command. Built-in engine actions own `<engine>.<action>` (`picker.exit`, `form.focus_next`, `capture.copy`, `embedded.cancel`), so a workflow command may not take one of those ids even when the workflow itself is named after the engine — a workflow called `form` may declare `open`, but not `exit` or `focus_next`. Reusing an engine action's id is rejected at load time instead of being resolved by priority.

```toml
[commands.open]
label = "Open Selected"
type = "run"
producer = "declared"

[commands.open.handler]
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
| `presentation` | table | Optional | Modal presentation table (see [Modal Presentation Options](#modal-presentation-options-presentation)). |
| `replace` | boolean | `false` | When `true`, replaces current View in history instead of pushing onto the stack. |
| `clear_input` | boolean | `false` | When `true`, clears the active query buffer before navigating. |

*Focus Hint*: Setting `__focus = "<item_value>"` inside `query` instructs a target Picker to automatically scroll to and select that item.

### 2. `call`
Transitions to a child View and establishes a return boundary:

| Parameter | Type | Required / Default | Description |
| :--- | :--- | :--- | :--- |
| `target` | string | **Required** | Destination child view. |
| `query` | value | Optional (`{}`) | Parameters for the child view. |
| `presentation` | table | Optional | Modal presentation table (see [Modal Presentation Options](#modal-presentation-options-presentation)). |

#### Modal Presentation Options (`presentation`)

When mounting a View as a modal overlay, configure the `presentation` table:

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `mode` | string | `"inline"` | `"popup"` mounts the View as a centered or anchored modal dialog. |
| `anchor` | string | `"center"` | 9-box viewport anchor: `"center"`, `"top"`, `"bottom"`, `"left"`, `"right"`, `"top-left"`, `"top-right"`, `"bottom-left"`, `"bottom-right"`. |
| `offset_x` | integer | `0` | Horizontal offset in terminal columns from the anchor boundary. |
| `offset_y` | integer | `0` | Vertical offset in terminal rows from the anchor boundary. |
| `width` | integer / string | `72` | Width in terminal cells (e.g. `72`) or viewport percentage string (e.g. `"80%"`). |
| `height` | integer / string | `16` | Height in terminal cells (e.g. `18`) or viewport percentage string (e.g. `"50%"`). |
| `min_width` | integer | None | Minimum width in terminal cells clamp. |
| `max_width` | integer | None | Maximum width in terminal cells clamp. |
| `min_height`| integer | None | Minimum height in terminal cells clamp. |
| `max_height`| integer | None | Maximum height in terminal cells clamp. |
| `show_title` | boolean | `true` | When `false`, suppresses rendering the view title/label on the popup top border. |

### 3. `return`
Closes the current View and returns a value to the caller boundary:

| Parameter | Type | Required / Default | Description |
| :--- | :--- | :--- | :--- |
| `value` | value | **Required** | JSON/TOML value passed back to caller's `return_processor`. Can be explicit `null`. |

### 4. `run`
Executes an external system command:

| Parameter | Type | Required / Default | Description |
| :--- | :--- | :--- | :--- |
| `argv` | array of strings | **Required** | Command and arguments to execute. |
| `exit` | boolean | `false` | When `true`, terminates `tflow` upon completion. |
| `success_message` | string | Optional | Message displayed in the footer for 3 seconds upon successful return. |
| `timeout_ms` | integer | Optional | Timeout in milliseconds; triggers process group termination when exceeded. |

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
