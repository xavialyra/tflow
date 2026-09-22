---
title: "Input and Navigation Model"
type: "concept"
tags:
  - navigation
  - input
  - routing
  - lifecycles
description: "Detailed explanation of route resolution, lossless input transport, binding precedence, and view lifecycle contracts."
---

# Input and Navigation Model

The input and navigation subsystem is the operational backbone of `tflow`. It decouples raw terminal input transport, route management, view lifecycle states, and engine-specific interactions.

## 1. Core Model & Boundaries

The architecture clearly demarcates responsibilities across four distinct layers:

1. **`RouteCatalog`**: Read-only adapter that resolves a navigation target selector to the View it names (with its suite alias) and validates query schemas.
2. **`Router`**: Manages the runtime View Stack, current View location, and navigation transitions (`navigate`, `call`, `return`).
3. **`Session`**: The stateful driver that coordinates terminal polling, hosts the `Router`, handles global session commands, and invokes render cycles.
4. **`View` & `Engine`**: Concrete execution units (`picker`, `capture`, `embedded`) that receive dispatched input events, manage internal view state, and emit command requests.

## 2. Input Layering and Binding Precedence

To ensure reliable, deterministic interaction across complex nested views and embedded terminals, input dispatch adheres to strict precedence:

```text
[Raw Terminal Event]
         │
         ▼
[1. Session / Host Bindings] ─► (Global actions, e.g. ctrl+k when command folding is active)
         │ (unhandled)
         ▼
[2. Active View Commands] ───► (User-configured commands on the mounted view)
         │ (unhandled)
         ▼
[3. Engine Keymap] ──────────► (Engine-specific bindings, e.g. up/down, toggle preview)
         │ (unhandled)
         ▼
[4. Engine Input Consumer] ──► (Text editing in picker, byte passthrough in embedded)
```

- **Lossless Transport**: Raw byte sequences from the terminal (including complex chords and escape sequences) are preserved without lossy conversions until matched by a binding.
- **Overlays and Popups**: When a modal popup is mounted via `presentation.mode = "popup"`, only the topmost view receives input. Unmatched input does **not** fall through to background views. The command palette follows this same path as a built-in Popup Picker View; it is not a Session-local overlay.

## 3. Route Resolution and Query Contracts

- **Left Prefix Display**: The host renders the current View's suite alias as an optional left prefix on the input line, and when `[defaults.picker] left_prefix_backspace` opts in, Backspace returns to the parent or root View while that prefix is rendered. The prefix is presentational and never changes key handling.
- **Route Query Scope**: Arguments passed via CLI or navigation actions are validated against the target view's declared `[views.<name>.query]` schema before the view is mounted.

## 4. View Lifecycle Sequences

### The `call` and `return` Boundary
- When a command uses `type = "call"`, the Router establishes a return frame on the View stack. A producer call may return a typed operation from a literal or JSON protocol handler.
- When the target View invokes `type = "return"`, the active child View is popped and its result is delivered to the recorded caller. A producer `return_processor` runs only after the child is closed and the caller is active.
- The built-in command palette is the ordinary Popup Picker View at `__commands:main`. The host opens it with command descriptors in its navigation parameters, then resolves its returned `CommandRef` against the current registry after the popup closes. Parameter editing uses the separate native Form View at `__form:main`; submitting it replaces the target View through the normal query-schema validation path.

### Task Correlation and Cancellation
- Asynchronous tasks (such as background script feeds or PTY streams) are tagged with unique task IDs associated with their owning view.
- When a view is unmounted or popped from the stack, all ongoing background tasks associated with that view receive cooperative cancellation signals.
