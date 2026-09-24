---
title: "ADR 0007: Declarative Popup Presentation and Viewport-Relative Geometry"
type: "concept"
tags:
  - adr
  - architecture
  - ui
  - popup
  - presentation
  - geometry
description: "Establish viewport-relative geometry, declarative responsive sizing, and optical anchoring for modal popup views."
---

# ADR 0007: Declarative Popup Presentation and Viewport-Relative Geometry

- **Status**: Accepted
- **Date**: 2026-09-23
- **Implementation status**: Implemented
- **Scope**: `ViewPresentation` contract, `ContentHost` popup placement and dimensions, TOML configuration schema, Producer navigation operations, and nested popup stacking rules.
- **Related decisions**: [ADR 0001](0001-decentralized-workflow-extensions.md), [ADR 0003](0003-unified-action-registration-and-host-folding.md), [ADR 0004](0004-scoped-command-registration.md), [Input and Navigation Model](../explanation/input-and-navigation-model.md).

## Context

`tflow` uses modal popups for self-contained, transient interactions—such as the built-in command palette (`__commands:main`), parameter input forms (`__form:main`), and workflow-defined popup pickers.

A keyboard-driven workflow launcher demands a consistent presentation model for modal views:

1. **Independent Viewport Geometry**: Multi-step workflows frequently push secondary modal views (e.g., a parameter form called from a compact picker). Each modal view requires stable geometry derived from the global viewport rather than the physical footprint of any covered view.
2. **Optical Ergonomics for Command Palettes**: Fast-filtering inputs and command palettes benefit from top-center anchoring (top 15%–25% of the viewport), aligning with the operator's natural eye level in terminal emulators.
3. **Responsive Terminal Adaptation**: Displays range from compact 80-column multiplexer panes to ultra-wide monitors. Popup layouts must declare responsive proportions alongside minimum and maximum safety bounds.

This document formalizes the geometry calculation, placement anchors, and dimension contracts for all modal popup views.

## Decision

### 1. Terminal Viewport Reference for All Modal Popups

All modal popup instances compute their layout geometry relative to the overall terminal viewport (`frame.area()`), distinct from the base view's padded `content_area`.

```text
+----------------------- Entire Terminal Viewport -----------------------+
|                                                                        |
|         +----------------- Primary Popup -----------------+            |
|         |                                                 |            |
|         |     +------------- Secondary Popup -----------+ |            |
|         |     |                                         | |            |
|         |     | Calculated relative to Terminal         | |            |
|         |     | Viewport without inherited bounds       | |            |
|         |     +-----------------------------------------+ |            |
|         +-------------------------------------------------+            |
+------------------------------------------------------------------------+
```

- When multiple popups exist on the navigation stack, each popup renders within the shared terminal viewport bounds according to its own presentation configuration.
- Secondary popups layer directly over preceding popups without inheriting or being constrained by predecessor bounding boxes.
- Anchors (e.g. `BottomRight`, `TopLeft`) align cleanly against the true physical boundaries of the terminal without artificial gaps caused by base view padding or global footer reservations.
- The active topmost popup renders its border, title, and bottom-border hints or status. Inactive covered popups render passive borders without command hints.

### 2. Declarative 9-Box Grid Anchoring and Transparent Offsets

`ViewPresentation` specifies viewport alignment via an explicit 9-box grid anchor:

```toml
[presentation]
mode = "popup"
anchor = "top"       # "center" (default) | "top" | "bottom" | "left" | "right" | "top-left" | "top-right" | "bottom-left" | "bottom-right"
offset_x = 0         # Optional horizontal offset from the anchor edge in cells (default 0)
offset_y = 0         # Optional vertical offset from the anchor edge in cells (default 0)
```

Alignment is modeled as two orthogonal axes:
- **Horizontal**: `Left` (`area.x + offset_x`), `Center` (horizontally centered), `Right` (`area.x + remaining_x - offset_x`).
- **Vertical**: `Top` (`area.y + offset_y`), `Center` (vertically centered), `Bottom` (`area.y + remaining_y - offset_y`).

Offsets default transparently to `0` with zero implicit guessing:
- `anchor = "top"` aligns flush against the top edge of the terminal viewport. To create an upper visual margin for command palettes, workflows declare explicit offsets (e.g. `offset_y = 2`).
- Combined anchors (e.g. `top-left`, `top-right`, `bottom-right`) enable specialized UI layouts such as drawer-style sidebars, top-right notifications, and bottom-right status panels without breaking the centralized modal flow.

