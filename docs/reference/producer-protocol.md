---
title: "Static Configuration and Producer Protocol"
type: "reference"
tags:
  - configuration
  - producers
  - protocol
  - scripts
description: "Reference for literal TOML, version-1 producer requests and responses, and runtime data boundaries."
---

# Static Configuration and Producer Protocol

`tlaunch` treats TOML as static configuration. Values are deserialized once, validated against their declared schema, and retained as configuration. Runtime-dependent behavior belongs in an explicit declared or script producer.

## Configuration Values

Configuration values are deserialized once, validated against their declared schema, and retained as View and workflow configuration. Strings remain data wherever the surrounding schema accepts strings, including command labels, query values, metadata, declared Picker items, declared Capture output, producer handler tables, and inline producer script bodies.

A producer handler supplies data for its declared entry point. It cannot alter the command, View, query schema, keymap, or Engine configuration around it.

## Producer Context

The host sends one JSON request to a producer and validates one complete version-1 JSON response:

| Entry point | Request data | Response |
| :--- | :--- | :--- |
| Command | `context.command` plus `context.parameters`, `context.input`, and `context.engine` | One operation matching the command's declared `type`, or an `error` feedback object |
| Picker items | `context.parameters`, `context.input`, and `context.engine` | Complete `items` array |
| Picker preview | `context.parameters`, `context.input`, and Picker `context.engine.state.input/item` | `preview` document or null |
| Capture output | `context.parameters`, `context.input`, and `context.engine` | `output` string |
| Form content | `context.parameters`, `context.input`, and `context.engine` | `content` object with an ordered `fields` array |
| Return processor | `context.parameters`, `context.input`, `context.engine`, and `context.result` | One operation matching the processor's declared `type`, or an `error` feedback object |

The public context fields are:

- `context.parameters`: bound parameters for the command owner or provider feed;
- `context.input`: the explicit launch input descriptor;
- `context.engine.type`: the carrying Engine type;
- `context.engine.state`: the Engine's public state projection. Picker exposes the current query as `state.input` and the normalized selected item as `state.item`. Form exposes edited `values`, raw `drafts`, `valid`, `errors`, `dirty`, and `focused`; its launch parameters remain unchanged. See [Form Content and State](form.md);
- `context.command`: `{ id, type }` identifying the command for `command` entry points;
- `context.result`: the raw JSON value returned by the child View for `return` entry points.

When an aggregate Picker dispatches a key through the focused item's `bindings`, `context.parameters` is the aggregate View's parameter snapshot and `context.engine.state.item` stays the normalized selected item. Feed IDs, owner View names, mounted instance identity, task generations, cancellation handles, and scheduling data remain host-owned.

## Command Producer Example

```toml
[views.main.keymap]
enter = "open"

[commands.open]
label = "Open selected item"
type = "navigate"
producer = "script"

[commands.open.handler]
file = "scripts/open.sh"
```

The script reads selection data from `context.engine.state.item` and returns one operation. Values that need computation belong in the producer request and response.

## Successful Command Feedback

A `run` operation may include `"success_message": "Copied to clipboard"`. The host emits the message as `INFO` only after the foreground process completes successfully. This applies to declared handlers and script responses. External clipboard commands must wait for their copy process and propagate failure through a nonzero exit status. The host does not infer success messages from command names or inspect shell commands.

Built-in clipboard effects use the same host success-feedback path with `Copied to clipboard`. External copy workflows retain their own backend, including binary clipboard formats. Commands that exit immediately record the message without holding the UI open.

Informational feedback in the footer or popup bottom border expires after 3 seconds, including while the UI is idle. Input or a change of active View clears it earlier. A new informational message replaces the previous message and restarts the timeout. Errors retain display priority and their existing lifecycle; expiration does not remove recorded logs.

## Error and Feedback Responses

A command or return processor producer may return a structured error response instead of an operation:

```json
{
  "version": 1,
  "error": {
    "message": "Item could not be processed",
    "level": "warning"
  }
}
```

Or a shorthand string message:

```json
{
  "version": 1,
  "error": "Item could not be processed"
}
```

`level` accepts `warning` (default), `info`, or `error`. Warning and info messages display expiring feedback in the active View's footer or popup bottom border and keep the View open and responsive without terminating or transitioning. An error level records a View error. Non-empty error messages are required. A response cannot define both `operation` and `error`.

## Configuration Ownership

Route definitions, query schemas, Engine types, preview pane sizing, preview provider configuration, and keymaps remain host-owned configuration. A `picker-preview` producer may return an internal document layout; it cannot change the automatic outer items/preview split. Custom preview sources are scripts, declared documents, or inherited feed providers; an absent provider falls back to host-rendered item details. Every Picker preview starts collapsed unless `preview_default_open` is enabled, and preview producers run only when the pane is shown. Preview parameters and relative paths belong to the provider’s workflow: the selected feed for inheritance, or the page for an explicit source. See [Picker Preview Documents and Producers](picker-preview.md) for the complete schema and lifecycle. A producer can supply only the data defined by its entry point and response schema.

## Protocol Rules and Limits

Producer stdout must contain exactly one complete JSON object. Surrounding whitespace is allowed, but diagnostics, a second JSON document, malformed JSON, unknown fields, unsupported `version`, a mismatched operation type, a nonzero exit status, or an invalid operation schema is failure. Diagnostics belong on stderr.

Script handlers use exactly one non-empty `file` or inline `script` field. Relative files in directory workflows stay below the workflow root. The host preserves the caller's `$PWD` and provides `$WORKFLOW_DIR` to directory workflow processes.

The default script policy is a 10-second timeout, 1 MiB stdout, and 64 KiB stderr. Picker item producers use the 64 MiB stdout bound. Managed child processes are cancelled and reaped through the shared execution layer. These controls do not sandbox trusted workflow code.

For complete operation fields and Engine schemas, see [workflow.toml Specification](workflow-toml.md). For task-oriented command examples, see [Commands and Producer Scripts](../how-to/commands-and-producers.md).
