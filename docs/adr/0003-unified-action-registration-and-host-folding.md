---
title: "ADR 0003: Unified Action Registration and Host-Owned Command Folding"
type: "concept"
tags:
  - adr
  - architecture
  - input
  - commands
  - rendering
description: "Register View and Engine actions through one dispatch model while keeping business command presentation and folding owned by the host."
---

# ADR 0003: Unified Action Registration and Host-Owned Command Folding

- **Status**: Accepted
- **Date**: 2026-09-12
- **Implementation status**: Implemented; the command palette is a built-in Popup Picker View routed through the normal View lifecycle.
- **Scope**: Action registration, key dispatch, Footer hints, command palette contents, and normal versus popup command presentation.
- **Related decisions**: [ADR 0002](0002-static-configuration-and-script-boundaries.md), [Input and Navigation Model](../explanation/input-and-navigation-model.md).

## Context

The current command layer uses several overlapping concepts and paths. Configured View commands, Engine keymaps, the built-in command palette binding, Footer overflow checks, and popup bottom-border rendering do not share one authoritative presentation model.

In particular, Form exposes Engine interactions such as Tab, Shift-Tab, and Escape as labeled `Binding` values. Picker usually exposes similar Engine interactions without labels. The Footer filters bindings by label, while the command palette collects configured commands independently. Normal Footers and popup borders also apply different overflow behavior. Some Engines use whether the Footer is currently overflowing as a condition for whether Ctrl-K can execute.

These implementation details make unrelated concerns affect one another:

- An Engine's choice to label an internal key changes business-command presentation.
- Footer width can change whether Ctrl-K is dispatched.
- Popup presentation can change which commands are displayed.
- The command palette can contain a different set of actions from the Footer without an explicit model explaining why.

The system needs one simple distinction: configured View commands are business actions; Engine commands are the interaction controls required by the selected Engine. Both must be dispatchable, but only View commands belong in business-command presentation.

## Decision

### 1. Register every action through one dispatch model

View commands and Engine commands are both registered in the active View's action registry. Registration records the action's source so the host can apply the correct behavior without inferring it from labels or implementation-specific keymaps.

The source distinction is:

```text
ViewCommand
EngineCommand
```

A View command comes from workflow configuration and has the configured command definition and invocation context. An Engine command is supplied by the selected Engine and is handled by that Engine. Engine editing, selection, back, close, and exit behavior remain Engine commands; they are not separate command-layer categories.

The registry used for dispatch contains both kinds of action. The registry used for business-command presentation is derived by filtering to `ViewCommand`.

### 2. Keep Engine commands out of business-command presentation

Engine commands are not included in either of these lists:

- the Footer's business-command hints;
- the command palette's selectable business commands.

This does not disable or unregister them. Engine commands remain directly dispatchable through their registered keys.

For a Form with one configured `Submit` command, the effective model is:

```text
Dispatch:
  Enter       -> ViewCommand(Submit)
  Tab         -> EngineCommand(NextField)
  Shift-Tab   -> EngineCommand(PreviousField)
  Escape      -> EngineCommand(Close)

Business presentation:
  Enter Submit
```

Whether an Engine command has a user-facing label must not decide whether it is registered or whether it works.

### 3. Compute folding once from View commands

The host computes one folding decision from the current View command set before rendering either a normal page or a popup. The initial rule is:

```text
fold = view_commands.len() > 2
       || view_commands.iter().any(command.key.is_none())
```

The exact threshold may be tuned later, but it is a host policy and must not be reimplemented by individual Engines or renderers.

When `fold` is false:

- no Ctrl-K command-palette binding is registered;
- all configured View commands are eligible for direct Footer presentation;
- Ctrl-K does not trigger because no Ctrl-K action exists in the registry.

When `fold` is true:

- the host keeps up to two directly bound View commands in the Footer;
- the host registers the Ctrl-K command-palette action as the final Footer entry;
- configured View commands omitted from the Footer remain directly dispatchable when they have keys;
- all configured View commands, including unbound commands, are available as selectable palette entries.

