---
title: "Picker Details via Companion Views"
type: "reference"
tags:
  - picker
  - companion
  - capture
  - producers
description: "Reference for replacing the removed Picker preview API with a named Capture companion view."
---

# Picker Details via Companion Views

The engine-specific `picker.preview` API has been removed. Picker details are ordinary named Views attached through the universal Companion mechanism.

## Configuration

Declare the companion on the Picker and define a separate Capture View:

```toml
[views.main]
engine = "picker"
companion = "details"

[views.details]
engine = "capture"

[views.details.capture.output]
file = "scripts/details.py"
```

A View declaring `companion` mounts it by default upon entering the View. The primary View's current selection and published state automatically synchronize to the companion's query.

## Toggle commands

Companion visibility is controlled by an ordinary workflow command. There is no built-in Picker toggle action and no default `Ctrl-P` binding:

```toml
[views.main.bindings]
"ctrl+p" = "toggle_details"

[commands.toggle_details]
label = "Toggle details"
companion = "details"
```

The binding is a normal View-layer binding. It can be moved to another key or released with `unbind`; it never receives Host-level special treatment.

## Capture producer response

A Capture script returns the standard Capture response:

```json
{"version":1,"output":{"type":"paragraph","text":"Selected item details"}}
```

The old `preview` response field, `picker-preview` entrypoint, `picker.preview` theme section, `toggle_preview`, `preview_scroll_up`, and `preview_scroll_down` are not compatibility aliases and are rejected.

See [Companion View Combinations](../how-to/companion-views.md), [workflow.toml](workflow-toml.md), and [Producer Protocol](producer-protocol.md) for the complete contracts.
