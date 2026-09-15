---
title: "ADR 0002: Static Configuration and Script Boundaries"
type: "concept"
tags:
  - adr
  - architecture
  - configuration
  - scripts
  - navigation
description: "Define static configuration, declared or script producers, typed command operations, and bounded Picker and Capture data protocols."
---

# ADR 0002: Static Configuration and Script Boundaries

- **Status**: Accepted
- **Date**: 2026-09-08
- **Implementation status**: Implemented in the current tree; producer-backed command, Picker, Capture, and return-handler paths are authoritative.
- **Baseline**: `e58073c`
- **Scope**: Configuration compilation, command preparation, Picker items, Capture output, navigation parameters, and call/return processing.
- **Related decisions**: [ADR 0001](0001-decentralized-workflow-extensions.md). This decision supersedes its earlier dynamic configuration direction. Its workflow packaging, interpreter support, caller CWD, and theme decisions remain applicable.

## Context

Earlier configuration coupled runtime snapshots to Engine-specific field allowlists and action stages. The same runtime value could be reached through several public paths, while malformed or unavailable data failed only when a particular action ran.

More field contracts or additional configuration indirection would improve checks but add complexity. The accepted design keeps configuration static and concentrates runtime computation in explicit producers with fixed inputs and complete, validated outputs. JSON producers are UTF-8 text protocols; byte-oriented NUL or non-UTF-8 behavior requires a separate protocol and is not part of this decision.

## Decision

### 1. Literal configuration and a common producer structure

Ordinary TOML values are configuration data, including script bodies, metadata, and generated strings. The host applies the schema for each declared field and producer response.

All producer objects use:

```toml
producer = "declared" # or "script"

[handler]
# Configuration selected by producer and entry point.
```

- `declared`: the result is fully declared in TOML, validated and compiled at startup. The operation still executes at its trigger time.
- `script`: run a JSON protocol script at the entry point's defined request time and validate its response before applying it.
- `handler`: producer-specific configuration. Declared handlers use the corresponding payload schema; script handlers use a common script schema containing exactly one non-empty `file` or `script` field.

Reject unknown and incompatible fields. A declared navigation handler cannot contain `file`; a script handler cannot contain `target`.

Literal Picker items arrays and Capture output strings remain concise declared-producer shorthands. Normalize them into the same internal representation as declared results. Do not introduce a second field-reference language or a reactive configuration graph.

### 2. Initial entry points and static boundaries

| Entry point | Producer output | Invocation time |
| :--- | :--- | :--- |
| Command | One operation matching the declared command type | Actual command dispatch |
| Picker items | Complete items array | Each explicit items request |
| Picker preview | Data-only document or null | Debounced current selection, after host commit |
| Capture output | Output string | After the Capture View is committed and mounted |
| Return processor | One operation matching the declared processor type | After a successful call return restores the caller |

There is **no general View builder in the initial design**. Route definitions, query schemas, Engine type, preview pane sizing (`preview_ratio`, `preview_min_width`, and `preview_default_open`), preview provider configuration, and keymap remain static. The dedicated Picker preview producer may return nested internal document layouts. The host validates and renders that data; the producer cannot change the View definition or emit operations. Providers cannot redefine those fields or create undeclared routes. View labels remain host-owned aliases or canonical references; there is no workflow `title` field.

This narrows the earlier builder direction: dynamic outer View layout and parameterized Embedded argv are not provided by this model. A future dedicated process-configuration producer requires a separate decision; do not restore arbitrary interpolation to fill that gap.

### 3. Commands declare their operation type

A command uses `type`, `producer`, and `handler`:

```toml
[views.main.commands.open]
key = "enter"
label = "Open"
type = "navigate"
producer = "script"

[views.main.commands.open.handler]
file = "scripts/navigate-handler.sh"
```

The supported operation types remain `navigate`, `call`, `return`, and `run`. `type` restricts the response, not the script interpreter. A script response with a different `operation.type` fails validation.

Declared navigation remains simple:

```toml
[views.main.commands.date]
key = "enter"
label = "Show date"
type = "navigate"
producer = "declared"

[views.main.commands.date.handler]
target = "sys:output"
query = { action = "date" }
replace = false
```

