---
title: "ADR 0004: Scoped Command Registration"
type: "concept"
tags:
  - adr
  - architecture
  - commands
  - input
  - registration
description: "Define scoped command registration and the ordinary navigation and return flow for command selection."
---

# ADR 0004: Scoped Command Registration

- **Status**: Accepted
- **Date**: 2026-09-13
- **Scope**: The current command set, scope lifecycles, input resolution, Footer, command selection, navigation return values, and command execution requests.

## Decision Summary

`CommandRegistry` is the single source of truth for the currently effective commands. Input resolution, Footer rendering, and command selection use the same registry entries and revision; they do not maintain separate command lists or reconstruct commands from different contexts.

The registry has exactly three scopes: `Host`, `Engine`, and `View`. Scopes express priority only. They do not express history, caller, origin, or the Router stack. The fixed priority order is:

```text
View > Engine > Host
```

Three responsibilities are deliberately separate:

1. **Navigation caller**: the View immediately below a pushed View on the Router stack determines who receives that View's result. The Router stack supplies the return target; the Host does not save caller state.
2. **Command registration**: the component that owns the lifecycle of a command registers it in the `Host`, `Engine`, or `View` scope. The Host registers fixed Host commands; the Engine and current View supply their current dynamic commands.
3. **Command execution**: the callback or its invoking caller completes execution according to the returned contract. Navigation carries results; it does not execute commands.

A normal navigation result carries an explicit return contract. For example:

```text
NavigationRequest.return_contract = Normal | CommandSelection
```

A `CommandSelection` result identifies a selected command reference and the registry revision against which it was selected. The Host is the caller of the command panel: it registers `Ctrl-K`, reads the current registry entries, pushes the command picker, and, when the returned contract is `CommandSelection`, dispatches the current reference. The Host does not retain the panel's caller. Other Popups return `Normal` results to their respective immediate callers, which handle those results according to their own View contracts.

The command picker is the built-in `__selectors:commands` ordinary View with popup presentation. It reads its query, applies ordinary Picker selection, and returns an ordinary `ViewResult` through the Router. Parameter editing is the separate built-in `__selectors:form` native Form View. Neither built-in View accesses the registry, registers commands, or executes commands.

## Why This Design Is Needed

The command set changes with the Engine, View, current item, and query-related state. If input, the Footer, and the command picker each retain a projection, the application can display one command while resolving another key to a different command, leak commands from an old View, or restore historical commands after returning from a Popup.

These failures conflate command availability, navigation ownership, and execution. Registration defines the current commands and their execution references. The Router defines where a View result goes. The callback or its invoking caller performs execution. No one component needs to infer the original caller from closed state, command ownership, or historical Views.

## Core Model

### Registry and scopes

The registry contains only three ordered scopes:

```text
Host    Fixed Host-level commands, such as Open Command Panel
Engine  Commands supplied by the current Engine for the current View/current item
View    More specific commands belonging to the current View
```

Scope priority is the only conflict rule. A higher-priority scope with the same binding overrides a lower-priority scope. Conflicts within one scope must be rejected or reported deterministically. A scope must not retain a replaced set, commands from a previous View, call history, caller, closed View, owner, or commands belonging to another View.

Each scope submission produces a monotonically increasing registry `revision` (or equivalent scope generation). `replace_scope(scope, entries)` is one commit: validation and conflict checks complete against a temporary set, then one indivisible state transition replaces the entire scope and publishes `CommandsChanged` with the new revision. If the commit fails, the previously committed set remains effective.

A registry entry contains at least a stable command ID, label/description, binding, scope, and the command reference or callback required for execution. A reference is valid only while its entry belongs to the current revision. The registry does not retain historical sets or reconstruct context from the Router, Session, View snapshot, or workflow owner. The registry does not understand navigation or return contracts.

### Current set and single snapshot

Chrome maintains one command snapshot containing the registry's current revision and the entries resolved at that revision. When Chrome receives `CommandsChanged`, it reads the complete registry state for that revision and replaces the snapshot; it does not merge new and old entries item by item.

