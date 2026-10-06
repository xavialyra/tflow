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

# Host layer: session-wide shortcuts targeting command FQIDs.
[host.bindings]
"ctrl+k" = "__commands.palette"
"ctrl+g" = "__parameters.edit"

# Engine layer: key-centric defaults, flattened one level (no `defaults.`).
[picker]
left_prefix = "$route"
left_prefix_backspace = "root"

[picker.bindings]
"ctrl+c" = "exit"
"ctrl+d" = "exit"
"escape" = "back"
"ctrl+u" = "clear_input"
"up" = "select_previous"
"down" = "select_next"

[capture.bindings]
"enter" = "copy"
"escape" = "back"

[embedded.bindings]
"escape" = "cancel"

[form.bindings]
"tab" = "focus_next"
"down" = "focus_next"
"backtab" = "focus_prev"
"up" = "focus_prev"
"escape" = "cancel"
"ctrl+c" = "exit"
"ctrl+d" = "exit"

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
| `image_protocol` | string | `"auto"` | Protocol used for rendering images in capture documents. Options: `"auto"`, `"halfblocks"`, `"kitty"`, `"sixel"`, `"iterm2"`. In `"auto"` mode, terminal capability replies select Kitty before Sixel; known iTerm2-compatible terminals use iTerm2 when not inside tmux. In tmux, Kitty requires `allow-passthrough on`; when passthrough is unavailable, automatic selection avoids Kitty/Sixel and falls back to Halfblocks. Unknown or non-responding terminals also use Halfblocks. Explicit values bypass detection and require a compatible terminal path. |
| `log_file` | string (path) | Unset | Destination path for host debug and execution logs. |
| `chrome_commands_show` | array of strings | `["enter", "ctrl+k"]` | Default footer key hints applied to every View that does not override `chrome_commands_show`. Keys with no bound command are skipped; `[]` hides all hints. |
| `host` | table | `{}` | Host-layer global shortcuts. See [Host Bindings](#host-layer-hostbindings). |
| `styles` | table | `{}` | Global semantic style overrides for mounted workflow slots. |

Engine defaults (`[picker]`, `[capture]`, `[embedded]`, `[form]`) are declared directly at the settings root; the redundant `defaults.` prefix does not exist.

---

## Host Scope (`[host.bindings]`)

Host-layer bindings are session-wide shortcuts that target workflow commands by fully qualified ID (`workflow.command`) or host actions using `@host:<action>`:

```toml
[host.bindings]
"ctrl+k" = "__commands.palette"
"ctrl+g" = "__parameters.edit"
"ctrl+l" = "@host:open_companion"
```

The same table is accepted in a Suite Manifest. The two are merged in increasing precedence, so `settings.toml` wins for a key both declare, and a `false` there also overrides the suite's binding.

`false` means the host layer declares **no** binding for that key: nothing claims the key, so it falls through to raw input, and a command whose only binding was that shortcut is no longer published by this layer or invokable by id. That is a different mechanism from a View's [`unbind`](workflow-toml.md), which releases a binding while the command keeps its identity. Any other value — `true`, a number, an array, or even an empty string — is rejected at load time.

---

## Engine Defaults (`[<engine>]`)

Global engine defaults define baseline bindings inherited by all matching views unless overridden per-view. Binding tables are key-centric: the physical key is the TOML map key and the value is the engine action.

Engine defaults do not provide a Companion toggle key. If a Picker should toggle its configured companion, declare a normal workflow command and bind it in `[views.<name>.bindings]`; see [Companion View Combinations](workflow-toml.md#companion-views).

### 1. Picker Engine (`[picker]`)

Configures presentation and navigation behaviors for Picker views.

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `left_prefix` | string | Unset | Left-side marker on the query input line for non-root Pickers. `"$route"` displays the current view's route/alias; any other string is rendered literally. Best used with East Asian Wide glyphs (e.g. `〈`). Purely presentational. |
| `left_prefix_backspace` | string | Unset | Action when pressing Backspace on an empty input line while a left prefix is rendered. `"parent"` returns to the parent view (like Escape); `"root"` returns to the root view in a single step; unset leaves Backspace inert. |
| `bindings` | table | See below | Key-centric bindings table for picker navigation and actions. |

Picker has no preview-specific engine actions. Selection, editing, and ordinary View commands are configured through the Picker engine bindings and the View/workflow command tables.

| Built-in Key | Action | Description |
| :--- | :--- | :--- |
| `"ctrl+c"`, `"ctrl+d"` | `exit` | Immediately aborts and terminates `tflow`. |
| `"escape"` | `back` | Returns to the previous/parent view without modifying input. |
| `"ctrl+u"` | `clear_input` | Clears the active query buffer. |
| `"ctrl+w"` | `delete_word` | Deletes the word before the cursor. |
| `"backspace"` | `delete_backward` | Deletes the character before the cursor. |
| `"up"` | `select_previous` | Moves the selection up one item. |
| `"down"` | `select_next` | Moves the selection down one item. |

*Note: Enter behavior is configured explicitly by each View's command bindings. The Picker engine intentionally provides no implicit primary selection action.*

---

### 2. Capture Engine (`[capture]`)

Configures presentation and keybindings for Capture views.

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `bindings` | table | See below | Key-centric bindings table for Capture views. |

#### Capture Default Bindings (`[capture.bindings]`)

| Built-in Key | Action | Description |
| :--- | :--- | :--- |
| `"escape"` | `back` | Returns to the previous view or closes the Capture view. |
| `"up"`, `"k"` | `scroll_up` | Scrolls the capture content up by one line. |
| `"down"`, `"j"` | `scroll_down` | Scrolls the capture content down by one line. |
| `"ctrl+u"` | `page_up` | Scrolls the capture content up by half a page. |
| `"ctrl+d"` | `page_down` | Scrolls the capture content down by half a page. |

*Note: `copy` (`capture.copy`) is not bound by default. In views where copying is desired, bind it explicitly in `[capture.bindings]` (e.g. `"ctrl+y" = "copy"`) or in a specific View's `[views.<name>.bindings]` (e.g. `"enter" = "@engine:capture.copy"`).*

---

### 3. Embedded Engine (`[embedded]`)

Configures presentation and keybindings for Embedded PTY views.

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `bindings` | table | See below | Key-centric bindings table for Embedded views. |

#### Embedded Default Bindings (`[embedded.bindings]`)

| Built-in Key | Action | Description |
| :--- | :--- | :--- |
| `"escape"` | `cancel` | Cancels the embedded process and unwinds the view stack. |

*Note: To allow Escape to pass directly into the child process (e.g. for Vim), release the inherited binding in that View: `[views.<name>.unbind] keys = ["escape"]`.*

---

### 4. Form Engine (`[form]`)

Configures presentation and keybindings for Form views.

| Field | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `bindings` | table | See below | Key-centric bindings table for Form views. |

#### Form Default Bindings (`[form.bindings]`)

| Built-in Key | Action | Description |
| :--- | :--- | :--- |
| `"tab"`, `"down"` | `focus_next` | Moves focus to the next form field. |
| `"backtab"`, `"up"` | `focus_prev` | Moves focus to the previous form field. |
| `"escape"` | `cancel` | Closes the form without submitting. |
| `"ctrl+c"`, `"ctrl+d"` | `exit` | Exits `tflow`. |

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
| `defaults` | Removed; declare engine tables (`[picker]`, `[capture]`, `[embedded]`, `[form]`) directly at the settings root. |