A declared return supplies a literal value:

```toml
[views.main.commands.cancel]
key = "escape"
type = "return"
producer = "declared"

[views.main.commands.cancel.handler]
value = "cancelled"
```

Despite its identifier, this command produces a successful return containing `"cancelled"`; it is not a cancellation event.

A declared run supplies process arguments directly:

```toml
[views.main.commands.date]
key = "enter"
type = "run"
producer = "declared"

[views.main.commands.date.handler]
mode = "foreground"
argv = ["date"]
exit = true
```

Initially, `run.mode` accepts only `foreground`. Embedded remains a View Engine with its own PTY lifecycle. The mode field does not promise background or Embedded run operations.

Command protocol scripts produce operations rather than interactive terminal output. A generated run operation enters the existing foreground execution path after protocol validation; foreground programs retain their own terminal and stdio behavior.

### 4. Producer requests use one explicit context

A command script receives one request on stdin:

```json
{
  "version": 1,
  "entrypoint": "command",
  "command": {"id": "open", "type": "navigate"},
  "context": {
    "parameters": {"mode": "normal"},
    "input": {
      "stdin": {"path": null, "length": 0, "is_tty": true}
    },
    "engine": {
      "type": "picker",
      "state": {
        "input": "Example app",
        "item": {
          "text": "Example app",
          "value": "org.example.App",
          "metadata": {}
        },
        "text": "Example app",
        "value": "org.example.App",
        "metadata": {},
        "selected_index": 0
      }
    }
  }
}
```

The context rules are fixed:

- `command.id` and `command.type` identify the host-selected command and its declared operation type.
- `context.parameters` contains the command owner's bound parameters. For an independently mounted Picker command, this is the mounted View's parameter snapshot. For a command projected from the selected feed owner into an aggregate Picker, this is that feed's independent parameter snapshot.
- `context.input` contains the explicitly published launch input descriptor.
- `context.engine` identifies the carrying Engine and exposes its public state projection. For Picker, selection is the normalized `state.item`; no selected item is represented as `null`.
- Feed identity, workflow roots, task generations, cancellation handles, mounted View identity, and other scheduling/provenance data are not sent automatically. Scripts use the public item projection rather than an owner or feed identifier.

Provider `display` remains the presentation input and may support rich display structures. Public selected-item text is normalized plain text. JSON fields are protocol data, and internal runtime state is exposed only through the documented public projection.

### 5. Responses and navigation parameter transport

A successful command response is one complete versioned JSON object:

```json
{
  "version": 1,
  "operation": {
    "type": "navigate",
    "target": "sys:output",
    "query": {"action": "date"}
  }
}
```

Static and script-produced operations converge on the same logical request:

```text
NavigationRequest(target, query, presentation)
```

`query` is already computed data. The host resolves the target, validates its query fields and types, binds defaults, prepares the statically configured Engine, and commits navigation through the existing Router. Invalid parameters or preparation failures retain the source View.

The target receives only its own bound parameters; the host does not implicitly propagate source selection or evaluate query strings. Command handlers construct dynamic query values themselves. `call` uses this same parameter path and additionally records the caller return boundary.

Validated output is necessary but not sufficient for an operation to execute: target availability and mounted-instance validity are still checked by the host at application time.

### 6. Picker items producers

Declared items retain their array shorthand:

```toml
[views.main.engine.config]
items = [
  { display = "Show date", value = "date" },
  { display = "System info", value = "system-info" }
]
```

Dynamic items use the common producer form:

```toml
[views.main.engine.config.items]
producer = "script"

[views.main.engine.config.items.handler]
file = "scripts/items.sh"
```

Request:

```json
{
  "version": 1,
  "entrypoint": "picker-items",
  "context": {
    "parameters": {"initial": "sec"},
    "input": {
      "stdin": {
        "path": "/tmp/tlaunch-input-123",
        "length": 13,
        "is_tty": false
      }
    },
    "engine": {
      "type": "picker",
      "state": {
        "input": "sec",
        "item": null,
        "text": null,
        "value": null,
        "metadata": null,
        "selected_index": 0
      }
    }
  }
}
```

