---
title: "Static Values and Runtime Data"
type: "reference"
tags:
  - configuration
  - literals
  - producers
  - protocol
description: "Reference for literal configuration values and the explicit producer protocol that replaces embedded expressions."
---

# Static Values and Runtime Data

`tui-launcher` has no configuration expression language. TOML values are deserialized once, validated against their declared schema, and retained as configuration. There is no `{{ namespace.path }}` expansion, runtime interpolation, or compatibility evaluator.

## Literal Boundaries

Strings are literal wherever the surrounding schema accepts strings. This includes:

- command labels, query values, metadata, and generated strings;
- declared Picker item display, value, and metadata fields;
- declared Capture output;
- producer handler tables and inline producer script bodies.

A template-looking string such as `{{ page.input }}` is ordinary text in a string field. Malformed or unknown namespace-looking text is treated the same way. A field with a stricter type still has to satisfy that type: an Embedded `command` must be an argv array, so a string containing template-looking text is invalid configuration rather than a deferred expression.

Literal configuration is not recursively inspected after deserialization. A producer handler cannot alter the command, View, query schema, keymap, or Engine configuration around it.

## Supplying Runtime Data

Runtime-dependent behavior belongs in an explicit producer. The host sends one JSON request to a script and validates one version-1 JSON response:

| Entry point | Request data | Response |
| :--- | :--- | :--- |
| Command | `command` plus `context.parameters`, `context.input`, and `context.engine` | One operation matching the command's declared `type` |
| Picker items | `context.parameters`, `context.input`, and `context.engine` | Complete `items` array |
| Capture output | `context.parameters`, `context.input`, and `context.engine` | `output` string |
| Return processor | `context.parameters`, `context.input`, `context.engine`, and raw `result` | One operation matching the processor's declared `type` |

Use a command producer when a selected item must determine navigation or a process invocation. Use an items producer when a list is computed at request time. Use a return processor when a successful call result must trigger another operation after the caller is restored.

Example command producer:

```toml
[views.main.commands.open]
key = "enter"
label = "Open selected item"
type = "navigate"
producer = "script"

[views.main.commands.open.handler]
file = "scripts/open.sh"
```

The script reads JSON from stdin. Selection data is in the documented `context.engine.state` projection; it is not interpolated into the handler or argv configuration.

## Rejected Forms

The following are not configuration features:

- embedded expressions in command, query, script, or navigation fields;
- dynamic `args`, `payload`, `shell`, `exit`, or return-handler fields from the removed command model;
- dynamic Picker or Capture source objects;
- expression namespaces such as `input`, `view`, `page`, `selection`, `current`, `result`, or `session`.

Unknown fields fail deserialization or entry-point validation. Values that need computation must be moved into a declared handler or a script producer with a documented request and response.

For exact operation schemas and protocol rules, see [workflow.toml Specification](workflow-toml.md). For process, cancellation, and output bounds, see [Runtime Guarantees and Safety Limits](../explanation/runtime-guarantees.md).