The Footer and command-panel opening read their display data from this snapshot. If an implementation reads the registry directly, it must read the same revision and entries. Footer rendering, panel rendering, and input `resolve` therefore share the same command ID, label, binding, scope, and validity.

Input resolution must agree with presentation: for the same registry revision and key, the binding displayed by the Footer or picker must resolve to the same entry as `resolve(key)`. Before dispatching, input handling verifies that the entry still belongs to the current revision. If the revision changed, it discards the stale reference and resolves again against the current registry or snapshot.

## Registration Responsibilities and Lifecycles

### Host

The Host registers fixed Host commands during session initialization. `Ctrl-K` (or the configured command-panel binding) is a Host command registered by the Host. It produces a `NavigationRequest` that pushes `__selectors:commands` with the current Chrome snapshot/query. It is not a Popup command and not a `command_picker` special case. The hidden `Ctrl-G` parameter command similarly calls `__selectors:form` with the active View's query definition and current values, including its raw input; form submission replaces that View after its query schema is applied.

The Host is the command panel's navigation caller. When the picker returns with `return_contract = CommandSelection`, the Router delivers that result to the Host because the Host View is immediately below the picker. The Host validates the selected reference against the current registry revision and dispatches it. The Host does not save a caller field, panel state, or return target. The Router stack supplies the target.

Host commands remain effective while the Engine or View is replaced, until the session ends or the Host explicitly replaces the Host scope.

### Engine and View

The Engine computes the complete command set required by the current View and current item for the Engine and View scopes. A View may provide inputs for its View scope, or the Engine may submit them on the View's behalf, but activation and replacement must have the same complete-set semantics.

After a View or current item change, recompute the complete set, compare it with the committed set, and atomically replace only an affected scope when it changed. When the Engine changes, replace the Engine scope and clear or resubmit dependent View commands in the same lifecycle transition. When the View changes, replace the View scope with the new View's complete set. Commit an empty View scope when the new View has no commands, so old commands disappear explicitly. Neither component retains historical commands or caller state.

## Navigation and Return Contracts

Navigation and command execution use different contracts. A pushed View returns a `ViewResult` through the Router to the immediate caller below it on the stack. The Router routes and propagates that value; it does not interpret commands, inspect the registry, dispatch references, or choose an execution owner.

`Normal` is the default contract for ordinary Popup results. The calling View handles the returned value according to its own navigation protocol. `CommandSelection` is used only when the Host intentionally pushes the command picker. It carries the selected command ID/reference and the selection revision. The Host then performs revision validation and dispatch. A normal Popup does not become a command caller merely because it uses popup presentation.

A Popup is an ordinary View plus presentation. It reads only the query supplied to it, maintains ordinary Picker selection, returns the current selection on Enter, and returns/closes on Return according to the normal View protocol. It never reads the registry, registers commands, or executes commands. Presentation changes layout only.

## Data Flow

```text
Host initialization
  -> Host registers fixed Host commands, including Ctrl-K

Current View/current item changes
  -> Engine/View computes complete Engine/View command sets
  -> Atomically replace affected scope(s), when changed
  -> registry revision += 1
  -> CommandsChanged(revision)
  -> Chrome replaces its single snapshot
       ├─ Footer reads the snapshot
       └─ Host reads the snapshot to build the command-picker query

Ctrl-K
  -> Host command resolves from the current snapshot/registry
  -> Host creates NavigationRequest(return_contract = CommandSelection)
  -> Router pushes ordinary command-picker View with query and snapshot data
  -> Router stack records Host as the immediate return target

Command picker
  -> Reads query and uses ordinary Picker selection
  -> Enter returns ViewResult(contract = CommandSelection, selection, revision)
  -> Return/close returns the ordinary result required by its navigation contract
  -> Router propagates the result to the View below it

CommandSelection at Host
  -> Host validates the selected revision/reference against the current registry
  -> Host dispatches the current command callback/reference
  -> Callback/invoking caller completes execution according to its return contract

Normal result from any other Popup
  -> Router returns it to that Popup's immediate caller
  -> That caller handles the result according to its own View contract
```

The Router never understands commands; it only maintains the stack and propagates return values. The registry never understands navigation; it only maintains current entries, resolves by scope priority, and validates references for dispatch.

