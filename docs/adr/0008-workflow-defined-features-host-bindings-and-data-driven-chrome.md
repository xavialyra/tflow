---
title: "ADR 0008: Workflow-Defined Features, Host-Scoped Bindings, Unified Addressing, and Data-Driven Chrome Commands"
type: "concept"
tags:
  - adr
  - architecture
  - workflow
  - commands
  - bindings
  - addressing
  - host
  - chrome
description: "Replace built-in command palette and parameter editor features with workflow-defined implementations, protocol invocation, host-layer bindings, a unified key-centric binding model, layer-based unbinding, dot-delimited command addressing, and data-driven Chrome presentation."
---

# ADR 0008: Workflow-Defined Features, Host-Scoped Bindings, Unified Addressing, and Data-Driven Chrome Commands

- **Status**: Accepted
- **Date**: 2026-09-27
- **Scope**: Reusable workflow-defined command palette and parameter editor features, protocol command invocation (`invoke-command`), read-only command snapshot context, host-layer command bindings (`[host.bindings]`), unified key-centric binding syntax (`bindings`), axis-based unbinding (`[views.<name>.unbind]` with `keys`/`commands`/`layers`), dot-delimited command addressing (`workflow.command`), colon-delimited view routing (`workflow:view`), configuration flattening, elimination of compile-time extension injection, and key-driven Chrome command presentation.
- **Related decisions**: [ADR 0003](0003-unified-action-registration-and-host-folding.md), [ADR 0004](0004-scoped-command-registration.md), [ADR 0005](0005-manifest-driven-suites-and-self-contained-workflows.md), [ADR 0006](0006-feed-removal-workflow-scoped-commands-and-item-bindings.md).
- **Supersedes**: The command-palette-specific Host folding and command-ID-specific Footer selection portions of ADR 0003. ADR 0004's registry layers, priority, revision, and dispatch invariants remain in force.

## Context

`tflow` previously had two distinct and conflicting models for commands and user interaction:

1. **Ordinary workflow commands** declared with `run`, `navigate`, `call`, or `return` operations and backed by producers.
2. **Host-special built-ins** (`OpenCommands`, `OpenParameters`, `__commands`, `__query`) that bypassed standard workflow protocols with special result handling in protocol adapters and hardcoded footer recognition for `Enter` and `commands`.

An initial attempt to decentralize these built-ins introduced compile-time **workflow extensions** (`extension.toml`, `[extensions.<name>]` in suites). That design attempted to macro-expand commands and keybindings into all matching workflows during manifest compilation. However, this approach introduced severe architectural friction:

- **Entity multiplication**: users had to navigate three distinct concepts (`Workflow`, `Suite`, `Extension`), splitting a single feature (e.g. Command Palette) across a workflow UI, an extension manifest, inline glue scripts, and suite mounts.
- **Compensatory macro complexity**: because suites were artificially forbidden from declaring commands, the compiler had to inject commands into individual views using brittle positive and negative selectors (`apply_to = ["*", "!commands:*"]`) to avoid recursive self-nesting.
- **Broken workflow self-containment**: workflows acquired "ghost bindings" from compile-time AST injections, complicating local reasoning and conflict debugging.
- **Coupled command and binding definitions**: `command.key` baked physical keybindings into semantic command declarations, confusing behavior with input routing.
- **Inverted binding models**: `defaults.<engine>.bindings` mapped `action = [keys]`, while view keymaps mapped `key = action`, so the same table could be read in either direction.
- **Ambiguous addressing**: Views and Commands both used colons (`workflow:view` vs `workflow:command`), obscuring whether an identifier was a navigable route or an executable action.

The runtime already possesses an orthogonal, three-tier layer hierarchy defined in ADR 0004:

```text
View > Engine > Host
```

The system requires an architecture where workflow features are first-class, command definitions are pure, keybindings strictly map physical keys to commands, addressing is unambiguous, and host-level commands are handled natively through `BindingLayer::Host` without AST macro injection.

## Decision

