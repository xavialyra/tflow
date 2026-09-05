---
title: "How to Create and Apply Custom Themes"
type: "guide"
tags:
  - themes
  - styling
  - colors
  - palette
description: "How to configure color palettes, semantic schemes, and structured component styles for custom launcher aesthetics."
---

# How to Create and Apply Custom Themes

`tui-launcher` provides a declarative styling system composed of **Palettes**, **Semantic Schemes**, and **Component Styles**.

## Problem

You want to customize the look and feel of the launcher to match your terminal emulator or desktop color scheme.

## Solution

### 1. Create a Theme File

Themes are placed in `$XDG_CONFIG_HOME/tui-launcher/themes/<name>.toml`. For example, create `~/.config/tui-launcher/themes/nord.toml`:

```toml
# themes/nord.toml

# 1. Custom Color Palette (Hex RGB or basic ANSI)
[palette]
nord0 = "#2E3440"
nord1 = "#3B4252"
nord2 = "#434C5E"
nord3 = "#4C566A"
nord4 = "#D8DEE9"
nord8 = "#88C0D0"
nord11 = "#BF616A"
nord14 = "#A3BE8C"

# 2. Semantic Color Scheme Roles
[scheme]
primary = "palette:nord8"
on-primary = "palette:nord0"
primary-container = "palette:nord2"
on-primary-container = "palette:nord8"
surface = "palette:nord0"
surface-container = "palette:nord1"
on-surface = "palette:nord4"
on-surface-variant = "palette:nord3"
outline = "palette:nord3"
error = "palette:nord11"
on-error = "palette:nord4"

# 3. Picker Component Styles (Query Editor, Candidate List, and Preview)
[picker.text]
foreground = "scheme:on-surface"
background = "scheme:surface"

[picker.muted]
foreground = "scheme:on-surface-variant"

[picker.input_prefix]
foreground = "scheme:primary"
bold = true

[picker.selected]
foreground = "scheme:on-primary-container"
background = "scheme:primary-container"
bold = true

[picker.selected_muted]
foreground = "scheme:on-surface-variant"
background = "scheme:primary-container"

[picker.marker]
foreground = "scheme:primary"
bold = true

[picker.scrollbar]
foreground = "scheme:primary"
bold = true

[picker.preview.text]
foreground = "scheme:on-surface"
background = "scheme:surface"

[picker.preview.border]
foreground = "scheme:outline"

[picker.preview.error]
foreground = "scheme:on-error"
background = "scheme:error"

# 4. Chrome / Host Styles (Framing, Popups, and Footer)
[chrome.text]
foreground = "scheme:on-surface"
background = "scheme:surface"

[chrome.muted_text]
foreground = "scheme:on-surface-variant"
background = "scheme:surface"

[chrome.divider]
foreground = "scheme:outline"

[chrome.border]
foreground = "scheme:outline"

[chrome.footer]
foreground = "scheme:on-surface-variant"
background = "scheme:surface-container"

[chrome.footer_title]
foreground = "scheme:primary"
background = "scheme:surface-container"
bold = true

[chrome.footer_status]
foreground = "scheme:on-surface-variant"
background = "scheme:surface-container"

[chrome.footer_key]
foreground = "scheme:on-primary-container"
background = "scheme:primary-container"
bold = true

[chrome.error]
foreground = "scheme:on-error"
background = "scheme:error"
bold = true
```

### 2. Activate the Theme in `config.toml`

In your `~/.config/tui-launcher/config.toml`, set the `theme` field:

```toml
theme = "nord"
```

You can also test a theme directly from the command line:

```bash
tui-launcher --theme nord
```

### 3. Component Slot Reference

| Section | Slot | Description |
| :--- | :--- | :--- |
| `[picker]` | `text` | Normal candidate text & unstyled editor input. |
| `[picker]` | `muted` | Secondary description text in candidate items. |
| `[picker]` | `input_prefix` | Highlight for recognized route prefixes and aliases in query input. |
| `[picker]` | `selected` | Active selected row background and text. |
| `[picker]` | `selected_muted` | Secondary description text on the active selected row. |
| `[picker]` | `badge` / `badge_selected` | Metadata badge pill styles. |
| `[picker]` | `marker` | Selection indicator symbol (`▌`). |
| `[picker]` | `scrollbar` | Scrollbar thumb indicator. |
| `[picker.preview]` | `text`, `border`, `error` | Preview panel contents, border, and error state. |
| `[chrome]` | `text` / `muted_text` | Application frame background and fallback text styles. |
| `[chrome]` | `divider` | General frame divider lines. |
| `[chrome]` | `border` | Modal popup dialog borders. |
| `[chrome]` | `footer` | Bottom status bar base background and text. |
| `[chrome]` | `footer_title` | Active view title label on the left of footer and popup header. |
| `[chrome]` | `footer_status` | Active status text on the footer (e.g. `2 items`). |
| `[chrome]` | `footer_key` | Keyboard shortcut badges (e.g. `Enter`, `Ctrl+K`). |
| `[chrome]` | `error` | Global error notification banner. |
| `[capture]` | `text` | Captured subprocess output text. |

### 4. Theme Fallbacks and Invariants

- **Default Theme**: If `theme` is omitted, the built-in `terminal` theme is used, honoring your terminal's ANSI palette.
- **Selective Override**: Any section or slot not explicitly specified in your custom theme will automatically inherit its sensible fallback default from `terminal.toml`.
- **Pure Styling Contract**: Themes can only define visual presentation (`foreground`, `background`, modifiers like `bold`, `italic`, `underline`); they cannot change keybindings or runtime application behavior.