## Strictly Forbidden Legacy Models

The implementation must not reintroduce any of the following:

- A Host-wide result handler that receives or interprets every Popup's return; each result goes to the immediate caller below that Popup on the Router stack.
- Host-saved caller state, return targets, or `caller`/`closed` inference; the Router stack determines the return target.
- Popup access to `CommandRegistry`, registry entries, registry revisions, registration APIs, producers, callbacks, or command execution.
- `owner` or `replace_owner`, historical scopes, or any cache that restores old View commands.
- Legacy `bindings`/`projection`, separate Footer/input/picker command lists, or a `command_picker` lifecycle branch.
- An `enabled_query` that can make display and resolution disagree; presence in the current registry revision determines availability.
- Router command dispatch, registry lookups, command filtering, or interpretation of `CommandSelection`.
- Registry navigation requests, Router-stack knowledge, or return-target selection.

## Implementation Order

1. Define `CommandScope::{Host, Engine, View}`, entries, stable references, registry revision, and `CommandsChanged`. Keep the registry independent of navigation.
2. Implement priority-based `resolve`, same-scope conflict validation, atomic `replace_scope`, stale-reference checks, and deterministic removal.
3. Define `NavigationRequest`, `ViewResult`, and `return_contract = Normal | CommandSelection`; make Router push/pop and result propagation target the immediate View below the pushed View.
4. Connect Host initialization to registration of fixed commands, especially `Ctrl-K`, and make that command push the ordinary picker with the Host as the stack-provided return target.
5. Connect Engine/View lifecycles to complete-set computation, atomic scope replacement, dependent-scope invalidation, and empty-scope commits.
6. Publish `CommandsChanged(revision)`, make Chrome maintain one snapshot, and make Footer, input, and Host's picker query use it.
7. Implement the picker as an ordinary Popup View with ordinary query, selection, Enter, Return, and `ViewResult` behavior. Remove registry access, registration, execution, producer use, and special picker lifecycle handling.
8. Implement Host handling only for `CommandSelection`: validate the current revision and dispatch the current reference. Verify that all other Popup results return to their own immediate callers with `Normal` semantics.
9. Remove legacy owner, projection, caller/closed inference, enabled-query, and command-aware Router paths; run the contract and integration suites.

## Atomic Replacement and Lifecycle Contract

- The registry transitions only from one validated snapshot to one validated snapshot; partial command sets are never observable.
- `CommandsChanged` is published after commit and includes enough revision information for Chrome to reject stale or duplicate notifications.
- Chrome replaces its complete snapshot before Footer or Host picker-query construction reads it.
- A failed replacement leaves the prior committed set effective.
- Replacing the Engine invalidates dependent View entries; replacing the View leaves no entries from the old View.
- The Host scope remains during ordinary View and Engine lifecycles.
- An old reference cannot remove entries from a new revision or execute an old callback.
- Pushing and returning from a Popup are ordinary navigation transactions. The Router retains the return target on its stack, while the registry continues maintaining the current command set.
- Only the Host handles `CommandSelection` from the command picker. A `Normal` result is delivered to and handled by that Popup's immediate caller.

## Test Acceptance Matrix

| Scenario | Required result |
|---|---|
| Same binding in all three scopes | Deterministic `View > Engine > Host` resolution; same-scope conflicts reject the commit. |
| Host initialization | Host registers `Ctrl-K`; normal key dispatch triggers an ordinary picker navigation request. |
| Router return target | A pushed View result is delivered to the immediate View below it; Host state is not required to identify the target. |
| View switch | Host remains; View scope becomes the new View's complete set; all old View commands disappear. |
| Engine switch | Old Engine commands and dependent View commands become invalid atomically; the new set is committed once. |
| Current item change | Engine recomputes the complete set; only an actual set change replaces a scope and increments the revision. |
| Empty command set | Old commands cannot resolve, display, or dispatch after an empty replacement. |
| Atomic failure | Conflict or validation failure leaves the old set unchanged and publishes no spurious change. |
| Revision/out-of-order notifications | Chrome accepts only the current complete revision; an old notification cannot roll the snapshot back. |
| Footer and input | Every Footer binding resolves to the same entry at the same revision. |
| Panel data | Host builds the picker query from Chrome's single snapshot; no Popup registry read occurs. |
| Popup structure | The picker is an ordinary View with popup presentation and ordinary query/selection/Enter/Return behavior. |
| Popup forbidden dependencies | No Popup accesses the registry, registers commands, uses a producer, or executes commands. |
| Command selection return | The picker returns `CommandSelection` through Router; Host validates the current revision and dispatches the current reference. |
| Other Popup return | The Popup returns `Normal`; Router delivers it to that Popup's immediate caller, which handles it locally. |
| Closure and replacement | Cleanup for an old View/Engine cannot remove entries from a new revision; an old reference cannot execute. |
| Concurrent changes | A selection made before a registry change is rejected or refreshed by Host; no historical set is restored. |