### 1. Workflow features replace Host-special built-ins

The command palette and parameter editor are standard workflow implementations provided by the suite or workflow environment. The kernel contains no hardcoded command-ID actions or AST bypasses.

The execution model follows standard workflow navigation:

```text
Host or View Command (type = "call")
  -> pushes workflow-defined Picker or Form View
  -> handler/return processor produces standard protocol operation
```

The Host retains only the low-level execution primitives that workflow protocols cannot perform in isolation:

- supplying read-only command snapshot context;
- invoking a selected command reference (`invoke-command`);
- revision validation and dispatch;
- standard replacement navigation (`navigate` with `replace = true`).

### 2. Bundled fallback assets and open naming convention

To guarantee an out-of-the-box experience for standalone portable workflows (`-w`) and fresh suites (`-s`), standard auxiliary workflows and the base theme are embedded as **bundled fallback assets** in `assets/builtin/`:

```text
assets/builtin/
├── themes/
│   └── default.toml                  # Base Schema and Default Theme
└── workflows/
    ├── __commands/
    │   └── workflow.toml              # Fallback Command Palette
    └── __parameters/
        └── workflow.toml              # Fallback Parameter Editor
```

- **Convention over privilege**: The double underscore prefix `__` is an open naming convention for system utilities (`__commands`, `__parameters`), not a compiler keyword. Users may freely declare workflows with `__` or shadow the built-ins by providing custom implementations.
- **Fallback mounting**: If a suite or standalone execution does not define `__commands` or `__parameters`, the runtime automatically mounts the bundled fallbacks.

### 3. Unified command and view addressing grammar

To eliminate visual and syntactic ambiguity, `tflow` strictly separates the addressing of navigable routes (Views), executable actions (Commands), and architectural layers:

| Category | Delimiter / Sigil | Syntax Example | Meaning |
| :--- | :--- | :--- | :--- |
| **View (Route / Page)** | Colon (`:`) | `core:main`<br>`__commands:main` | Navigable surface or layout in a workflow |
| **Command (Action / Method)** | Dot (`.`) | `calculator.eval`<br>`__commands.palette`<br>`sys.quit` | Executable command / method in a workflow |
| **Local Command** | Bare word | `accept`<br>`open` | Local command of the current workflow |
| **Workflow Command (explicit owner)** | At-sigil (`@`) + colon | `@workflow:calculator.eval`<br>`@workflow:accept` | Workflow command with its processor named explicitly |
| **Engine Action** | At-sigil (`@`) + dot | `@engine:picker.toggle_preview`<br>`@engine:capture.copy` | Built-in engine action, addressed explicitly |

- **Colon (`:`) represents Location/Route**: Centered, balanced, and evoking an endpoint (`workflow:view`).
- **Dot (`.`) represents Action/Invocation**: Lightweight, evoking method execution (`workflow.command`).
- **Sigil (`@`) represents the Command Processor**: It names the owner/execution location of a command reference (`@workflow:` / `@engine:`), disambiguating it from a workflow identifier. It is **not** a layer: priority layers are not addresses and live in the dedicated `unbind.layers` field.
- **The dotted index is unique**: a command's identity is `<owner>.<name>` — `<workflow>.<command>` for a workflow command and `<engine>.<action>` for an engine action. A workflow command may not claim an id an engine action already owns (`form.exit`, `picker.clear_input`), so one FQID always names exactly one command; the collision is rejected at load time rather than resolved by priority. The sigil chooses the processor; it is never part of the identity.

### 4. `invoke-command` is a standard Host capability

A producer or return processor may return an `invoke-command` operation containing a validated command reference:

```json
{
  "version": 1,
  "operation": {
    "type": "invoke-command",
    "command": {
      "id": "apps.open",
      "revision": 42
    }
  }
}
```

The reference names the target by its FQID and the registry revision it was resolved against. It never carries the owning View: the origin is a dispatch detail, not part of the command's identity.

The Host resolves the reference against the current registry revision. Missing or stale references are safely rejected without executing stale callbacks. Command invocation is subject to a maximum call depth of 2.

