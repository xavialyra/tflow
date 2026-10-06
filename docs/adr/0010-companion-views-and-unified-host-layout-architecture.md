---
title: "ADR 0010: Companion Views and Unified Host Layout Architecture"
type: "concept"
tags:
  - adr
  - architecture
  - layout
  - companion
  - navigation
  - focus
  - omnibar
description: "Promote picker preview into universal companion views across all engines, formalize single-stack projection topology ([A|B]), introduce companion toggle command operations, define bidirectional focus navigation with deterministic Escape ladder, and decouple the host Omnibar prompt via engine input policies."
---

# ADR 0010: Companion Views and Unified Host Layout Architecture

- **Status**: Accepted, with current-design amendment below
- **Date**: 2026-10-02
- **Scope**: Companion view architecture (`companion`), deprecation of engine-specific `picker.preview`, single-stack projection model (`[A|B]` vs `[A, B]`), command-level `companion` toggle operations, bidirectional focus navigation (`Ctrl+l`/`Ctrl+Right`, `Ctrl+h`/`Ctrl+Left`), deterministic Escape ladder, and universal host Omnibar with `UnhandledInputBehavior`.
- **Related decisions**: [ADR 0006](0006-feed-removal-workflow-scoped-commands-and-item-bindings.md), [ADR 0007](0007-declarative-popup-presentation-and-viewport-relative-geometry.md), [ADR 0008](0008-workflow-defined-features-host-bindings-and-data-driven-chrome.md), [ADR 0009](0009-canonical-view-engines-and-flat-command-syntax.md).

## Current Design Amendment

The bidirectional focus handoff, focus-specific Escape ladder, and active/inactive focus-border contracts below are retained as historical rationale, not current implementation requirements. Returning uses the foreground View's ordinary close/back behavior (`Escape`). There is no pane-focus state machine or separate focus keys.

Host-owned Omnibar editing and shared pane geometry are implemented convergence results. The Host stores the editor per mounted View instance, renders the Omnibar, and delivers pane-specific Resize events from the same layout calculation used for rendering. Picker receives read-only editor snapshots; Form fields and Embedded PTY input remain local engine contracts.

See [Companion Navigation and Host Ownership](../explanation/companion-host-ownership.md) for current semantics and implementation boundaries.

## Historical Proposal

The Context, Decision, and Consequences below preserve the original proposal, not copy-paste configuration or the current API. In particular, `companion_input`, pane-focus actions, and the old command examples are superseded. Current contracts are documented in [Companion Navigation and Host Ownership](../explanation/companion-host-ownership.md) and [Configure Companion Views](../how-to/companion-views.md).

## Context

Following the consolidation of view engines in ADR 0009 and the unification of presentation layers in `v0.1.0-alpha.7` (embedding `CaptureSession` into `picker.preview`), the system exposed fundamental structural asymmetries:

1. **Preview as a Privileged Second-Class Citizen:**
   Preview remained hardcoded exclusively inside `picker`. Other engines (such as `form` for real-time document rendering, or `capture` for split inspection) could not leverage side-by-side companion panes. Furthermore, preview panes were passive and non-interactive: users could not focus into them, scroll via independent view bindings, copy text directly, or execute view-specific commands without closing or navigating away from the parent picker.

2. **The Dilemma of Terminal Multiplexing vs. Single-View Stacks:**
   Introducing generalized multi-pane layouts risks degrading `tflow` into a bloated terminal multiplexer (like tmux or zellij), saddled with complex binary split trees (BSP), ambiguous prefix keys, focus traps, and orphan panes when master views close. Conversely, keeping views strictly single-column full-screen severely limits workflows requiring master-detail inspection or side-by-side contextual tasks.

3. **Ambiguity of the Top Input Bar (Omnibar):**
   Historically, the top input row was tightly coupled to `picker`, even though query parameters (`input`), route prefixes (`left_prefix`), and line editing are host-level primitives. When considering multi-pane layouts, the ownership of this top input row and unhandled keyboard input (fallbacks) required clear architectural boundaries.

## Decision

### 1. The Single-Stack Companion Topology (`[A|B]` Model)

The runtime maintains a **strictly linear view stack** (`Stack<ViewInstance>`). Multi-pane presentation is modeled not as a split-tree window manager, but as a **projected companion attachment** on the active stack-top view:

- **State `[A|B]` (Companion Projection State)**:
  View `A` is the active master view on the stack. View `B` is attached as a companion, rendered side-by-side. Focus remains on `A`. `B` passively observes and reacts to state updates from `A`.
- **State `[A, B]` (Promoted Stack State)**:
  When the user focuses into `B`, `B` is promoted to a first-class stack-top view. `B` receives full keyboard input and owns the footer command bar.
- **Deterministic Return**:
  Pressing `Escape` (or unfocusing) pops `B` from the top of the stack, cleanly restoring state `[A|B]`. If `A` is navigated away to `C`, the stack becomes `[A, C]`; `B` suspends cleanly with `A`. Upon returning from `C`, the state `[A|B]` is restored without ghost panes or orphaned windows.

