---
title: "Configure Companion Views"
type: "guide"
tags:
  - companion
  - picker
  - capture
  - form
  - embedded
description: "Configure side-by-side companion views across engines with automatic state synchronization and toggle commands."
---

# Configure Companion Views

A companion view is a secondary view displayed side-by-side with a primary view in a split layout. Picker and Capture companions track the primary view's state (such as the selected item or form drafts) without stealing keyboard focus. Form and Embedded companions capture that state at mount time.

## 1. Attach a Companion View

Declare `companion = "<target>"` on any primary view definition. The companion mounts automatically when the primary view opens:

```toml
[views.main]
engine = "picker"
companion = "details"

[views.details]
engine = "capture"

[views.details.capture.output]
file = "scripts/details.py"
```

## 2. Toggle Companion Visibility

To let users toggle the companion pane open or closed, define a workflow command with `companion` and bind it to a key (such as `Ctrl-P`):

```toml
[views.main.bindings]
"ctrl+p" = "toggle_details"

[commands.toggle_details]
label = "Toggle Details"
companion = "details"
```

When `query` is omitted in the toggle command, Picker/Capture companions track the primary view's live selection; Form/Embedded companions retain a mount snapshot. To pass static or custom parameters instead, specify `query = { ... }` in the command.

## 3. Multiple Companions and Dynamic Projections

For complex views that need multiple companion attachments (such as a code preview and an inspector) with explicit parameter contracts, declare companion commands directly:

```toml
[views.main]
engine = "picker"
companion = "preview"  # Default companion command or view

[views.main.bindings]
"ctrl+p" = "preview"
"ctrl+i" = "inspector"

# Companion commands declare target and dynamic parameter projections directly
[commands.preview]
label = "Toggle Preview"
type = "companion"
target = "details"
args = { title = "$selection.display", code = "$selection.value" }

[commands.inspector]
label = "Inspect"
type = "companion"
target = "meta_info"
args = { id = "$selection.value", search = "$input" }
```

Available projection tokens include:
- `$selection`: The complete selected item object.
- `$selection.<path>` (e.g. `$selection.value`, `$selection.display`): Specific scalar or nested fields from the active item.
- `$input`: The active query or omnibar buffer string.
- `$query.<path>`: The caller's existing parameters.

## 4. Navigation and Keyboard Focus

- **Input focus**: The companion is a passive attachment; keyboard focus remains on the primary view.
- **Entering companion**: Bind or invoke `@host:open_companion` to open a new instance of the companion target in the foreground. This does not transfer existing drafts, scroll position, or a running PTY. There is no separate pane-focus state.
- **Customizing key in host.bindings**: Rebind or disable the host command in `settings.toml` or `suite.toml`:
  ```toml
  [host.bindings]
  "ctrl+l" = false                    # Disable the shortcut
  "ctrl+o" = "@host:open_companion"   # Or remap to another key
  ```
- **Unwinding**: Pressing `Escape` in a pushed view returns to the parent view or exits.

## 4. State Synchronization

- **Picker Companion**: When selecting an item in a Picker, the companion view automatically receives the selected item and query parameters.
- **Form Companion**: Form companions retain their mount snapshot, including drafts, rather than restarting or replacing local editing state on source updates.
- **Static Snapshots**: Form companions and embedded PTY processes retain their initial source data rather than discarding drafts or continuously restarting. Toggle off/on to capture another selection.
- **Capture producers**: Companion scripts receive projected parameters directly in `context.parameters` (matching the target view's declared query schema). No private engine states are leaked. Explicit command `args`/`query` templates enable dynamic context projection and live selection tracking.
