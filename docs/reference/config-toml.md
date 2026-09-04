---
title: "config.toml Specification"
type: "reference"
tags:
  - config
  - toml
  - specification
  - session
description: "Authoritative reference for tui-launcher root configuration, defaults, themes, and session bindings."
---

# config.toml Specification

The root `config.toml` file configures application-wide settings, default views, global keymaps, and session-level command bindings.

## Schema Overview

```toml
# Main settings
default_view = "plugin:view"
theme = "theme_name"

# Global engine defaults
[defaults.picker.bindings]
exit = ["ctrl+c", "ctrl+d"]
back = ["escape"]
select_previous = ["up"]
select_next = ["down"]
activate = ["enter"]

# Session-wide commands
[commands.bindings.commands]
key = "ctrl+k"
```

## Top-Level Fields

### `default_view`
- **Type**: `string` (Format: `<plugin-id>:<view-name>` or `<alias>`)
- **Description**: The default view presented when `tui-launcher` is started without explicit arguments.
- **Example**: `default_view = "core:default"`

### `theme`
- **Type**: `string` (Optional)
- **Description**: Name of the theme to load from `$XDG_CONFIG_HOME/tui-launcher/themes/<name>.toml`. Defaults to `terminal`.

## Engine Defaults (`[defaults.<engine>.bindings]`)

Global keybindings for engines can be adjusted at the root level.

### `[defaults.picker.bindings]`
Available configurable actions for the `picker` engine:
- `exit`: Array of key combinations to immediately exit the launcher.
- `back`: Clear input or return to the parent view.
- `select_previous`: Move selection up.
- `select_next`: Move selection down.
- `activate`: Trigger primary selection action.

## Session Command Bindings (`[commands.bindings]`)

Session commands are high-priority actions handled by the host above all individual views and engines.

### `[commands.bindings.commands]`
- **`key`**: Key chord that triggers the global command palette (opens `selectors:commands`).
- **Default**: `"ctrl+k"`
- **Behavior**: This action cannot be replaced; only the triggering key binding can be customized.