```text
       Navigation Stack: [ ..., A ]
                                 │
                     ┌───────────┴───────────┐
                     ▼                       ▼
            Primary Pane (A)       Companion Pane (B)
            [Active Focus]         [Passive Follower]
                     │
             (Press Ctrl+l / Tab)
                     ▼
       Navigation Stack: [ ..., A, B ]
                                   ▲
                             [Active Focus]
```

### 2. Universal Companion Configuration (Pure Reference Syntax)

In accordance with ADR 0009's clean-break policy, `picker.preview` is superseded by the universal `companion` feature available to all views. Companion views are **first-class named views**; inline or anonymous view declarations are forbidden.

#### Static View Declaration
A view declares its default companion and explicit input binding directly:

```toml
[views.pods]
engine = "picker"
items.source = "kubectl get pods"
companion = "pod_logs"         # Target view name
companion_input = "$item"      # Explicit data binding

[views.pod_logs]
engine = "capture"             # Explicit engine declaration (ADR 0009)
output.script = "kubectl logs -f $input"
commands = [
  { key = "ctrl+r", run = "kubectl restart pod $input" }
]
```

To eliminate hidden magical assumptions, data binding is strictly explicit via `companion_input`. If omitted, the companion view receives an empty parameter.

### 3. Command-Level Companion Toggle Operations

`companion` is formalized as a first-class command operation alongside `navigate`, `run`, and `close`. It natively exhibits **Toggle semantics**: invoking `companion = "X"` mounts `X`; invoking it again when `X` is already visible toggles it closed.

#### Static Command Declarations
```toml
# In [commands.<id>]
[commands.toggle_logs]
companion = "pod_logs"
input = "$item"

[commands.toggle_metrics]
companion = "pod_metrics"
input = "$item"
```

#### Dynamic Script Protocol
Dynamic scripts can emit companion operations identically to navigate operations:
```json
{
  "version": 1,
  "companion": {
    "target": "pod_logs",
    "input": "pod-12345"
  }
}
```

### 4. Bidirectional Focus Handoff and Navigation Keybindings

Moving focus between primary and companion panes is governed by standard host actions:

- `host.focus_companion`: Transfers focus from Primary (`A`) to Companion (`B`). Default keybindings: `ctrl+l`, `ctrl+right`.
- `host.focus_primary`: Transfers focus from Companion (`B`) back to Primary (`A`). Default keybindings: `ctrl+h`, `ctrl+left`.

#### Deterministic Escape Ladder
To avoid ambiguous escape traps, `Escape` follows an invariant monotonic unwinding ladder:
1. **Focus on Companion (`B`)**: `Escape` unfocuses `B` and returns focus to `A` (`[A, B]` -> `[A|B]`).
2. **Focus on Primary (`A`) with Non-Empty Omnibar**: `Escape` clears the Omnibar query text.
3. **Focus on Primary (`A`) with Empty Omnibar**: `Escape` pops `A` and navigates back to the caller workflow or parent view.
4. **Focus on Root View**: `Escape` exits `tflow`.

### 5. Universal Host Omnibar and Engine Input Policies

The top input row (Omnibar) is recognized as a host-level console prompt comprising:
1. `left_prefix` (workflow/route breadcrumb and hierarchical backspace navigation);
2. `LineEditor` (universal text editor buffer and cursor);
3. State indicators (searching, filtering, dirty status).

To resolve key dispatch ambiguity without key-interception conflicts, active views declare an `UnhandledInputBehavior`:

```rust
pub enum UnhandledInputBehavior {
    /// Printable characters and line-editing keys flow to the Host Omnibar (e.g. Picker filtering)
    ForwardToOmnibar,
    /// Unhandled input is consumed directly by the engine (e.g. Embedded PTY, Form field focus)
    ConsumeLocally,
    /// Unhandled input is safely ignored (e.g. Capture read-only viewport)
    Ignore,
}
```

- When `A` (Picker) is focused: input policy is `ForwardToOmnibar`. Unbound keys insert text into the Omnibar.
- When focus transfers to `B` (Capture companion): input policy switches to `B`'s policy (`Ignore` or view-specific bindings). The Omnibar **hides its cursor** to clearly indicate inactive state.

### 6. Chrome Visual Contract

- **Borders**: The focused pane renders using the active accent theme slot (`border.focus` / `accent`). The unfocused companion renders using the inactive border slot (`border.inactive`).
- **Footer Command Bar**: Follows active focus identically to standard navigation: the footer renders the commands and key hints of whichever view currently holds primary focus.
- **Popups**: Popups remain global, modal, and centered across the full terminal window, dimming both primary and companion panes uniformly.

## Consequences

### Positive
- **Complete Feature Parity for Previews**: Companions are full views with independent commands, scrolling, themes, and clipboard copying.
- **Zero Window-Management Complexity**: Avoids arbitrary split trees, orphan pane bugs, and nested window manager state.
- **Ergonomic Switching**: Allows binding multiple inspector views (logs, metrics, config) to distinct function keys with native toggle support.
- **Explicit Dataflow**: Explicit `companion_input` ensures transparent, verifiable parameter binding across all engine combinations.

### Negative / Migration
- Existing `[views.<name>.preview]` blocks must be migrated to separate `[views.<target>]` with `companion = "<target>"`.
- Workflows relying on implicit `$item` injection in previews must declare `companion_input = "$item"`.