Thus three business commands are presented as two direct commands plus `Commands`. Folding does not mean hiding every direct shortcut; it limits the number shown in the Footer.

Folding is a presentation decision. It does not remove direct View-command bindings from the dispatch registry.

### 4. Treat the command palette opener as a host action

The command palette opener is a host action, but the palette itself is an ordinary built-in Popup Picker View. It is not a business command defined by the active View, and it is not an Engine command.

When folding is enabled, the host pushes the built-in command-picker View through the Router with a Popup presentation and passes the eligible View-command entries through the request parameters. The built-in View converts those parameters into standard Picker items and returns a `CommandRef` when the user selects one. The host then resolves and executes that command.

The built-in command-picker View uses the normal View lifecycle, Popup placement, Picker input, selection, rendering, and return flow. It must not include its own opener, Engine commands, or Footer-only presentation artifacts. Its configuration is part of the built-in View configuration set and is loaded before user workflow configuration so it can be extended or overridden under the normal merge rules.

This removes the need for a special `overflow_binding` to act as both a display marker and a dispatch gate. Ctrl-K exists only when the host's folding decision inserts it.

### 5. Share command preparation between normal and popup presentation

Normal Footer rendering and popup bottom-border rendering consume the same host-prepared command presentation result:

```text
View commands
Directly displayed commands
Whether folding is enabled
Whether the Ctrl-K opener exists
Palette entries
```

They may differ only in geometry and final drawing location. Popup presentation must not recalculate folding, select a different command source, or bypass the host's folding decision.

### 6. Keep scope as command ownership, not action kind

If configured commands need ownership metadata, `session` and `view` remain scopes of `ViewCommand`. Scope does not distinguish View commands from Engine commands. Engine actions are owned by the active Engine and do not need a session/view command scope.

## Consequences

### Positive

- One dispatch registry covers every key that can be handled by the active View.
- Business-command presentation no longer depends on whether an Engine adds labels to its internal bindings.
- Footer width, popup geometry, and command availability are independent.
- Ctrl-K behavior is deterministic: it exists exactly when folding is enabled.
- Normal and popup views share the same command set and folding decision.
- The command palette is a routed built-in Popup Picker View, not a Session-owned rendering path.
- The command palette contains business commands rather than low-level editing and navigation controls.

### Costs

- The action registry must retain the source of each registered action.
- Existing Engine-specific event paths need to be adapted to consume the common dispatch result.
- Existing tests that expect labeled Form Engine bindings or width-driven Ctrl-K behavior must be updated.
- Session/global configured commands need an explicit policy for whether they are included in the View-command presentation set.

## Rejected alternatives

### Use labels to decide presentation

Rejected because label presence is an incidental Engine implementation detail. Picker and Form would continue to expose different Footer behavior for equivalent internal actions.

### Let each renderer decide overflow independently

Rejected because normal Footers and popup borders would continue to disagree about whether the command palette exists and which commands are visible.

### Make Ctrl-K an always-available View command

Rejected because the command palette is a host capability, and the requested behavior is that Ctrl-K exists only when folding is active. The host should inject the action when the shared folding rule requires it.

### Use the Footer's current layout to gate dispatch

Rejected because hiding a command is a presentation decision. A directly bound View command must remain executable even when it is not visible in the Footer.

## Invariants

1. Every dispatchable View or Engine action is registered once with an explicit source.
2. Engine actions never enter the business-command Footer or palette collections.
3. The folding decision is computed once by the host from View commands.
4. Ctrl-K is registered if and only if folding is enabled.
5. Footer rendering and popup rendering consume the same prepared command presentation.
6. A View command with a key remains directly dispatchable even when omitted from the Footer.
7. The command palette never presents its own opener or Engine actions.
8. The command palette is mounted through the Router as a built-in Popup Picker View; Session does not maintain a parallel Picker state or renderer.
