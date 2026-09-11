---
title: "How to Create and Apply Custom Themes"
type: "guide"
tags:
  - themes
  - styling
  - colors
description: "Create a flat color scheme, override component styles, and apply a custom launcher theme."
---

# How to Create and Apply Custom Themes

## Problem

You want the launcher to match your terminal or desktop colors, with specific styles for selected items and workflow metadata.

## Solution

### 1. Create a Theme File

Create `themes/nord.toml` beside your `config.toml` (normally under `~/.config/tlaunch/`):

```toml
# themes/nord.toml
[scheme]
background = "#2E3440"
foreground = "#D8DEE9"
muted = "#4C566A"
accent = "#88C0D0"
border = "#4C566A"
selection = "#434C5E"
on-selection = "#88C0D0"
error = "#BF616A"
on-error = "#D8DEE9"
branch = "#A3BE8C"

[picker.marker]
bold = false

[picker.cursor]
foreground = "scheme:accent"
underline = true

[picker.badge]
foreground = "scheme:background"

[picker.badge.selected]
foreground = "scheme:branch"
bold = false

[chrome.footer_key]
foreground = "scheme:background"

[workflows.git.styles.branch]
foreground = "scheme:branch"

[workflows.git.styles.branch.selected]
italic = true
```

Each scheme entry contains an `ansi:NAME` or `#RRGGBB` color. Add any names you need, such as `branch`, and refer to them as `scheme:branch` in styles. Styles also accept literal colors directly.

You can start with just `[scheme]` and one entry. The launcher merges your file with the complete built-in theme by scheme key and style field, then resolves references. For example, changing `accent` recolors the inherited marker, cursor, and shortcut foregrounds. `surface` controls the input-prefix and shortcut backgrounds; `selection` controls selected-row and selected-badge backgrounds, which default to the terminal background. Omitted fields inherit; `bold = false` turns off inherited bold, and `background = "ansi:reset"` explicitly restores the terminal background.

### 2. Activate and Check the Theme

Set the theme name in `config.toml`:

```toml
theme = "nord"
```

Validate the configuration and try the theme:

```bash
tlaunch --check --theme nord
tlaunch --theme nord
```

Use `tlaunch --theme terminal` to select the built-in theme. Omitting `theme` from the configuration also uses it. Each user theme extends this single baseline; themes cannot load other themes.

### 3. Customize Workflow Slots

Declare a default slot in the workflow's `workflow.toml`:

```toml
[styles.branch]
foreground = "scheme:accent"
bold = true

[styles.branch.selected]
underline = true
```

Use the slot name in structured item or preview displays. The `[workflows.git.styles.branch]` example above overrides that workflow's default foreground while retaining its bold setting. Its selected style also keeps the workflow's underline and adds italic. The selected background defaults to the picker's selection background unless explicitly set.

See the [theme reference](../reference/theme-toml.md) for the built-in scheme, all component slots, accepted colors, and selected-style rules. The [built-in theme](../../src/ui/theme/builtin/terminal.toml) is a complete template.