Response:

```json
{
  "version": 1,
  "items": [
    {"display": "second", "value": "1", "metadata": {}}
  ]
}
```

`context.parameters` contains the independent provider parameters, `context.input` contains the explicit launch input, and `context.engine.state.input` is the current Picker request input. Feed identity, workflow root, generation, cancellation, and other scheduling state remain inside Picker. The response replaces the complete items collection after validation. Providers cannot return operations, incremental patches, or other View configuration. Aggregate composition and item provenance remain host-owned. Item producer stdout uses the normal script timeout and stderr budget, with a 64 MiB stdout bound for large candidate sets.

### 7. Capture output producers run after mount

Declared output remains a string:

```toml
[views.output.engine]
type = "capture"

[views.output.engine.config]
output = "fixed text"
```

Dynamic output:

```toml
[views.output.engine]
type = "capture"

[views.output.engine.config]

[views.output.engine.config.output]
producer = "script"

[views.output.engine.config.output.handler]
file = "scripts/output-provider.sh"
```

Request and response:

```json
{
  "version": 1,
  "entrypoint": "capture-output",
  "context": {
    "parameters": {"action": "date"},
    "input": {
      "stdin": {"path": null, "length": 0, "is_tty": true}
    },
    "engine": {"type": "capture", "state": null}
  }
}
```

```json
{
  "version": 1,
  "output": "System date output\n"
}
```

Lifecycle:

```text
Validate target route and query
  -> Router commits and mounts Capture View
  -> Capture starts output provider and displays loading
  -> Validate complete output response
  -> Capture publishes content
```

On provider failure, Capture remains mounted and displays an error. The provider can produce only output content. It cannot change the route, query schema, Engine type, keymap, or Router stack. A provider failure after mount is distinct from a navigation preparation failure. Capture uses the same unified `context` as other producers; its `context.input` is the explicit launch input descriptor.

### 8. Explicit return processors

A call may declare a return processor. Like a command, the processor declares its permitted operation type:

```toml
[views.main.commands.choose]
key = "enter"
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

Declared return processors use the same typed operation payload model, without reading returned data. Script processors can inspect returned data, but their response must still match their declared `type`.

Example request:

```json
{
  "version": 1,
  "entrypoint": "return",
  "context": {
    "parameters": {},
    "input": {
      "stdin": {"path": null, "length": 0, "is_tty": true}
    },
    "engine": {
      "type": "picker",
      "state": {
        "input": "",
        "item": {"text": "Show date", "value": "date", "metadata": {}},
        "text": "Show date",
        "value": "date",
        "metadata": {},
        "selected_index": 0
      }
    }
  },
  "result": {"text": "Show date", "value": "date", "metadata": {}}
}
```

Return processors receive raw JSON in `result`. A selected value may be part of an item object, but a processor can also receive any other JSON value, including an explicit `null`. Missing return values are invalid, and close/cancel decisions produce no result.

The processor returns the same versioned `operation` envelope used by commands. `result` is an explicit JSON request field, not a namespace resolved by the host.

Return semantics:

| Outcome | Host behavior |
| :--- | :--- |
| Successful child return with processor | Restore caller, then dispatch processor |
| Successful child return without processor | Restore caller and discard the unconsumed result |
| Child cancellation or close | Restore caller; do not dispatch the success processor |
| Root View return | Deliver to invocation result handling |
| Explicit returned JSON `null` | Successful return, not cancellation |

Processor execution sequence:

1. Capture the child's result and the recorded call origin.
2. Commit the return transition, restore the recorded caller, and close the child.
3. Start the processor under the caller's task ownership.
4. Validate its complete response and declared operation type.
5. Confirm caller identity, request generation, and eligibility before applying the operation.

Caller owner parameters are captured at call dispatch and retained for this processor request. Do not silently merge them with the caller's latest runtime state. A later explicit operation can request new work with new inputs.

If the return transition fails, do not start the processor. If the processor fails after return, keep the caller mounted and display the error. Do not reopen the child or automatically rerun a possibly side-effecting script.

### 9. Shared protocol and execution rules

All requests use `version: 1`, an `entrypoint`, and a unified `context` containing `parameters`, `input`, and `engine`. Command requests add `command`; return-processor requests add raw `result`. Feed identity, mounted View, task generation, cancellation, and other scheduling data are internal.

- stdin carries one versioned JSON request.
- stdout carries exactly one complete versioned JSON response, allowing surrounding whitespace but no diagnostics, additional JSON documents, or streaming records.
- stderr carries diagnostics.
- A nonzero exit, malformed JSON, unsupported version, wrong operation type, or invalid result schema is failure. No operation is applied from a failed protocol execution.
- Script handlers specify exactly one of `file` or inline `script`. Both workflow package forms in ADR 0001 remain supported.
- Runtime data travels through the request. Producer handlers do not accept a configured argument vector or interpolate values into argv.
- Host execution policy controls a 10-second script timeout, a 1 MiB default stdout bound, a 64 MiB Picker-item bound, a 64 KiB stderr bound, cancellation, and process cleanup.
- Protocol producers run as managed work. Foreground business programs and Embedded PTY programs keep their distinct terminal policies.
- Script code is trusted code running with the user's permissions. The host cannot roll back side effects performed inside a protocol script.

Capture command input at actual dispatch, after readiness checks; capture provider input for each request. Inputs remain immutable for that execution. Match results against `(ViewInstanceId, TaskId, generation)` and entry-point-specific request validity before consuming them. Replacement, closing, or invalidation makes older results ineligible. Being mounted alone does not authorize a late operation to affect the active View. Picker feeds retain independent parameters, workflow roots, generations, cancellation, and stale-result identity internally. A Picker explicitly projects the commands of the currently selected feed owner into its aggregate footer, while dispatch still uses that feed's owner context and current-result provenance.

### 10. Preserve host ownership

Keep one process, one crate, and the three built-in Engines. Router remains the sole navigation-stack owner. Reuse parameter binding and navigation contracts. `execution` owns process mechanics; `task` owns scheduling and cancellation; protocol adapters map validated operations to host decisions.

The static producer boundary does not remove Picker selection, preview, provenance, readiness checks, or internal snapshots. Those remain implementation state and enter scripts only through the declared public protocol.

## Alternatives Considered

| Alternative | Assessment |
| :--- | :--- |
| Preserve field-specific runtime paths and stage conventions | Leaves scope and snapshot dependencies distributed across configuration. |
| Add typed field contracts and more stages | Improves checks but adds more configuration stages. |
| Introduce typed reference nodes and action wiring | Adds configuration complexity without simplifying the underlying context model. |
| Embed a complete configuration language | Adds a language runtime and a larger migration than reusing workflow scripts. |
| General View builder generating Engine configuration | Deferred; the initial model uses restricted data providers and static Engine configuration. |
| Declared/script producers with typed outputs | Selected: explicit computation boundaries, literal configuration, and complete output validation. |

## Consequences

Users can see where runtime computation occurs. Declared operations remain concise, protocol requests can be captured and replayed, and generated navigation follows one parameter-validation path. Script data cannot redefine unrelated View configuration through a provider response.

The trade-offs are additional scripts for small dynamic mappings, process/JSON overhead, and runtime validation of external outputs. Static checking cannot predict arbitrary script output or external failures. User-defined dynamic outer layouts and Embedded argv are outside the initial capability set; View labels remain host-owned.

Existing dynamic workflows require migration to declared values or producer handlers. The release boundary uses the producer schema directly; runtime values must be supplied through the documented request and response fields.

## Implementation Follow-Up and Acceptance Criteria

The producer structure, operation-type matching, ownership, and lifecycle rules above are implemented. The following behavior is now part of the supported producer path:

- Configuration and version-1 responses are strict. Unknown fields, invalid versions, extra JSON documents, malformed operation payloads, producer/payload mismatches, unsupported run modes, invalid fixed targets, and invalid handler shapes fail validation.
- Declared handlers are normalized through the same protocol operation parser as script responses. Command, Picker items, Capture output, and return processor scripts receive JSON on stdin and must emit one JSON response on stdout.
- Command and return-processor scripts are bounded by the standard script execution policy. Picker items use the larger 64 MiB stdout limit; Capture output uses the default 1 MiB limit.
- Capture output scripts start after the Capture View is mounted. Return processors run after the child is closed and the recorded caller is active. A failed provider leaves Capture mounted with an error; a failed processor leaves the restored caller mounted with an error.
- Missing and explicit `null` return values remain distinct. An explicit `null` is a successful return result and is delivered to a post-commit return processor; a close/cancel decision does not dispatch the processor.
- Runtime computation uses the producer paths described above. JSON protocol boundaries are UTF-8 and do not preserve NUL-delimited or non-UTF-8 output.

Remaining work is migration of user configurations plus future decisions for capabilities intentionally outside this initial producer set such as user-defined dynamic outer layouts and Embedded process-configuration producers. Complete schemas and exhaustive optional/default/error behavior remain implementation follow-up only where the runtime has not yet exposed a dedicated producer contract.

The implementation checklist for the initial producer scope is:

1. **Implemented**: strict configuration and version-1 JSON parsing cover all six command operations, Picker items, Capture output, and return processors. Missing and explicit-null return values are distinct.
2. **Implemented**: declared results and script responses use one operation model; unknown fields, producer/payload mismatches, unsupported modes, and invalid fixed targets are rejected.
3. **Implemented**: protocol work uses the existing bounded execution, cancellation, task ownership, and foreground/Embedded I/O boundaries. Picker requests retain mounted-instance and generation checks.
4. **Implemented**: declared and generated navigation use the existing target binding and Router path. Capture provider failures occur after the target has mounted.
5. **Implemented**: command requests select feed-owner parameters and the host assigns item provenance before exposing Picker output.
6. **Implemented**: return, cancellation/close, explicit null, absent processor, processor failure, and stale-result paths are covered by runtime logic and focused tests. Return transitions commit before new processors run.
7. **Implemented**: fixtures and protocol tests cover replayable requests, complete-response validation, literal configuration data, and entry-point-labelled errors.
8. **Implemented**: representative static navigation, selected-item navigation, feeds, Capture output, call/return, foreground run, and inline producer scripts are covered in the test fixtures.
9. **Implemented**: references, tutorials, CLI inspection, and runtime guarantees describe the static configuration and producer protocol. User-defined dynamic outer layouts and Embedded process-configuration producers remain deferred; Picker preview sizing uses its dedicated static fields and View labels are host-owned.

[Producer Protocol](../reference/producer-protocol.md) records the literal boundary and the explicit request/response contract for runtime data. The [Architecture Convergence plan](../explanation/architecture-convergence.md) records the implemented Router, task, execution, configuration ownership, and documentation boundaries.


### Picker preview extension

Every Picker provides an initially collapsed preview with built-in item details containing display and value. Metadata is available to custom providers but is omitted from the built-in renderer. Custom sources are scripts, declared documents, or inherited feed providers. Specialized metadata projection belongs to producer code; the host has no JSON Pointer block source or parallel block renderer. Preview source ownership is independent of document rendering. An aggregate page uses the selected feed's provider by default, falling back to built-in details, and may declare a page-owned override. Dedicated preview sizing fields control the outer split without enabling or expanding the pane. Parameters, relative paths, and workflow styles follow the provider owner; the outer items/preview layout follows the page. This preserves feed ownership without allowing a feed producer to restructure the carrying View.

Preview scripts begin only through the committed mount's task starter. A bounded runtime-owned preview worker isolates slow preview requests from the default serialized item/task worker. Both workers share correlation delivery, managed execution, cancellation, and shutdown ownership. Preview task IDs and generations cannot replace item registrations. Replacing selection, hiding, covering, and closing discard old preview state; late completions cannot publish into a new selection.

Documents share the item display schema and semantic theme slots, with explicit bounds on structure, text, constraints, and image count. Image decode and terminal encoding remain asynchronous and bounded. See [Picker Preview Documents and Producers](../reference/picker-preview.md).
