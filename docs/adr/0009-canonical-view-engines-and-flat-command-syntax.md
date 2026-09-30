---
title: "ADR 0009: Canonical View Engine Tables, 5-Field Preview, and Flat Command Syntax"
type: "concept"
tags:
  - adr
  - architecture
  - configuration
  - workflow
  - engine
  - preview
  - commands
description: "Formalize view-level contracts with explicit engine declarations and named engine tables, simplify picker preview to five canonical fields, flatten command definitions by removing producer/handler wrappers, and enforce a clean-break policy for v0.1.0-alpha.6."
---

# ADR 0009: Canonical View Engine Tables, 5-Field Preview, and Flat Command Syntax

- **Status**: Accepted
- **Date**: 2026-10-01
- **Scope**: View engine declarations (`engine = "..."`), named engine sub-tables (`[views.<name>.<engine>]`), canonical 5-field picker preview (`[views.<name>.preview]`), flattened commands (`[commands.<id>]`) and return processors (`return_processor`), direct data sources (`items`, `output`, `content`), and clean-break removal of legacy compatibility shims.
- **Related decisions**: [ADR 0002](0002-static-configuration-and-script-boundaries.md), [ADR 0005](0005-manifest-driven-suites-and-self-contained-workflows.md), [ADR 0006](0006-feed-removal-workflow-scoped-commands-and-item-bindings.md), [ADR 0008](0008-workflow-defined-features-host-bindings-and-data-driven-chrome.md).

## Context

Following the architectural convergence of command routing and host layers in ADR 0008, the syntax layer of `workflow.toml` still suffered from accumulated structural legacy and cognitive overhead:

1. **Semantic Misattribution and Polymorphic Tables:**
   Views previously configured engines using a generic `[views.<name>.engine]` table with an inner `type = "<engine>"` string, alongside an optional `config` sub-table or direct fields. Even worse, engine-specific properties (such as `items` or `preview`) were occasionally declared directly at the top level of `[views.<name>]`. This created semantic confusion: `items` and `preview` are specific to the `picker` engine, yet their placement suggested they were generic view-level primitives applicable to `capture`, `form`, or `embedded`.

2. **Preview Configuration Sprawl and Vestigial Abstractions:**
   Picker preview configuration had accumulated multiple historical forms: `[views.<name>.preview]`, `[views.<name>.engine.config.preview]`, `preview_ratio`, `preview_min_width`, and `preview_default_open`. It also retained vestigial concepts such as `inherit` (which became obsolete once ADR 0006 removed Picker feeds) and static TOML `document` AST nodes that competed with dynamic script execution.

3. **Boilerplate Wrappers around Commands and Return Processors:**
   Declaring any command required nesting under a `producer` discriminator and a `handler` table:
   ```toml
   # Legacy boilerplate
   [commands.open]
   type = "run"
   producer = "declared"
   [commands.open.handler]
   argv = ["xdg-open", "{item.path}"]
   ```
   This two-layer nesting (`producer` + `handler`) added repetitive noise without functional benefit. The same friction affected `return_processor`, `capture.output`, and `form.content`.

4. **Hidden Complexity of Backward Compatibility in Pre-1.0 Alpha:**
   Retaining fallback deserialization branches, desugaring shims, and verbose migration guidance across parsers and test suites degraded internal maintainability and obscured configuration errors.

## Decision

### 1. Explicit Engine Declaration and Named Engine Sub-Tables

Every view declaration must explicitly specify its engine via a required top-level string field:

```toml
[views.main]
engine = "picker" # Exactly one of: "picker", "capture", "form", "embedded"
```

Engine-specific properties are declared exclusively in a named sub-table corresponding to the view's engine:

- `[views.<name>.picker]`
- `[views.<name>.capture]`
- `[views.<name>.form]`
- `[views.<name>.embedded]`

Mismatches between the declared `engine` and the configured sub-table produce immediate, unambiguous validation errors (e.g. `view "main" declares engine = "picker", but configures [views.main.capture]`). Configuring multiple engine sub-tables on a single view is strictly prohibited.

### 2. Canonical 5-Field Picker Preview

Picker preview configuration is formalized exclusively as `[views.<name>.preview]` under views with `engine = "picker"`. The preview specification admits exactly five canonical fields:

| Field | Type | Description |
| :--- | :--- | :--- |
| `open` | `bool` | Initial visibility state (default: `false`). |
| `width` | `integer` | Percentage of terminal width allocated when open (`10`–`90`, default: `50`). |
| `min_width` | `integer` | Minimum column threshold required to render the preview pane (default: `40`). |
| `file` | `string` | Relative path to a script generating preview documents. |
| `script` | `string` | Inline script generating preview documents. |

All legacy aliases (`preview_ratio`, `preview_min_width`, `preview_default_open`), the obsolete `inherit` option, and static TOML `document` definitions are removed. Preview configuration on non-picker engines (`capture`, `form`, `embedded`) is rejected at load time.

### 3. Flattened Commands and Return Processors

The `producer` and `handler` wrapper levels are completely eliminated.

- **Dynamic Script Commands:** Directly declare `file = "..."` or inline `script = "..."`.
- **Declarative Operations:** Directly declare the operation's fields (`argv`, `target`, `value`, etc.) at the top level of the command table.

```toml
# Declarative run command
[commands.open]
type = "run"
argv = ["xdg-open", "{item.path}"]

# Dynamic script command
[commands.inspect]
type = "run"
script = '''#!/usr/bin/env bash
jq . <<< "$1"
'''

# Declarative return processor on a call command
[commands.pick_user]
type = "call"
target = "users:selector"
return_processor = { type = "navigate", target = "details", clear_input = true }
```

### 4. Flat Engine Data Sources

Data source declarations across all engines adopt the same flat convention:
- **`picker.items`**: Accepts an array of item records, an inline script string (`script = "..."`), or a script path (`file = "..."`).
- **`capture.output`**: Accepts an inline script or file directly under `[views.<name>.capture.output]`.
- **`form.content`**: Accepts static fields directly (`fields = [...]`) or dynamic script definitions (`file` / `script`).

### 5. Clean-Break Policy

In `v0.1.0-alpha.6`, all backward compatibility shims, fallback deserializers, and migration notices are removed. Unknown or legacy fields (`producer`, `handler`, `engine.type`) trigger standard parser errors rather than transitional desugaring paths.

## Consequences

### Positive
- **Predictable Structure:** A view's engine and configuration are visible at a glance without polymorphic deduction.
- **Dramatic Boilerplate Reduction:** Eliminating `producer` and `handler` wrappers saves 30–50% of lines across workflow manifests.
- **Clean Invariants:** Engine sub-tables cleanly isolate engine-specific capabilities, eliminating cross-engine property bleed.
- **Foundation for Sub-Panes:** A clean, standardized preview model establishes the foundation for future sub-pane containerization (e.g. nesting `capture` or `embedded` engines within a preview region).

### Breaking Changes
- Workflows using `[views.<name>.engine]` or omitting `engine = "..."` will fail to load.
- Commands or return processors containing `producer` or `handler` tables will be rejected with unknown field errors.
- Previews using legacy ratios, top-level preview aliases, or static TOML documents must be migrated to the 5-field schema.
