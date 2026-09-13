---
title: "ADR 0004: Scoped Command Registration"
type: "concept"
tags:
  - adr
  - architecture
  - commands
  - input
  - registration
description: "Define command registration as a small scoped input registry whose callbacks retain their own context and whose dispatch is independent of workflow context resolution."
---

# ADR 0004: Scoped Command Registration

- **Status**: Accepted
- **Date**: 2026-09-13
- **Scope**: Command registration, lifecycle, key resolution, callback ownership, and View/Engine switching.
- **Related decisions**: [ADR 0003](0003-unified-action-registration-and-host-folding.md)

## Context

The command path had grown a second responsibility: resolving execution context after a key had already selected a command. Command bindings produced invocations, the Session located a source View, a View supplied a snapshot, and a protocol command service reconstructed page and owner context before preparing an operation.

This made command dispatch depend on Router state, View snapshots, command origins, owner contexts, projections, and several intermediate request types. It also made View switching appear to be a command-context transition instead of a simple replacement of the commands belonging to the active scope.

The command subsystem only needs to answer two questions:

1. Which registered command owns this input?
2. What callback should be invoked for that command?

The object that registers a callback owns the context required by that callback. The registry does not need to understand or reconstruct that context.

## Decision

### 1. Use one scoped command registry

Commands are registered dynamically in one registry. The registry stores commands grouped by one concept: `CommandScope`. `layer` and `scope` are not separate concepts.

The initial scopes are:

```text
Host
Engine
View
```

A separate dynamic scope is not required. Commands whose availability depends on runtime state are registered or removed by the component that owns that state, using the appropriate existing scope.

Each scope has an explicit priority. Key resolution searches scopes from highest to lowest priority. Registration order is not used as an implicit priority rule.

```text
View > Engine > Host
```

The exact numeric representation is an implementation detail; the precedence is part of the contract. A higher-priority registration shadows a lower-priority registration for the same physical key.

### 2. Store registrations, not reconstructed command contexts

A registered command contains stable metadata and an implementation handler:

```text
RegisteredCommand:
  id
  description
  binding
  scope
  generation
  enabled_query
  handler
```

`CommandId`, `CommandRef`, and `RegistrationHandle` are the stable references shared by the registry, Footer, Palette, and dispatcher. A `CommandRef` includes the scope generation and becomes invalid when that scope is replaced. The handler may be a callback internally, but callers must not retain or invoke a bare callback.

The registry does not store `ViewContext`, `CommandContext`, `CommandOwnerContext`, Router references, Engine snapshots, or Session state.

The callback, or the object captured by the callback, owns the context needed to perform its work. Host callbacks retain Host-owned state; Engine callbacks retain Engine-owned state; View callbacks retain View-owned state.

### 3. Registration is the lifecycle operation

Components register commands when they become active:

```text
Session startup  → register Host commands
Engine mount     → register Engine commands
View mount       → register View commands
```

Components remove or replace their registrations when they become inactive.

View switching therefore has a direct lifecycle meaning:

```text
retain Host commands
retain Engine commands if the Engine remains active
replace View commands
```

When the Engine changes:

```text
retain Host commands
replace Engine commands
replace View commands
```

A registration handle or scope generation must be used for removal so an old View cannot remove a newer View's command after a transition.

### 4. Input dispatch resolves and invokes directly

The input path is:

```text
InputEvent::Key
  → CommandRegistry::resolve(key)
  → RegisteredCommand::callback
  → CommandOutcome
```

The command registry does not participate after command selection. It does not prepare workflow operations, resolve owners, inspect View snapshots, or convert commands into Router decisions. Dispatch validates the command reference generation and enabled state before invoking the handler.

The input layer handles an unregistered key as unhandled. A registered command is invoked immediately. A disabled command is consumed without invoking its callback.

### 5. Callbacks return a small outcome protocol

Callbacks may complete locally or ask the Host to perform an application-level operation:

```text
CommandOutcome:
  Consumed
  Request(HostRequest)
```

`HostRequest` may contain navigation, call, return, external execution, palette opening, or exit requests. The Host interprets these requests and delegates to Router or infrastructure services.

A callback does not directly mutate Router or Session. It may mutate the state of the component that registered it, such as an Engine's selection state. Cross-component work is expressed through `HostRequest` or a task/effect request; process execution and scheduling remain owned by `execution` and `task`.

### 6. Registration conflicts and replacement are deterministic

Conflicts between scopes use explicit scope priority. A conflict within one scope is rejected during registration and reported as a diagnostic; registration order is never an implicit override rule.

Scope replacement installs one new generation and invalidates the old generation. Removal is conditional on its registration handle, so an old View or Engine cannot remove a newer registration. Footer and Palette consume the same metadata and enabled-state query as dispatch, and retained references are revalidated before execution.

### 7. Host commands use the same path

Host commands are ordinary registrations in `Host` scope. They do not use a separate command execution path.

For example, the command palette opener is registered by Host and returns a Host request. Host then obtains the currently registered View commands for presentation and opens the built-in palette View. Selecting an entry invokes the still-valid registration through the normal callback path.

The palette must reject or refresh a selection whose registration handle or scope generation is no longer valid.

## Consequences

### Positive

- Command lookup is a direct key-to-callback operation.
- Command registration and command context ownership are separate responsibilities.
- View and Engine switching is explicit scope replacement.
- Engine commands can remain active when only the View changes.
- Host, Engine, and View commands share one precedence and dispatch mechanism.
- The registry does not depend on Router, Session, View snapshots, or workflow execution details.
- Dynamic command projection and a second dynamic scope are unnecessary.
- Command palette, Footer, and input dispatch can derive their entries from the same registrations.

### Costs

- Callbacks need a safe lifetime strategy when they capture View or Engine state.
- Workflow command preparation must move behind the callback or a View-owned command adapter.
- Registration handles or scope generations are required for safe replacement.
- Existing `CommandInvocation`, `CommandRequest`, and context reconstruction paths need migration.

## Rejected alternatives

### Maintain separate layers and scopes

Rejected because both concepts describe grouping, lifecycle, and precedence. Keeping both would require an additional rule explaining how they interact without adding domain value.

### Reconstruct context in Session after dispatch

Rejected because the registering component already owns the context needed by its callback. Reconstructing it from Router and View snapshots couples the registry to application orchestration.

### Maintain a global current command context

Rejected because commands belong to different lifecycle owners. A global context becomes a shared mutable object containing unrelated Host, Engine, and View state.

### Add a Dynamic scope

Rejected because runtime-dependent commands can be installed, replaced, or removed by their owning Host, Engine, or View scope. A fourth scope would encode a state transition as a second classification system.

## Invariants

1. `CommandScope` is the only grouping concept in the registry.
2. Scope precedence is explicit and deterministic.
3. Every active command is registered exactly once in its owning scope.
4. The registering component owns the callback context.
5. The registry only resolves keys, exposes descriptions, and manages registration lifetimes.
6. View replacement replaces View registrations without requiring an Engine replacement.
7. Engine replacement replaces Engine registrations and any dependent View registrations.
8. A stale registration cannot remove or invoke a newer registration for the same scope.
9. A `CommandRef` is invalid after its scope generation is replaced.
10. Same-scope key conflicts fail deterministically.
11. Footer, Palette, and dispatch use the same metadata and enabled-state decision.
12. Host, Engine, and View commands use the same input dispatch path.
10. Cross-component effects are returned as Host requests and interpreted outside the registry.
