---
title: "config.toml Specification"
type: "reference"
tags:
  - config
  - toml
  - specification
  - session
description: "Authoritative reference for tlaunch root configuration, defaults, themes, and session bindings."
---

# config.toml Specification

The root `config.toml` file configures application-wide settings, default views, global keymaps, and session-level command bindings.

## Schema Overview

```toml
# Main settings
default_view = "workflow:view"
theme = "theme_name"

# Global engine defaults
[defaults.picker.bindings]
exit = ["ctrl+c", "ctrl+d"]
back = ["escape"]
select_previous = ["up"]
select_next = ["down"]
toggle_preview = ["ctrl+p"]

# Session-wide command
[commands.bindings.help]
key = "f1"
label = "Help"
type = "navigate"
producer = "declared"
handler = { target = "help:main" }
```

## Top-Level Fields

### `default_view`
- **Type**: `string` (Format: `<workflow-id>:<view-name>` or `<alias>`)
- **Description**: The default view presented when `tlaunch` is started without explicit arguments.
- **Example**: `default_view = "core:default"`

### `theme`
- **Type**: `string` (Optional)
- **Description**: Name of the theme to load from `themes/<name>.toml` beside the selected configuration file. Omission uses the built-in `terminal` theme. The CLI `--theme` option overrides this setting; `--theme terminal` selects the built-in theme. See the [theme specification](theme-toml.md) for flat scheme colors and field-level style overrides.

## Engine Defaults (`[defaults.<engine>.bindings]`)

Global keybindings for engines can be adjusted at the root level.

### `[defaults.picker.bindings]`
Available configurable actions for the `picker` engine:
- `exit`: Array of key combinations to immediately exit the launcher.
- `back`: Return to the parent view without changing the input.
- `clear_input`: Clear the current input. The default key is `ctrl+u`.
- `select_previous`: Move selection up.
- `select_next`: Move selection down.
- `toggle_preview`: Toggle the initially collapsed preview in any Picker. The default key is `ctrl+p`.
- `preview_scroll_up`, `preview_scroll_down`: Scroll the preview by three rows. No default keys are assigned.

Picker Enter behavior is configured by the View's explicit command bindings. The Picker engine has no implicit primary-selection action.

## Session Command Bindings (`[commands.bindings]`)

Session commands are high-priority actions handled by the host above all individual views and engines.

Session commands are business commands available above individual View commands. They may appear in the command palette when the active View is folded, but they do not define or open the palette.

The command palette is opened by the host with `Ctrl-K` when command folding is active. It is implemented as a built-in Popup Picker View. The host passes the eligible command descriptors to that View through the Popup navigation request; the View returns the selected command reference to the host for execution.

The built-in command-picker View is loaded before user workflow packages. User configuration may extend or override built-in View configuration through the normal workflow merge rules. The built-in View is host-owned and is not a user-declared session command.
