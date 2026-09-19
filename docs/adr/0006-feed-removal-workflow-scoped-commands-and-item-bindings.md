---
title: "ADR 0006: Feed Removal, Workflow-Scoped Commands, and Item-Driven Bindings"
type: "concept"
tags:
  - adr
  - architecture
  - workflow
  - commands
  - picker
  - feeds
  - suite
description: "Abolish view-level feeds and command projection in favor of workflow-scoped commands, explicit view binding strategies, headless query/inspect decomposition, and item-driven binding processors."
---

# ADR 0006: Feed Removal, Workflow-Scoped Commands, and Item-Driven Bindings

- **Status**: Accepted
- **Date**: 2026-09-19
- **Scope**: `workflow/config`, `engine/picker`, `command/registry`, `protocol/contracts`, `app/cli`, `suite` manifest entrypoint.
- **Related decisions**: [ADR 0002](0002-static-configuration-and-script-boundaries.md), [ADR 0004](0004-scoped-command-registration.md), [ADR 0005](0005-manifest-driven-suites-and-self-contained-workflows.md).
- **Supersedes**: ADR 0002 Picker feed aggregation and command projection contracts.

---

## Context

In ADR 0005, `tlaunch` established a flat, two-tier architecture separating orchestration suites (`[suite]`) from atomic workflows (`[workflow]`). However, legacy mechanisms rooted in ADR 0002's Picker feeds specification (`[[views.<name>.engine.config.feeds]]`) remained in the system, introducing severe architectural contradictions:

1. **Leaky Self-Containment and Dependency Inversion**:
   In the test fixtures and existing configurations, the `core` workflow hardcoded cross-workflow references:
   ```toml
   [[views.default.engine.config.feeds]]
   view = "calculator:main"
   [[views.default.engine.config.feeds]]
   view = "apps:main"
   ```
   Executing `core` standalone (`tlaunch -w core/workflow.toml`) crashed during compilation because `calculator:main` and `apps:main` were absent. An atomic workflow was coerced into behaving as an asymmetric "God workflow", directly violating the zero-coupling chip invariant of ADR 0005.

2. **Conflation of View Commands and Item Actions**:
   Business actions targeting individual items (e.g., launching an application, copying a calculation result) were erroneously declared as properties of the *View* (`[views.main.commands.*]`). To support heterogeneous items in aggregate views, the host had to perform internal **Command Projection**: tracking each item's hidden provenance (`snapshot.owner_view`), extracting commands from the owner View, and synthesizing parameter snapshots. This created high-complexity state synchronization and black-box context manipulation.

3. **Inability to Support Custom Aggregation Logic**:
   Because aggregation was hardwired inside the Picker engine via `feeds`, custom ranking, trigger sigils (e.g., `=` for math, `>` for shell commands), and dynamic filtering could not be implemented without continuously bloating the host C++/Rust engine with ad-hoc heuristics.

4. **The Pitfall of Implicit Magic**:
   Attempting to solve aggregation by having the host automatically mutate items (e.g., automatically injecting bindings or normalizing relative command names like `open` to `apps:open`) violated pipeline idempotence and produced different outputs between standalone and suite execution contexts.

---

## Decision Drivers

* **Absolute Self-Containment (Zero-Coupling Workflows)**: Atomic workflows must never reference external sibling workflows in their manifests or data outputs.
* **Elimination of Parasitic Feeds**: Completely remove view-level feeds, command projection, and hidden item provenance tracking from the host.
* **Domain Purity (Commands Belong to Workflows)**: Business actions are capabilities of the workflow domain, not presentation window artifacts.
* **Explicit Composition over Implicit Magic**: The host must never mutate query data. Data query and command inspection must be orthogonal, leaving composition sovereignty entirely to caller scripts.
* **Preservation of Registry Stability and Safety**: Retain ADR 0004's strict `View > Engine > Host` dispatch order and prevent selection state from causing high-frequency registry lock thrashing.

---

## Architectural Decisions

### 1. Complete Abolition of `feeds` and Command Projection

- The table `[[views.<name>.engine.config.feeds]]` is completely removed.
- Internal runtime mechanisms associated with feeds—specifically `is_feeds_page`, `expand_feed_patterns`, `snapshot.owner_view`, feed-level badge rendering, and command projection—are permanently excised.
- Picker views return to being single-stream, pure presentation engines.

