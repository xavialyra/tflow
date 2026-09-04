---
title: "How to Configure View Navigation and Popups"
type: "guide"
tags:
  - navigation
  - popup
  - modal
  - commands
description: "How to configure view transitions, popup overlays, stack unwinding, and call/return patterns."
---

# How to Configure View Navigation and Popups

In `tui-launcher`, views are managed via a stack. Commands can push new views, replace current views, or present views in modal popup overlays.

## Problem

You want to open a secondary view (such as a confirmation dialog or a command palette) as a popup modal over the current view, and return data or control back upon completion.

## Solution

### 1. Configure a Popup Presentation

In your command definition, set `type = "call"` and add a `presentation` block under `payload`:

```toml
[views.main.commands.select_action]
key = "ctrl+o"
label = "Actions"
type = "call"

[views.main.commands.select_action.payload]
target = "selectors:actions"

[views.main.commands.select_action.payload.presentation]
mode = "popup"
width = 70
height = 18
```

- **Popup Dimensions**: Width and height are measured in terminal cells and automatically clamped if the terminal window is smaller.
- **Input Isolation**: While the popup is active, only the top popup view receives keystrokes; the parent view stays mounted and visible underneath.

### 2. Configure the Target View to Return

In the target view (`selectors:actions`), define a command to return to the parent view:

```toml
[views.actions.commands.confirm]
key = "enter"
label = "Confirm"
type = "return"

[views.actions.commands.confirm.payload]
value = "{{ selection.value }}"
```

### 3. Replace Navigation vs. Stack Push

If you do not want to keep the current view in the backstack, use `type = "navigate"` with `replace = true`:

```toml
[views.step1.commands.next]
key = "enter"
type = "navigate"

[views.step1.commands.next.payload]
target = "workflow:step2"
replace = true
```

When the user navigates back from `step2`, they will skip `step1` and return directly to the view before `step1`.