## Stop When Something Smells Wrong

Stop and return to this ADR when implementation requires any of the following:

- Saving a caller or making Host handle every Popup result instead of using the Router stack;
- Making the Router inspect or dispatch commands, or making the registry understand navigation;
- Giving a Popup registry access, registration authority, or execution logic;
- Making old View commands reappear by retaining historical entries, an owner, or closed state;
- Adding a separate Footer/input/picker projection, `command_picker` protocol, or enabled query;
- Replacing only part of a scope or exposing an intermediate command set;
- Weakening revision validation or calling an old callback;
- Producing different commands for the same key and revision in resolve, Footer, or picker data.

These signals mean that navigation, registration, and execution responsibilities have been combined. Restore the single registry data flow, Router-stack return routing, and explicit return contracts before addressing local API details.

## Rejected Alternatives

### Multiple bindings or projections

Rejected. They create multiple sources for current commands and cannot guarantee consistency between Footer, picker, and input.

### owner/replace_owner or historical scopes

Rejected. Scopes represent priority only; history and return targets belong to navigation lifecycle management, not the registry.

### Popup accessing the registry and executing commands directly

Rejected. The Popup is an ordinary selection View. The Host receives its `CommandSelection` result because it called that picker, while other callers receive their own Popup's normal results through the Router.

### A Host-wide Popup result handler or saved caller field

Rejected. The Router stack already identifies the immediate caller and propagates each result to it.

### A command-aware Router or navigation-aware registry

Rejected. The Router transports return values, and the registry maintains current commands. Dispatch belongs to the callback or invoking caller under the explicit return contract.

### A dedicated `command_picker` View protocol

Rejected. The command panel uses ordinary navigation and Picker behavior with an explicit `CommandSelection` return contract.

### An independent enabled query

Rejected. Current availability is determined by entries in the current registry revision; a separate enabled decision would recreate divergence.

## Final Invariants

1. `CommandRegistry` is the single source of truth for currently effective commands.
2. The registry has only Host, Engine, and View scopes, and scopes manage priority only.
3. Scopes do not retain historical sets, old View commands, caller, owner, or closed state.
4. The Host registers fixed Host commands, including `Ctrl-K`; the Engine/View register current dynamic commands.
5. Navigation caller identity comes from the Router stack's immediate View below the pushed View.
6. `NavigationRequest.return_contract` explicitly distinguishes `Normal` from `CommandSelection`.
7. The Router only pushes Views and propagates `ViewResult` values; it does not understand or dispatch commands.
8. The registry only maintains and resolves entries; it does not understand navigation.
9. Every effective replacement is atomic and publishes `CommandsChanged` with a revision.
10. Chrome maintains one current snapshot; Footer, input, and Host picker-query construction use that current data.
11. A Popup is an ordinary View plus presentation and uses ordinary query, Picker selection, Enter, and Return behavior.
12. A Popup never accesses the registry, registers commands, or executes commands.
13. The Host dispatches only a returned `CommandSelection` from the picker after revision validation; callbacks/invoking callers complete execution under the return contract.
14. Other Popups return `Normal` results to their own immediate callers through Router.
15. `owner`, `replace_owner`, legacy bindings/projection, `command_picker` special handling, caller/closed inference, and `enabled_query` are not part of the implementation.