---

### 2. Workflow-Scoped Commands

Business commands are promoted from individual views to the workflow root table:

```toml
# workflows/apps/workflow.toml
[workflow]
api = 1
name = "applications"
entrypoint = "main"

# Commands are domain capabilities of the workflow
[commands.open]
label = "Launch"
type = "run"
handler = { file = "scripts/open.sh" }

[commands.open_dir]
label = "Open Directory"
type = "run"
handler = { file = "scripts/open_dir.sh" }
```

- **Local Naming**: Inside a workflow, commands are referenced strictly by their local identifiers (e.g., `open`, `open_dir`). Workflows have zero awareness of their suite-assigned member aliases.
- **Suite-Level Fully Qualified Identifier (FQID)**: When mounted in a suite (e.g., `apps = { dir = "..." }`), the host automatically registers commands in the session command pool under `<member_id>:<command_id>` (e.g., `apps:open`, `apps:open_dir`).

---

### 3. Explicit View Binding Strategies (`mode = "static"` vs `mode = "item"`)

A View governs how keyboard inputs map to commands. To prevent precedence confusion and implicit fallback bugs, views must adhere to one of two mutually exclusive binding modes:

```toml
# View Keymap Specification
[views.main.keymap]
mode = "static" | "item" # Defaults to "static" if omitted
```

#### Mode A: Static Mode (`mode = "static"`, Default)
Suitable for 95% of standard single-purpose views (e.g., calculator, application launcher, dmenu):
- **Omission Rule**: If `mode` is omitted, `mode = "static"` is assumed by default.
- Key bindings map directly and immutably to local workflow commands:
  ```toml
  [views.main.keymap]
  "enter"  = "open"
  "ctrl+o" = "open_dir"
  "escape" = "exit"
  ```
- Items produced by data scripts remain pure data (e.g., `[{"display": "...", "value": "..."}]`) and are prohibited from altering key mappings.

#### Mode B: Item-Driven Mode (`mode = "item"`)
Dedicated to aggregate launchers (e.g., `hub`) or heterogeneous menus:
- The View explicitly relinquishes static key mapping:
  ```toml
  [views.main.keymap]
  mode = "item"
  ```
- In this mode, physical key dispatch is entirely delegated to the `bindings` dictionary attached to the currently focused Item.

---

### 4. Zero-Lock Dispatch-Time Resolution for Item Bindings

To permanently eliminate lock contention on `CommandRegistry` during high-frequency cursor navigation (e.g. holding `j` or rapid typing):

- **No Registry Thrashing**: The selection change event does **not** call `CommandRegistry::replace_scope`. The global command registry remains 100% read-only throughout item navigation.
- **Dispatch-Time Late-Binding**:
  1. In `mode = "item"` views, the active View registers standard delegated slot receivers in `CommandScope::View` during view mount (e.g., routing `Enter` to the item-action dispatcher).
  2. When a physical key is pressed, the dispatcher asks the `Picker` engine for the currently focused item's binding for that key (e.g. `"enter" -> "apps:open"`).
  3. The dispatcher resolves the command directly against the static session command pool and executes its handler, completely bypassing registry writes.
- **Passive Footer Inspection**: The Chrome Footer reads the focused item's bound command metadata directly from the static session command pool during passive draw passes without acquiring write locks or publishing `CommandsChanged` events.

---

### 5. Orthogonal Headless CLI / IPC Interface (`--items` vs `--inspect`)

To avoid collision with suite aliases and member workflow names (where a view or alias named `query` would cause CLI parsing ambiguity and concept overloading), the host exposes two strictly orthogonal, non-mutating flag interfaces:

#### 1. Data Query Interface (`tlaunch --items <view_ref> [QUERY_OPTIONS...]`)
- Runs the item producer of the specified view and outputs the strict JSON array stream to stdout.
- **Strict JSON Array Protocol & Error Propagation**: The host validates that the target view's producer returns a valid JSON array. Non-zero producer exit codes or schema violations are propagated directly to stderr with a non-zero exit code (exit code 1 for runtime errors, 2 for missing views), ensuring calling scripts fail fast instead of ingesting corrupted pipelines.
- **Zero-Mutation Invariant**: The host returns the exact JSON array emitted by the producer without field injection, binding synthesis, or automatic namespace qualification.