### 5. `commands` context is a runtime snapshot

Workflow producers that present or select commands receive the same projection at `context.commands`:

```json
{
  "revision": 42,
  "commands": [
    {
      "id": "apps.open",
      "label": "Open",
      "key": "enter",
      "layer": "view"
    }
  ]
}
```

The revision describes the snapshot as a whole, so it lives on the envelope and never on an entry. A workflow selects a command by echoing its `id` back with that revision; because the entry id is the command's FQID, the same command carries one id wherever it is addressed.

Every published entry can be run from a command reference. An engine action is executed by the engine of the View that owns the entry — the same View whose key binding produced it — so a key press and a command reference are two ways of locating one entry, never two ways of executing it. A selector curates; it does not gate. The built-in palette lists the View and Host layers (what this workflow and the host offer) and leaves the Engine layer to Chrome hints, because the engine's own default keymap is not a command the workflow provides.

The snapshot is read-only. Workflows select references and return them; the Host executes the invocation.

### 6. Parameter editing uses ordinary navigation replacement

A workflow-defined parameter editor returns a standard `navigate` operation with `replace = true`:

```json
{
  "version": 1,
  "operation": {
    "type": "navigate",
    "target": "apps:main",
    "query": {
      "query": "new value"
    },
    "replace": true
  }
}
```

No specialized host result branches or parameter-editor-specific lifecycle hooks exist.

### 7. Abolish compile-time extensions; Workflows are self-contained packages

The `extension` concept, `extension.toml`, `[extensions.<name>]` manifests, and compile-time AST macro expansion (`expand_extensions`) are abolished.

A workflow is a self-contained capability package. It may provide:
- **Interactive applications**: declaring `entrypoint`, `[views]`, and `[commands]` (e.g. `apps`, `calc`).
- **Modal utility views**: declaring popup views and exporting callable commands (e.g. `__commands.palette`, `__parameters.edit`).
- **Headless command libraries**: declaring only `[commands]` and producer scripts without views.

A workflow is never mutated or injected into by external manifests. Its behavior is strictly determined by its own manifest and the ambient Host/Engine fallbacks.

### 8. Decouple commands from keybindings

The `key` field is removed from command definitions (`Command` struct and `[commands.<id>]`).

- **`[commands.<id>]`** defines pure semantic behavior: `label`, `type`, `handler`, `presentation`, and `return_processor`. Commands carry no physical keybinding information.
- **`bindings`** tables define physical input mappings from keystrokes to command identifiers.

Commands can be triggered via keyboard bindings, searched in the command palette without any shortcut, or invoked programmatically via `invoke-command`.

### 9. Unified key-centric binding model (`bindings`) and Scope Unbinding (`unbind`)

The legacy term `keymap` is deprecated in favor of `bindings` across all configuration layers.

All binding tables adopt a **key-centric mapping** where the physical key is the TOML map key:

```toml
[views.main.bindings]
"enter" = "accept"
"down" = "@engine:picker.select_next"
```

1. **TOML Parse-Time Conflict Prevention**: Duplicate bindings within the same layer fail immediately during TOML parsing.
2. **Multiple Keys per Command**: Supported cleanly by mapping multiple keys to the same command identifier:
   ```toml
   "down" = "@engine:picker.select_next"
   "ctrl+j" = "@engine:picker.select_next"
   ```
