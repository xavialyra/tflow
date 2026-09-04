---
title: "How to Create and Apply Custom Themes"
type: "guide"
tags:
  - themes
  - styling
  - colors
  - palette
description: "How to configure color palettes, semantic schemes, and element bindings for custom launcher aesthetics."
---

# How to Create and Apply Custom Themes

`tui-launcher` provides a declarative styling system composed of **Palettes**, **Schemes**, and **Element Bindings**.

## Problem

You want to customize the look and feel of the launcher to match your terminal emulator or desktop color scheme.

## Solution

### 1. Create a Theme File

Themes are placed in `$XDG_CONFIG_HOME/tui-launcher/themes/<name>.toml`. For example, create `~/.config/tui-launcher/themes/nord.toml`:

```toml
# themes/nord.toml
[palette]
nord0 = "#2E3440"
nord4 = "#D8DEE9"
nord8 = "#88C0D0"
nord11 = "#BF616A"

[scheme]
primary = "palette:nord8"
background = "palette:nord0"
text = "palette:nord4"
error = "palette:nord11"

# Style concrete UI components
[bindings.picker-selected]
foreground = "scheme:primary"
bold = true

[bindings.picker-match]
foreground = "palette:nord8"
underline = true

[bindings.footer-key]
foreground = "scheme:primary"
bold = true

[bindings.footer-label]
foreground = "scheme:text"
```

### 2. Activate the Theme in `config.toml`

In your `~/.config/tui-launcher/config.toml`, set the `theme` field:

```toml
theme = "nord"
```

### 3. Verify Theme Fallbacks

- If `theme` is omitted, the built-in `terminal` theme is used, honoring your terminal's default ANSI colors.
- Any element binding not specified in your theme will automatically fall back to its sensible default styling.
- Theme files can only define visual styles; they cannot execute commands or change application behavior.
