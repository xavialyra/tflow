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

The input and navigation subsystem is the operational backbone of `tui-launcher`. It decouples raw terminal input transport, route management, view lifecycle states, and engine-specific interactions.

## 1. Core Model & Boundaries

The architecture clearly demarcates responsibilities across four distinct layers:

1. **`RouteCatalog`**: Read-only directory of declared views, aliases, query schemas, and canonical route paths.
2. **`Router`**: Manages the runtime View Stack, current route location, and navigation transitions (`navigate`, `call`, `return`).
3. **`Session`**: The stateful driver that coordinates terminal polling, hosts the `Router`, handles global session commands, and invokes render cycles.
4. **`View` & `Engine`**: Concrete execution units (`picker`, `capture`, `embedded`) that receive dispatched input events, manage internal view state, and emit command requests.

## 2. Input Layering and Binding Precedence

To ensure reliable, deterministic interaction across complex nested views and embedded terminals, input dispatch adheres to strict precedence:

```text
[Raw Terminal Event]
         │
         ▼
[1. Session Bindings] ───────► (Global actions, e.g. ctrl+k Command Palette)
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
- **Overlays and Popups**: When a modal popup is mounted via `presentation.mode = "popup"`, only the topmost view receives input. Unmatched input does **not** fall through to background views.

## 3. Route Resolution and Query Contracts

- **Prefix Routing**: In the default view, typing `<alias> <query>` or `<plugin:view> <query>` automatically commits the route and switches to the target view with the remainder parsed as its query argument.
- **Route Query Scope**: Arguments passed via CLI or navigation actions are validated against the target view's declared `[views.<name>.query]` schema before the view is mounted.

## 4. View Lifecycle Sequences

### The `call` and `return` Boundary
- When a command uses `type = "call"`, the router establishes a return frame on the view stack.
- When the target view invokes `type = "return"`, the top view is popped, and its result payload is adapted and delivered back to the caller frame.

### Task Correlation and Cancellation
- Asynchronous tasks (such as background script feeds or PTY streams) are tagged with unique task IDs associated with their owning view.
- When a view is unmounted or popped from the stack, all ongoing background tasks associated with that view receive cooperative cancellation signals.