3. **Engine Actions are Indexed, but Addressed Explicitly**: Built-in engine actions (`exit`, `back`, `select_next`, `toggle_preview`, `focus_next`, `cancel`, `copy`) live in the unified command index at `BindingLayer::Engine`, keyed by `<engine>.<action>`. A View binding reaches one with the `@engine:<engine>.<action>` address; a bare value never resolves to an engine action, so `"escape" = "exit"` means the current workflow's `exit` command, not `picker.exit`. Engine-layer defaults (`[picker.bindings]`, `[form.bindings]`, …) keep the bare action names because they are the engine's own namespace.
4. **Unbinding by Axis (`unbind`)**:
   Unbinding is a per-View table whose fields map to three independent axes — a physical key, a command address, and a priority layer are not the same kind of thing, so they do not share one sigil-overloaded string list:
   ```toml
   [views.main.unbind]
   keys = ["ctrl+u"]
   commands = ["@engine:picker.clear_input", "core.page"]
   layers = ["host"]
   ```
   - `keys` unbinds one physical key; `commands` unbinds one command by its address (the unique index), removing all of its keys; `layers` drops an entire priority layer in this View.
   - `layers = ["host"]` cleanly unbinds the whole host layer in this view without hardcoding external workflow FQIDs or guessing user key remappings.
   - Modal views (such as `__commands:main` and `__parameters:main`) declare `layers = ["host"]` to prevent host shortcuts (`ctrl+k`, `ctrl+g`) from causing self-referential nesting or modal interruption.
   - Single-key removal is expressed with `keys = ["escape"]`; a View binding always names a command, so a key is never claimed by a bare boolean.

### 10. Direct alignment with 3-tier Scope Hierarchy and Configuration Flattening

Binding configuration maps 1:1 to the runtime layer hierarchy (`View > Engine > Host`):

#### Host Scope: `[host.bindings]`
Configures session-wide global shortcuts in `settings.toml` or `suite.toml`. Values use dot-delimited command FQIDs (`workflow.command`):

```toml
# suite.toml or settings.toml
[host.bindings]
"ctrl+k" = "__commands.palette"
"ctrl+g" = "__parameters.edit"
```

#### Engine Scope: Flattened `<engine>.bindings`
Configures engine-level defaults in `settings.toml`. The redundant `defaults.` prefix is eliminated:

```toml
# settings.toml
[picker.bindings]
"ctrl+c" = "exit"
"escape" = "back"
"up" = "select_previous"
"down" = "select_next"
"ctrl+p" = "toggle_preview"

[form.bindings]
"tab" = "focus_next"
"backtab" = "focus_prev"
"escape" = "cancel"

[capture.bindings]
"enter" = "copy"
"escape" = "back"

[embedded.bindings]
"escape" = "cancel"
```

#### View Scope: `[views.<name>.bindings]` and `[views.<name>.unbind]`
Configures view-specific business bindings and unbinding rules in `workflow.toml`:

```toml
# workflow.toml
[views.main.bindings]
"enter" = "open"
"ctrl+o" = "open_detached"
"ctrl+y" = "copy_path"

[views.main.unbind]
keys = ["escape"]
layers = ["host"]
```

### 11. Chrome command display is key-driven and data-driven

Chrome command presentation in the Footer does not inspect command IDs or hardcode special branches.

Global configuration selects which physical bindings are displayed:

```toml
chrome_commands_show = ["enter", "ctrl+k"]
```

Views may replace the inherited list:

```toml
[views.main]
chrome_commands_show = ["enter", "ctrl+y", "ctrl+k"]
```

The Footer resolves each configured physical key against the current `CommandRegistry` revision using `View > Engine > Host` precedence. If an active binding exists, its command label is displayed; otherwise, the hint is omitted. `unbind` removes the key-to-command association entirely, so an unbound key exposes no binding and likewise produces no hint.

### 12. Dual-Mode CLI Execution Preserved

The execution modes of the CLI remain cleanly separated:

- **`-s, --suite <PATH>`**: Multi-workflow workspace orchestration. Mounts member workflows, sets session entrypoints, and configures `[host.bindings]`. Omission defaults to `default.toml`.
- **`-w, --workflow <PATH>`**: Standalone, single-file or single-directory workflow runner. Executes ad-hoc, portable workflows (including standard input pipelines `cat flow.toml | tflow -w -`) without suite dependencies.
- **`[VIEW]`**: Positional entrypoint override specifying the target view (e.g. `tflow git:branch` or `tflow -w tool.toml preview`).

## Consequences

### Positive

