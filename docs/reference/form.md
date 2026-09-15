---
title: "Form Content and State"
type: "reference"
tags:
  - form
  - producers
  - commands
description: "Native Form Engine content producers, editable field types, public state, and keyboard behavior."
---

# Form Content and State

The `form` Engine renders editable fields with Ratatui. View query parameters remain immutable launch inputs. A content producer interprets those parameters and supplies fields; edited values are published separately under `context.engine.state`. Ordinary commands determine whether to return values, navigate, call another View, or run a process.

## Configuration

`engine.config.content` is required and accepts a producer with exactly `producer` and `handler`. Form rejects Picker items/feeds, other Engine configuration fields, and a View `keymap`. View and session commands retain their normal binding precedence.

```toml
[views.edit.engine]
type = "form"

[views.edit.engine.config.content]
producer = "declared"

[views.edit.engine.config.content.handler]
fields = [
  { name = "name", label = "Project name", type = "string", required = true },
  { name = "count", type = "integer", value = 2 },
  { name = "enabled", type = "boolean", value = false },
  { name = "options", type = "json", value = { tags = [] } },
]
```

A script producer uses the standard script handler:

```toml
[views.edit.engine.config.content]
producer = "script"
handler = { file = "scripts/content.py" }
```

The request is:

```json
{"version":1,"entrypoint":"form-content","context":{"parameters":{"spec":"a:string:dd,b:number:null"},"input":{"stdin":{"path":null,"length":0,"is_tty":true}},"engine":{"type":"form","state":{"values":{},"drafts":{},"valid":false,"errors":{},"dirty":false,"focused":null}}}}
```

The response is:

```json
{"version":1,"content":{"fields":[{"name":"a","type":"string","value":"dd"},{"name":"b","type":"number","value":null}]}}
```

The script runs after activation using the shared managed task and execution layers. A successful response initializes the fields once. Covering and restoring the View preserves its drafts; a covered producer completion may initialize local content. Closing cancels pending work. A failed producer leaves the View mounted with a diagnostic; reopen it to retry. Content is not regenerated on each edit.

`--check` validates declared content and script handler configuration without running scripts. Script responses follow the strict version-1 protocol, with the default 10-second timeout, 1 MiB stdout, and 64 KiB stderr limits. The producer can supply content only; it cannot redefine View query schemas, commands, or Engine configuration.

## Content Fields

Content has exactly one required property, `fields`, an ordered array. Empty arrays are accepted. Unknown properties and duplicate or blank field names are errors.

| Property | Type | Default / meaning |
| :--- | :--- | :--- |
| `name` | string | Required stable key in published values and drafts. |
| `label` | string or null | Uses `name` when absent or null. |
| `type` | string | `string`; also accepts `password`, `integer`, `number`, `boolean`, and `json`. |
| `value` | JSON value | Null when absent; initializes the editor. Non-null values must match the field type. |
| `required` | boolean | False; rejects null and whitespace-only string values when true. |

`string` and `password` fields retain text verbatim (`password` fields mask characters with `*` when rendered). An absent or null initial string becomes an empty string. Other initial values are serialized as JSON; absent or null values initialize an empty editor. Empty non-string editors produce null. Nonempty integer, number, and boolean editors must contain JSON of the corresponding type; `json` accepts any JSON value, including arrays, objects, and null. An incomplete required field is valid content configuration and appears as an editable validation error.

Fields use single-line editors in a vertical layout. The focused field remains visible as focus changes. Pasted content is retained verbatim, including newlines; control characters are displayed as spaces. JSON can therefore be pasted with formatting, although the editor displays it on one line.

## Public State and Commands

A command receives the original query in `context.parameters` and the following current state in `context.engine.state`:

| Property | Meaning |
| :--- | :--- |
| `values` | Object of parsed field values; invalid fields contain null. |
| `drafts` | Object of exact editor strings, including incomplete input. |
| `valid` | True only after content loads and every field passes validation. |
| `errors` | Object mapping invalid field names to messages. |
| `dirty` | Whether any editor string differs from its initial string. |
| `focused` | Focused field name, or null when there are no fields. |

There is no implicit submit action. Enter does nothing unless a command binds it. Commands remain available while loading or invalid, so navigation and cancellation can still work. A command that requires valid values must check `state.valid` before producing its operation. It must not infer validity from null values, since optional fields may legitimately be null.

```python
state = request["context"]["engine"]["state"]
if not state["valid"]:
    raise SystemExit("Please correct the form fields")
response = {"version": 1, "operation": {"type": "return", "value": state["values"]}}
```

`edit-input` is not supported by Form: its query is launch input, and its fields are independent drafts. Submit, call/return, and navigation use the existing [command protocol](workflow-toml.md#commands).

## Keyboard Behavior

Registered View, Engine, and Host commands take precedence over these editing keys.

| Key | Behavior |
| :--- | :--- |
| Tab / Down | Next field, wrapping at the end. |
| Shift+Tab / Up | Previous field, wrapping at the start. |
| Left / Right | Move within the field. |
| Home / Ctrl+A | Move to the start. |
| End / Ctrl+E | Move to the end. |
| Backspace / Delete | Delete before / after the cursor. |
| Ctrl+U | Clear the field. |
| Ctrl+W | Delete the previous word. |
| Space in a boolean field | Toggle true/false. |
| Esc | Close without a return value. |
| Ctrl+C / Ctrl+D | Exit. |

Theme slots are `form.label`, `form.input`, `form.focused`, and `form.error`; see the [theme reference](theme-toml.md).
