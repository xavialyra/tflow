---
title: "settings.toml Specification"
type: "reference"
tags:
  - config
  - toml
  - specification
  - session
  - settings
description: "Authoritative reference for tflow passive host settings, global engine defaults, image protocols, and keybindings."
---

# settings.toml Specification

The passive host configuration file lives at `$XDG_CONFIG_HOME/tflow/settings.toml`. It defines the ambient execution environment, global display protocols, default keybindings, and engine-level behavior.

Both standalone workflows (`-w`) and suite orchestration sessions (`-s`) inherit `settings.toml`. Orchestration concerns (such as mounting workflows, defining aliases, and session entrypoints) are strictly prohibited here and belong exclusively in a [Suite Manifest](suite-toml.md).

---

## File Resolution Order

The host discovers `settings.toml` using the following precedence:

1. Path specified by `--settings <PATH>` on the CLI.
2. Path provided via the `TFLOW_SETTINGS` environment variable.
3. `$XDG_CONFIG_HOME/tflow/settings.toml` (if it exists).
4. `~/.config/tflow/settings.toml` (standard fallback).
5. If no settings file is located, built-in defaults apply.

---

## Schema Overview

```toml
theme = "nord"
image_protocol = "kitty"
log_file = "/tmp/tflow.log"

[defaults.picker]
left_prefix = "$route"
left_prefix_backspace = "root"

[defaults.picker.bindings]
exit = ["ctrl+c", "ctrl+d"]
back = ["escape"]
clear_input = ["ctrl+u"]
select_previous = ["up", "ctrl+k"]
select_next = ["down", "ctrl+j"]
toggle_preview = ["ctrl+p"]
preview_scroll_up = ["ctrl+u"]
preview_scroll_down = ["ctrl+d"]

[defaults.capture.bindings]
exit = ["ctrl+c", "escape"]

[styles.git.staged]
foreground = "scheme:accent"
bold = false
underline = true
```

---

## Root Configuration Fields

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `theme` | string | Unset (terminal default) | Theme name loaded from `themes/<name>.toml` beside the settings file. Overridden by CLI `--theme`. |
| `image_protocol` | string | `"halfblocks"` | Protocol used for rendering images in previews. Options: `"halfblocks"`, `"kitty"`, `"sixel"`, `"iterm2"`. |
| `log_file` | string (path) | Unset | Destination path for host debug and execution logs. |
| `defaults` | table | `{}` | Global defaults applied across all views for specific engines. |
| `styles` | table | `{}` | Global semantic style overrides for mounted workflow slots. |

---

## Engine Defaults (`[defaults.<engine>]`)

Global engine defaults define baseline behaviors inherited by all matching views unless explicitly overridden per-view.

### 1. Picker Engine (`[defaults.picker]`)

Configures presentation and navigation behaviors for Picker views.

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `left_prefix` | string | Unset | Left-side marker on the query input line for non-root Pickers. `"$route"` displays the current view's route/alias; any other string is rendered literally. Best used with East Asian Wide glyphs (e.g. `〈`). Purely presentational. |
| `left_prefix_backspace` | string | Unset | Action when pressing Backspace on an empty input line while a left prefix is rendered. `"parent"` returns to the parent view (like Escape); `"root"` returns to the root view in a single step; unset leaves Backspace inert. |
| `bindings` | table | See below | Keybindings table for picker navigation and actions. |

#### Picker Default Bindings (`[defaults.picker.bindings]`)

Configures physical key mappings to standard Picker actions:

| Action | Built-in Default | Description |
| :--- | :--- | :--- |
| `exit` | `["ctrl+c", "ctrl+d"]` | Immediately aborts and terminates `tflow`. |
| `back` | `["escape"]` | Returns to the previous/parent view without modifying input. |
| `clear_input` | `["ctrl+u"]` | Clears the active query buffer. |
| `select_previous`| `["up"]` | Moves the cursor selection up one item. |
| `select_next` | `["down"]` | Moves the cursor selection down one item. |
| `toggle_preview`| `["ctrl+p"]` | Toggles visibility of the item preview pane. |
| `preview_scroll_up` | Unset | Scrolls the preview document up by three rows. |
| `preview_scroll_down`| Unset | Scrolls the preview document down by three rows. |

*Note: Enter behavior is configured explicitly by each View's command keymap. The Picker engine intentionally provides no implicit primary selection action.*

---

### 2. Capture Engine (`[defaults.capture]`)

Configures presentation and keybindings for Capture views.

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `bindings` | table | See below | Keybindings table for Capture views. |

#### Capture Default Bindings (`[defaults.capture.bindings]`)

| Action | Built-in Default | Description |
| :--- | :--- | :--- |
| `exit` | `["ctrl+c", "escape"]` | Exits or closes the Capture view. |

---

## Semantic Style Slot Overrides (`[styles.<member_id>.<slot>]`)

Global settings can define top-priority overrides for semantic style slots declared by workflows:

```toml
[styles.git.staged]
foreground = "scheme:accent"
bold = false
underline = true
```

### Style Cascade Precedence
When resolving color and text decorations for any item or element, `tflow` applies styles in the following order (highest priority wins):
1. **`settings.toml` Overrides** (`[styles.<workflow>.<slot>]`)
2. **Suite Manifest Overrides** (`<suite>.toml`: `[styles.<workflow>.<slot>]`)
3. **Active Theme File** (`themes/<name>.toml`: `[workflows.<workflow>.styles.<slot>]`)
4. **Workflow Defaults** (`workflow.toml`: `[styles.<slot>]`)

---

## Prohibited Fields (Purity Invariant)

In compliance with [ADR 0005](../adr/0005-manifest-driven-suites-and-self-contained-workflows.md), `settings.toml` is strictly reserved for passive host configuration. The presence of any of the following fields causes immediate validation failure:

| Forbidden Key | Reason & Migration |
| :--- | :--- |
| `suite` | Session orchestration belongs in a Suite Manifest (`default.toml` or `<suite>.toml`). |
| `workflow` | Workflows must be defined in standalone files (`workflow.toml`). |
| `workflows` | Workflow mounts belong in a Suite Manifest under `[workflows]`. |
| `views` | View definitions belong in atomic workflows under `[views.<name>]`. |
| `default_view` | Removed; declare `[suite].entrypoint` in your suite manifest instead. |
| `disabled_workflows`| Removed; suites mount workflows explicitly; omit unneeded workflows from `[workflows]`. |
| `[commands]` | Commands belong to atomic workflows; settings reject command definitions. |
