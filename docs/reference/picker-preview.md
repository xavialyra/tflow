---
title: "Picker Preview Documents and Producers"
type: "reference"
tags:
  - picker
  - preview
  - producers
  - rendering
description: "Configuration, ownership, version-1 script protocol, document schema, scrolling, and resource limits for Picker previews."
---

# Picker Preview Documents and Producers

Every Picker has a preview pane, initially collapsed. `Ctrl+P` toggles it by default, and `preview_default_open = true` can open it at activation. The outer pane is sized automatically from `preview_ratio` and `preview_min_width`; providers and documents cannot define that split. While collapsed, the host does not prepare preview content, execute preview scripts, or load preview images.

A Picker preview has two independent parts: a data source and a host-rendered document. Without a custom provider, the host displays built-in item details. A document may contain its own internal layouts.

## Source Configuration

The following source forms are mutually exclusive. Unknown and incompatible fields fail validation.

| Form | Fields under `engine.config.preview` |
| :--- | :--- |
| Script | `producer = "script"`, `handler = { file = "scripts/preview.py" }` or `handler = { script = "..." }` |
| Declared document | `producer = "declared"`, `document = "text"` or a document table |
| Inherited feed provider | `inherit = true` |

Script handlers use the shared file/inline-script convention. Relative script files must stay within the provider workflow directory. `--check` validates the handler and path without running the producer.

Omitting `preview` is equivalent to `inherit = true`. On an aggregate page, the host uses the selected item's source feed provider when one is configured. If the feed has no provider or itself specifies inheritance, the host displays built-in item details. On a non-aggregate page, omission or explicit inheritance displays built-in item details.

An explicit page script or declared document overrides feed providers and built-in details. Preview provider ownership never changes the page's automatic outer pane sizing.

Built-in details display the item's plain-text display as a title and its value when present. Metadata is not shown by the built-in renderer; workflow preview providers can display it when needed. Built-in text uses the standard paragraph sanitizer and is truncated at a UTF-8 boundary to fit the 256 KiB text limit, with a truncation notice. No selection displays the empty state.

For an inherited provider, bound parameters, script files, image paths, and custom style slots belong to the source feed's workflow. For an explicit page script or declared document, they belong to the page workflow. Launch input is shared with the mounted Picker. Relative image paths resolve from the provider's workflow directory; absolute paths and `~/` paths are also supported. The process retains the caller's working directory and receives `TFLOW_WORKFLOW_DIR` for directory workflows.

## Outer Pane Sizing

```toml
[views.main.engine.config]
preview_ratio = 0.35
preview_min_width = 24
preview_default_open = false
```

`preview_ratio` is a number from 0 through 1 and controls the preview share of the horizontal body. `preview_min_width` is an unsigned 16-bit minimum width for the preview pane. `preview_default_open` is a boolean and defaults to false. The host always places the items pane first and uses a one-column gap. The query and divider rows are deducted before sizing the body. If the body is empty or the minimum width cannot fit, the preview is hidden and its pending work is cancelled.

## Script Protocol

The host starts a script about 80 ms after the selected item stabilizes, through the post-commit task starter. It writes one version-1 request to stdin:

```json
{
  "version": 1,
  "entrypoint": "picker-preview",
  "context": {
    "parameters": {"search": "main"},
    "input": {"stdin": {"path": null, "length": 0, "is_tty": true}},
    "engine": {
      "type": "picker",
      "state": {
        "input": "main",
        "item": {"text": "Main branch", "value": "main", "metadata": {"summary": "Recent commits"}}
      }
    }
  }
}
```

The response contains exactly `version` and `preview`:

```json
{"version":1,"preview":{"type":"paragraph","spans":[{"text":"Ready. ","slot":"success"},"Recent commits"]}}
```

`preview` may be `null`, a string, or a document object. Null means an empty preview. Built-in details are a fallback for an absent provider only; a null result stays empty and a failed provider displays its error. The response cannot contain commands, operations, actions, outer pane definitions, or View configuration. Surrounding whitespace is allowed. Extra stdout text, a second JSON document, unknown fields, missing fields, unsupported versions, invalid documents, and nonzero process exits fail the preview and render an error in its pane.

## Document Schema

A string is shorthand for a wrapped paragraph. Object nodes use the `type` discriminator. `display`, `paragraph`, `image`, and `layout` accept optional `border` (boolean, default false) and `title` (string).

| Type | Required fields | Optional fields |
| :--- | :--- | :--- |
| `display` | `display`: existing item-display input | `border`, `title` |
| `paragraph` | Exactly one of `text` (string) or `spans` (array) | `wrap` (default true), `slot` (optional, used for `text`), `border`, `title` |
| `image` | `path`: nonempty image path | `border`, `title` |
| `separator` | None | None |
| `layout` | `direction`: `horizontal` or `vertical`; nonempty `children`: documents | `constraints`, `border`, `title` |