### 3. Responsive Dimensions and Boundary Constraints

Width and height accept either fixed terminal cells or viewport-relative percentages, with optional boundary clamps:

```toml
[presentation]
mode = "popup"
width = "80%"        # Percentage of content area width
height = 18          # Absolute row count
min_width = 50       # Minimum width clamp
max_width = 110      # Maximum width clamp
```

The underlying schema and Rust representations:

```rust
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PopupAnchor {
    #[default]
    Center,
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DimensionConstraint {
    Cells(u16),
    Percentage(u8), // e.g., "80%"
}
```

- Percentages resolve dynamically against the available `content_area` width or height during frame rendering.
- Clamping enforces `min_dimension <= resolved <= max_dimension`, capped strictly by the bounds of `content_area`.
- Omitted width and height default to standardized fallback dimensions (e.g., 72 cells wide, 16 cells high).

### 4. Global Focus Isolation and Non-Focus Backdrop Dimming

To maintain visual hierarchy when modal views are active, `ContentHost` applies non-destructive dimming to all visible screen cells outside the topmost active focus rectangle:

- **Single Focus Rectangle Boundary**: When an active popup is present, its bounding rectangle (`popup_rect`, including borders and content) forms the exclusive focused area.
- **Universal Backdrop Dimming**: All cells outside the active focus rectangle—including base view text, inactive covered popup borders and bodies, and the host footer—receive the global `chrome.backdrop` style once per frame. The default sets the foreground to `scheme:muted` and adds ANSI faint; it does not scale RGB values.
- **Style Overlay**: Only configured colors and modifiers are patched. The default preserves background colors, bold, and reversed selections. Explicit modifier values such as `bold = false` remove that modifier. No workflow-level backdrop configuration is exposed.
- **Global Theme Configuration**: Backdrop dimming is globally managed via `[chrome]` in `themes/<name>.toml`:
  ```toml
  [chrome]
  dim_backdrop = true    # Default true; set to false to disable dimming

  [chrome.backdrop]
  foreground = "scheme:muted"
  dim = true
  ```

### 5. Architectural Invariant: Presentation is Pure Layout

`ViewPresentation` is strictly a layout concern handled by `ContentHost`:

- **Router Invariance**: Presentation properties alter layout geometry only. They do not alter the view stack, navigation semantics, or return value propagation.
- **Input Exclusivity**: In accordance with [ADR 0004](0004-scoped-command-registration.md), the topmost view holds exclusive input focus regardless of its anchor or dimensions. Unmatched keys do not fall through to covered views.
- **Producer Protocol Parity**: The `Navigate` and `Call` operations in the Producer Protocol expose the exact same presentation schema and validation rules.

## Rejected Alternatives

### Absolute (X, Y) Terminal Coordinates
Allowing workflows or producers to supply explicit `(x, y)` cell coordinates is rejected:
- It breaks responsive resizing when terminal dimensions change.
- It introduces failure modes where popups render off-screen or collide awkwardly with host chrome.
- Declarative anchors (`top`, `center`, `bottom`) satisfy layout intent without binding to transient terminal dimensions.

### Window Management / Draggable Popups
Adding mouse drag-to-move, manual resizing handles, or multi-window tiling is rejected:
- `tflow` is an intentional, keyboard-driven launcher and workflow orchestrator, not a window manager or desktop environment.
- Floating window state machines introduce immense complexity for negligible keyboard-workflow value.

### Cursor-Relative Anchoring
Anchoring popups to an active text cursor is rejected:
- `tflow` views are self-contained modal workspaces (Pickers, Forms, Embedded terminals), not inline text-editor buffers.
- Modals require predictable global focal points, not local cursor tracking.

## Consequences

### Positive
- **Ergonomics**: Command palettes and quick pickers anchor to the upper focal plane, providing a focused launcher experience.
- **Multi-View Composition**: Nested modal workflows (e.g., parameter forms called from popup pickers) display without boundary clipping.
- **Responsive Layout**: Workflows scale across compact multiplexer panes and wide monitors via percentage sizing and boundary constraints.

### Negative / Trade-offs
- Schema expansion for `ViewPresentation` across TOML configurations and JSON Producer messages.
- `ContentHost::popup_rect` requires percentage resolution and boundary clamping based on active terminal dimensions.