- **Architectural symmetry**: `host.bindings`, `<engine>.bindings`, and `views.<name>.bindings` strictly correspond to `Host`, `Engine`, and `View` runtime layers.
- **Unambiguous addressing**: Views use colon paths (`workflow:view`), commands use dot methods (`workflow.command`), and a command reference names its processor with an at-sigil (`@workflow:` / `@engine:`) — priority layers are never written as addresses.
- **Axis-based unbinding**: `[views.<name>.unbind]` separates physical keys, command addresses, and priority layers, so `layers = ["host"]` cleanly isolates modal popups without leaking orchestration FQIDs or breaking under user key remappings.
- **Zero compile-time AST mutation**: Workflows remain completely self-contained; no ghost bindings or negative selectors.
- **Unified syntax**: Terminology is unified to `bindings`, and all layers use `"key" = "target_command"`.
- **Pure command definitions**: `[commands.<id>]` contains no keybinding details; commands can be triggered by shortcuts, search, or protocol invocation.
- **Clean configuration**: Removing `defaults.` eliminates unnecessary nesting in `settings.toml`.
- **Independent utility workflows**: The command palette and parameter editor are standard workflows that can be customized, swapped, or omitted.

### Costs

- `settings.toml`, `suite.toml`, and `workflow.toml` manifests require migration to rename `keymap` to `bindings`, rename `keymap_mode` to `binding_mode`, and remove `defaults.` prefixes.
- `Command` struct loses `pub key: Option<String>`, requiring loader and validation adjustments.
- Built-in engine actions must be formally enumerated as internal command identifiers.
- Command FQID parsing must accept dot notation (`workflow.command`).

## Rejected Alternatives

### Compile-time extension macro injection (`[extensions]`)

Rejected. Injecting commands and keymaps into target workflows at compile time created fragile selector logic, broke workflow self-containment, and required complex override policies. Host-level commands naturally belong in `BindingLayer::Host`.

### Colon for both views and commands (`workflow:view` and `workflow:command`)

Rejected. Overloading the colon delimiter creates visual ambiguity in logs, UI headers, and error messages, and makes it impossible to distinguish between navigating to a route and executing an action.

### Slash delimiter for views (`workflow/view`)

Rejected. In terminal monospaced typography, slashes create an aggressive visual tilt and strong "filesystem path" connotation (`/usr/bin`), degrading the aesthetic presentation of UI routes and breadcrumbs.

### Hardcoding external FQIDs in view unbinding (`unbind = ["__commands:palette"]`)

Rejected. Forcing views to specify external package FQIDs leaks orchestration-layer details into self-contained workflows, creating tight coupling that breaks when workflows are renamed or executed in standalone mode.

### Action-centric binding model (`action = [keys]`)

Rejected for view and host bindings. Key-centric mapping (`"key" = "command"`) ensures parse-time conflict prevention via TOML duplicate key rules and matches physical keyboard input dispatch.

### Baking physical keys into command declarations (`command.key`)

Rejected. Coupling keys to commands prevents reusing the same command across different shortcuts, confuses palette search availability with shortcut assignment, and conflates semantic behavior with input routing.

## Invariants

1. The command palette and parameter editor are standard workflow implementations, not built-in kernel actions.
2. View routes strictly use colon notation (`workflow:view`); command FQIDs strictly use dot notation (`workflow.command`).
3. Command references name their processor with the `@` sigil (`@workflow:` / `@engine:`). Priority layers are not addresses; they are targeted through the `unbind.layers` field.
4. `BindingLayer::Host` is the sole mechanism for suite-level and ambient global command resolution.
5. `Command` declarations are strictly semantic and contain no physical keybinding definitions.
6. All keybinding tables use `bindings` and the `"key" = "command_id"` mapping syntax.
7. Workflows are self-contained; no external manifest may inject, mutate, or expand a workflow's AST.
8. Engine actions are registered as Engine-layer internal commands.
9. Footer command presentation is strictly key-driven via `chrome_commands_show` and resolves through `View > Engine > Host` precedence without special command-ID recognition.