#### 2. Contract Inspection Interface (`tlaunch --inspect <view_ref>`)
- Emits the static metadata and binding contract of the target view in the current suite context:
  ```json
  {
    "view": "apps:main",
    "mode": "static",
    "commands": {
      "apps:open": { "label": "Launch" },
      "apps:open_dir": { "label": "Open Directory" }
    },
    "keymap": {
      "enter": "apps:open",
      "ctrl+o": "apps:open_dir"
    }
  }
  ```

#### 3. Explicit Combinator Script Pattern
Aggregation scripts run as standard external processes (Python, Bash, JQ) and explicitly assemble data and command pointers without host magic:

```python
#!/usr/bin/env python3
import json, subprocess

# 1. Fetch unmutated raw items via --items flag
apps_raw = json.loads(subprocess.check_output(["tlaunch", "--items", "apps:main"]))
calc_raw = json.loads(subprocess.check_output(["tlaunch", "--items", "calc:main"]))

# 2. Explicitly attach command bindings
apps_items = [{**i, "bindings": {"enter": "apps:open"}} for i in apps_raw]
calc_items = [{**i, "bindings": {"enter": "calc:copy"}} for i in calc_raw]

# 3. Custom ranking, sigil filtering, and output
print(json.dumps(calc_items + apps_items))
```

---

### 6. Parameterized Suite Entrypoint & Context Sandboxing

#### 1. Query Parameter Injection
Suite manifests support passing initial query parameters to their entrypoint:

```toml
# default.toml
[suite]
api = 1
name = "Desktop Suite"

[workflows]
hub  = { dir = "./workflows/hub" }
apps = { dir = "./workflows/apps" }
calc = { dir = "./workflows/calc" }

[suite.entrypoint]
target = "hub:main"
query = { sources = ["apps:main", "calc:main"] }
```

- A generic `hub` workflow reads `context.parameters.sources` dynamically, making it a reusable template across completely different suites (e.g., DevOps vs. Desktop).

#### 2. Environment Sandboxing
When executing producer scripts, the host exports:
- `TLAUNCH_SUITE`: The absolute filesystem path to the active suite manifest.
- When child scripts invoke `tlaunch --items` or `tlaunch --inspect`, the CLI automatically resolves within the parent suite's configuration sandbox, eliminating context drift.

---

## Consequences

### Positive
- **Architectural Purity**: Completely dismantles the asymmetric "God workflow" pattern (`core`); all workflows become 100% self-contained, independent chips.
- **Extreme Extensibility**: Advanced launcher logic (prefix sigils like `=`, fuzzy scoring, grouping, debouncing) moves entirely to user/author aggregation scripts without engine changes.
- **Zero Black-Box Mutation**: `tlaunch --items` is an idempotent, pure UNIX filter.
- **Registry Stability**: Keeps high-frequency item selection separated from static command dispatch tables, avoiding lock contention and priority inversion.
- **Reusable Hubs**: Hub workflows become pure parameterizable utilities driven by `suite.entrypoint.query`.

### Implementation Milestones & Verification Requirements
This ADR is marked **Proposed** until the following five implementation milestones are completed:

1. **CLI & Query Protocol**: Implement `tlaunch --items <view_ref>` with end-to-end integration tests covering stdout array output, parameter passing, and non-zero exit code error propagation.
2. **De-aggregation Cleanup**: Fully remove `FeedInstance`, `is_feeds_page`, `expand_feed_patterns`, and `snapshot.owner_view` from `src/engine/picker/`, `src/workflow/config/`, and `src/protocol/command_adapter.rs`. Delete `tests/fixtures/config/workflows/core/`.
3. **Workflow Root Commands Migration**: Migrate workflow commands from `[views.<name>.commands]` to root `[commands]`, while issuing clear deprecation diagnostics for legacy locations.
4. **Item Binding Dispatch Verification**: Implement dispatch-time late-binding in the Picker engine and verify that high-speed cursor scrolling performs zero `CommandRegistry` write locks.
5. **Documentation Synchronization**: Scrub references to `feeds` across `docs/how-to/picker-views.md`, `docs/reference/workflow-toml.md`, and tutorials, replacing them with the explicit aggregation script recipe.
- Transition status to **Accepted** only once all five milestones are verified and merged.
