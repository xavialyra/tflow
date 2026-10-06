---
title: "Companion Navigation and Host Ownership"
type: "concept"
tags:
  - companion
  - navigation
  - host-layout
  - omnibar
description: "Current Companion navigation and input semantics, and the remaining boundaries of Host layout and Omnibar ownership."
---

# Companion Navigation and Host Ownership

## Navigation Is Ordinary Stack Navigation

A companion is a side-by-side attachment to its primary View, not a second keyboard-focus owner. Normal navigation actions push views onto the stack, and closing a foreground View with `Esc` reveals the primary and its attachment again.

Return behavior belongs to the active View's ordinary back/close commands (`Esc`).

Popups remain modal. A foreground popup does not provide input to its parent's companion.

## One Request Construction Contract

Attachment and navigation into a companion resolve the target route, construct its default query, validate that query, and optionally supply a separate editable input seed. The input seed does not replace the query's schema or its default values.

A failed replacement during request validation, creation, or activation leaves the previous companion mounted. A failed automatic attachment is reported as a diagnostic without rejecting the already committed primary navigation. Errors while closing an existing attachment are still subject to lifecycle cleanup semantics; they are not a transactional rollback guarantee.

## Companion Dataflow and Query Synchronization

Declaring `companion = "<target>"` on a primary View establishes an attached companion opened by default. The primary View's publication and parameters are captured as an explicit source payload. Picker and Capture companions opt into live updates; Form and Embedded companions retain their mount snapshot so updates do not discard drafts or restart PTYs. Source updates are not mutations of the target's validated navigation query.

Companion commands accept `query` identical to `navigate`. An explicit command `query` targets the companion view directly, while omitting `query` in a toggle command maintains live tracking with the primary View's state.

## Host Ownership

Request construction, binding resolution, pane geometry, and Omnibar ownership are shared:

- Picker input editing is owned by the Host Omnibar; Picker receives read-only editor snapshots and owns parameter parsing/item loading.
- Headerless and Picker primary Views use the same Host-computed pane rectangles; Companion and primary Resize events use those rectangles.
- Form field editing and Embedded PTY input remain engine-local and do not create a second Host editor.

The Host owns pane geometry and the Omnibar editing/rendering contract. It preserves Form field editing and Embedded PTY input as engine-local behavior and does not add a redundant input row or a window-management state machine.

Companion producer tasks use the background execution class regardless of engine. Ordinary foreground Views use the serial class. Opening a companion in the foreground creates a new instance with a captured source payload; it does not transfer an existing Form draft, scroll position, or PTY.

Capture never infers source context from JSON-looking strings or reserved-looking business fields. For a bound companion, the Host supplies source parameters, publication input, and source state explicitly; ordinary Capture launch input is passed through unchanged.

The focus handoff, Escape ladder, and focus-border sections of [ADR 0010](../adr/0010-companion-views-and-unified-host-layout-architecture.md) are historical design proposals, not current runtime contracts.
