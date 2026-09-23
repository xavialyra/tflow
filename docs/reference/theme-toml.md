---
title: "Theme TOML Specification"
type: "reference"
tags:
  - themes
  - styling
  - schema
description: "Flat scheme colors, component style fields, built-in defaults, and workflow slot merge rules."
---

# Theme TOML Specification

A user theme is `themes/<name>.toml` relative to the selected configuration file. Root `theme = "<name>"` selects that file; the CLI `--theme` option takes precedence. Omitting the root setting uses the built-in `terminal` theme. `--theme terminal` explicitly selects the built-in theme.

The only top-level tables are `scheme`, `picker`, `chrome`, `capture`, `form`, and `workflows`. Unknown fields, including a `palette` table or theme inheritance configuration, are errors. There is no inheritance chain between user themes.

## Color Values

| Location | Accepted syntax |
| :--- | :--- |
| `[scheme]` values | `ansi:NAME` or `#RRGGBB` |
| Style `foreground` and `background` | `ansi:NAME`, `#RRGGBB`, or `scheme:NAME` |

ANSI names are case-insensitive and include the 16 terminal colors: `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `gray`, `bright-black`, `bright-red`, `bright-green`, `bright-yellow`, `bright-blue`, `bright-magenta`, `bright-cyan`, and `bright-white`; `white` and `reset` are also accepted. In the Ratatui mapping, `gray` is ANSI 37, `bright-black` is ANSI 90, `bright-red` through `bright-cyan` are ANSI 91 through 96, and `bright-white` is ANSI 97. The existing `white` token is an alias for bright white. Prefixes are lowercase. Hex colors require exactly six hexadecimal digits. Leading and trailing whitespace around a quoted color literal or reference is ignored. Internal whitespace in ANSI or hex literals is invalid. Bare color names, indexed colors, shorthand hex, and other reference prefixes are invalid.

`[scheme]` is a flat map with arbitrary nonempty string keys. Names are case-sensitive and stored without trimming; names containing dots or spaces must be quoted as TOML keys. Reference lookup uses the exact name after `scheme:` once the outer whitespace of the complete reference has been removed. Scheme values cannot refer to other scheme entries. A style reference to an unknown name is an error, and unused invalid scheme values are also errors.

## Built-in Scheme

| Name | Value |
| :--- | :--- |
| `background` | `ansi:reset` |
| `foreground` | `ansi:reset` |
| `muted` | `ansi:bright-black` |
| `accent` | `ansi:yellow` |
| `surface` | `ansi:black` |
| `border` | `ansi:yellow` |
| `selection` | `ansi:reset` |
| `success` | `ansi:green` |
| `warning` | `ansi:yellow` |
| `info` | `ansi:cyan` |
| `error` | `ansi:red` |

Normal surfaces use `background` and `foreground`; secondary text uses `muted`. Dividers, borders, and scrollbars use `border`. Markers, cursors, and footer titles use `accent`. Input prefixes and shortcut keys use `accent` foregrounds on `surface` backgrounds. Selected rows use `selection` backgrounds and `foreground` primary text; selected secondary text and badges retain `muted` foregrounds. Selection background defaults to terminal reset, preserving the normal background and primary text color. Bold text and the accent marker distinguish the active row without adding a background highlight. Semantic status styles use `success`, `warning`, `info`, and `error`. Banner errors use `error` backgrounds with `background` foregrounds.

The [built-in theme](../../src/ui/theme/builtin/terminal.toml) defines all component bindings and modifier defaults.

## Style Fields and Merging

Style bindings accept optional `foreground`, `background`, `bold`, `italic`, `underline`, `strikethrough`, `dim`, `reversed`, and a `selected` style table. Modifier fields are booleans. Unknown style fields are errors.

The complete built-in theme is the baseline. User scheme entries replace matching keys and add new keys. Component styles merge field by field, including fields inside selected tables. All references resolve against the merged scheme, so an inherited binding sees user color changes and a user binding can reference an inherited scheme entry.

An omitted field inherits; `false` overrides `true`. `ansi:reset` is an explicit terminal-default color, not an omitted value.

For badges and workflow custom slots, selected styles inherit the normal foreground and modifiers unless the merged selected table supplies those fields. A selected background comes from the selected table when specified, otherwise from `picker.selected.background`. An explicit `ansi:reset` selected background overrides this fallback. The built-in badge selected table supplies its own foreground, background, and bold setting.

Selected badge fields are configured only under `[picker.badge.selected]`; the removed `[picker.badge_selected]` table is an unknown-field error. `picker.selected` and `picker.selected_muted` are independent row bindings with their own baseline defaults.

## Component Slots

| Section | Slot | Description |
| :--- | :--- | :--- |
| `[picker]` | `text` | Normal candidate text & unstyled editor input. |
| `[picker]` | `muted` | Secondary description text in candidate items. |
| `[picker]` | `placeholder` | Hint text in an empty query input (see `input_placeholder`). |
| `[picker]` | `input_prefix` | Highlight for the non-root left prefix marker in query input. |
| `[picker]` | `cursor` | Styled pseudo-cursor in the query input. |
| `[picker]` | `selected` | Active selected row background and text. |
| `[picker]` | `selected_muted` | Secondary description text on the active selected row. |
| `[picker]` | `badge` | Normal metadata badge style; `[picker.badge.selected]` configures its selected state. |
| `[picker]` | `marker` | Selection indicator symbol (`▌`). |
| `[picker]` | `scrollbar` | Scrollbar thumb indicator. |
| `[picker.preview]` | `text`, `border`, `error` | Preview panel contents, border, and error state. |
| `[chrome]` | `text` | Application frame background style. |
| `[chrome]` | `divider` | General frame divider lines. |
| `[chrome]` | `border` | Modal popup dialog borders. |
| `[chrome]` | `footer` | Bottom status bar base background and text. |
| `[chrome]` | `footer_title` | Active view title label on the left of footer and popup header. |
| `[chrome]` | `footer_status` | Active status text on the footer (e.g. `2 items`). |
| `[chrome]` | `footer_key` | Keyboard shortcut badges (e.g. `Enter`, `Ctrl+K`). |
| `[chrome]` | `error` | Global error notification banner. |
| `[chrome]` | `backdrop` | Style applied to non-focus background cells (base views, covered popups, footer) when a modal popup is active. Defaults to `foreground = "scheme:muted"`, `dim = true`. |
| `[chrome]` | `dim_backdrop` | Boolean (`true` by default). Master switch controlling whether the `chrome.backdrop` style is applied to non-focus background cells. |
| `[capture]` | `text` | Captured subprocess output text. |
| `[form]` | `label`, `input`, `focused`, `border`, `focused_border`, `error` | Field labels, normal and focused editors, field borders, and validation errors. |

The picker cursor is a styled cell in the render buffer. Cursor blink is not a theme field. Themes control presentation only; they do not configure keys or application behavior.

## Backdrop Style

The global `[chrome.backdrop]` style is patched onto cells outside the active popup's outer rectangle, including visible inactive popup borders. The focused popup remains unchanged. With no active popup, this overlay is not applied.

```toml
[chrome]
dim_backdrop = true

[chrome.backdrop]
foreground = "scheme:muted"
dim = true
```

These are the built-in defaults. User fields merge with these defaults. Omitted background and other modifiers preserve each rendered cell's existing values, including bold and reversed selections. Explicit `bold = false` removes bold; `dim = false` disables the inherited faint modifier while retaining the foreground override. For a custom color without ANSI faint:

```toml
[chrome.backdrop]
foreground = "#888888"
dim = false
```

Choose a foreground suited to the terminal background; ANSI palette colors and faint intensity depend on the terminal. This is a style overlay, not alpha blending. `chrome.dim_backdrop = false` disables the entire overlay. Workflow presentation tables do not configure it.

## Workflow Custom Slots

Workflow files declare defaults under `[styles.<slot>]` and optional `[styles.<slot>.selected]`. A theme overrides them under `[workflows.<workflow-id>.styles.<slot>]`, including an optional `.selected` table. Theme fields override workflow fields, with omitted normal and selected fields retained before reference resolution. Theme-only slots are supported too.

Workflow references use the active merged scheme. Built-in names such as `scheme:accent` let a workflow work with every theme; a custom name must exist in the active theme. See the [workflow reference](workflow-toml.md) and [custom theme guide](../how-to/custom-themes.md).