A paragraph span is a string or `{ "text": "...", "slot": "accent" }`. Document strings, paragraph `text` without a slot, and plain string spans use `picker.preview.text`, including the paragraph’s trailing background. An explicit paragraph or span slot overrides that text style. Object spans default to `primary` when their slot is omitted; display nodes retain item-display defaults. Paragraph newlines are preserved and wrapping keeps spaces. Tabs become one space, following the shared terminal text sanitizer. Other control characters and ANSI CSI/OSC escape sequences are stripped before text is rendered; literal escape codes do not apply styles. Display spans and titles use the same sanitizer and remain single-line text. Shared slots include `primary`, `secondary`, `muted`, `accent`, `badge`, `success`, `warning`, and `error`. Workflow custom slots use the provider owner's theme styles. Preview data cannot supply raw colors or terminal escape commands.

A display uses the same progressive schema as item display: a string, `{ "cells": [...] }`, or `{ "rows": [...] }`. Rows contain `cells` and optional `constraints`. A cell is `{ "text": "...", "slot": "...", "align": "right" }` or `{ "spans": [...], "align": "left" }`. Alignment is `left`, `center`, or `right`. Spans use the paragraph span shape. Display rows occupy one terminal row each.

Constraints use the item-display JSON encoding: `{"Length": 3}`, `{"Percentage": 50}`, `{"Ratio": [1, 2]}`, `{"Min": 2}`, `{"Max": 8}`, or `{"Fill": 1}`. A nonempty constraint array must match the number of children/cells. Missing constraints distribute available space equally. Fill weights and ratio denominators must be positive; percentages cannot exceed 100. Length, percentage, min, max, and fill values are unsigned 16-bit integers; ratio values are unsigned 32-bit integers.

```json
{
  "version": 1,
  "preview": {
    "type": "layout",
    "direction": "vertical",
    "constraints": [{"Length": 1}, {"Length": 1}, {"Fill": 1}],
    "children": [
      {"type": "display", "display": {"cells": [{"text": "Main", "slot": "accent"}]}},
      {"type": "separator"},
      {"type": "layout", "direction": "horizontal", "children": [
        {"type": "image", "path": "art.png"},
        {"type": "paragraph", "text": "Long text wraps here.", "border": true, "title": "Details"}
      ]}
    ]
  }
}
```

## Lifecycle, Scrolling, and Limits

The host displays loading, error, and empty states. Changes to metadata, input, parameter values, or provider provenance invalidate a preview even when the item's value is unchanged. The host cancels replaced work and rejects stale completions. Hiding the preview, clearing selection, covering, or closing the View cancels preview work and clears loaded content. Showing or restoring the View reloads the current selection.

A script document is also cached by preview provider (the resolved `owner`) for the session, bounded to 32 providers and shared by every Picker the session's factory mounts. When navigation or `replace` mounts the same View again, the new instance paints that provider's cached document immediately and refreshes it in the background instead of flashing `Loading preview…`; image decoding still waits for post-commit host authority. The key deliberately ignores the request identity, because a self-navigation changes the parameters (for example a Picker's selected set) that the identity would otherwise include. Declared and inherited documents install synchronously and do not use the cache.

`toggle_preview`, `preview_scroll_up`, and `preview_scroll_down` are available in every Picker, including Views without preview configuration. `toggle_preview` is bound to `Ctrl+P` by default and can be overridden by the existing keymap mechanism. Scroll actions move three rows; no scrolling keys are assigned by default. The stored scroll offset is clamped to the document’s last screen before and after each scroll action and whenever the available body size changes. A new selection resets scrolling. The host computes content height for wrapping and nested layout; explicit fixed constraints may clip a child's content. The whole document scrolls as one virtual canvas. Borders and titles appear only where their original edges remain visible; clipping does not create a border at the viewport bottom. Images fit the visible fragment of their original inner rectangle as the document scrolls.

Preview scripts have an isolated runtime-owned worker and a queue limited to one pending job across mounts. A submission replaces pending work in the same mount lane. If another mount owns the pending slot, the incoming task fails with `preview task queue is full`; the other mount’s work is preserved. Tasks have independent correlation, mount cancellation, and joined shutdown. Both workers are cancelled before shutdown joins either worker. The default task worker remains serialized; a slow preview does not block item loading. Image decoding and terminal-protocol encoding retain their asynchronous bounded pools.

| Resource | Limit |
| :--- | :--- |
| Script wall time | 10 seconds |
| Script stdout / stderr | 1 MiB / 64 KiB |
| Document nodes / nesting | 128 nodes / 16 nested edges |
| JSON structural nodes / nesting | 4096 nodes / 48 nested edges |
| Document string bytes in total | 256 KiB |
| Images in one document | 4 |
| Virtual document height / scroll offset | 16,384 rows |
| Image dimensions | 8192 × 8192 |
| Image encoded bytes / decode allocation | 64 MiB each |
| Concurrent decodes / protocol encodes | 2 each |

The shared managed execution layer enforces process timeout, output bounds, cancellation, and child reaping. These controls do not sandbox trusted workflow scripts.

See [Picker Views](../how-to/picker-views.md) for setup and a runnable mixed-document fixture.
