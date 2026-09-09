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

`tui-launcher` treats TOML as static configuration. Values are deserialized once, validated against their declared schema, and retained as configuration. Runtime-dependent behavior belongs in an explicit declared or script producer.

## Static Configuration Boundary

Strings are literal wherever the surrounding schema accepts strings. This includes command labels, query values, metadata, declared Picker items, declared Capture output, producer handler tables, and inline producer script bodies.

A template-looking string such as `{{ page.input }}` is ordinary text in a string field. There is no configuration expression language, runtime interpolation, or compatibility evaluator. A producer handler cannot alter the command, View, query schema, keymap, or Engine configuration around it.

## Producer Context

The host sends one JSON request to a producer and validates one complete version-1 JSON response:

| Entry point | Request data | Response |
| :--- | :--- | :--- |
| Command | `command` plus `context.parameters`, `context.input`, and `context.engine` | One operation matching the command's declared `type` |
| Picker items | `context.parameters`, `context.input`, and `context.engine` | Complete `items` array |
| Capture output | `context.parameters`, `context.input`, and `context.engine` | `output` string |
| Return processor | `context.parameters`, `context.input`, `context.engine`, and raw `result` | One operation matching the processor's declared `type` |

The public context fields are:

- `context.parameters`: bound parameters for the command owner or provider feed;
- `context.input`: the explicit launch input descriptor;
- `context.engine.type`: the carrying Engine type;
- `context.engine.state`: the Engine's public state projection. Picker exposes the current query as `state.input` and the normalized selected item as `state.item`.

When an aggregate Picker projects a command from the selected feed owner, `context.parameters` is that feed's independent parameter snapshot. A command declared by the aggregate Picker View receives the aggregate View's parameters. Feed IDs, owner View names, mounted instance identity, task generations, cancellation handles, and scheduling data remain host-owned.

## Command Producer Example

```toml
[views.main.commands.open]
key = "enter"
label = "Open selected item"
type = "navigate"
producer = "script"

[views.main.commands.open.handler]
file = "scripts/open.sh"
```

The script reads selection data from `context.engine.state.item` and returns one operation. Runtime values are not interpolated into TOML, handler fields, or argv configuration.

## Unsupported Dynamic Configuration

The following are not configuration features:

- embedded expressions in command, query, script, or navigation fields;
- dynamic command arguments, payloads, shells, or return-handler fields from the removed command model;
- dynamic Picker or Capture source objects;
- ambient namespaces such as `input`, `view`, `page`, `selection`, `current`, `result`, or `session`.

Values that need computation must move into a declared handler or a script producer with a documented request and response.

## Protocol Rules and Limits

Producer stdout must contain exactly one complete JSON object. Surrounding whitespace is allowed, but diagnostics, a second JSON document, malformed JSON, unknown fields, unsupported `version`, a mismatched operation type, a nonzero exit status, or an invalid operation schema is failure. Diagnostics belong on stderr.

Script handlers use exactly one non-empty `file` or inline `script` field. Relative files in directory workflows stay below the workflow root. The host preserves the caller's `$PWD` and provides `$WORKFLOW_DIR` to directory workflow processes.

The default script policy is a 10-second timeout, 1 MiB stdout, and 64 KiB stderr. Picker item producers use the 64 MiB stdout bound. Managed child processes are cancelled and reaped through the shared execution layer. These controls do not sandbox trusted workflow code.

For complete operation fields and Engine schemas, see [workflow.toml Specification](workflow-toml.md). For task-oriented command examples, see [Commands and Producer Scripts](../how-to/commands-and-producers.md).
