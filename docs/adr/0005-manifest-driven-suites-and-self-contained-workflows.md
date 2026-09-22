---
title: "ADR 0005: Manifest-Driven Workflow Suites and Self-Contained Workflows"
type: "concept"
tags:
  - adr
  - architecture
  - workflow
  - suite
  - manifest
  - cli
  - theming
description: "Establish manifest-driven workflow suite orchestration, strict two-tier non-nesting, pure self-contained workflow contracts, -w vs -s CLI semantics, and semantic style slot overrides."
---

# ADR 0005: Manifest-Driven Workflow Suites and Self-Contained Workflows

- **Status**: Accepted
- **Date**: 2026-09-19
- **Scope**: `workflow/config`, `app/cli`, `ui/theme`, suite manifest schema, workflow self-containment contract, entrypoint resolution, CLI dispatch flags (`-w` vs `-s`), centralized alias routing, style slot overriding, and host environment separation.

---

## Context

In ADR 0001, `tflow` transitioned from legacy plugins to decentralized workflow packages residing in `$XDG_CONFIG_HOME/tflow/workflows/` and introduced single-file workflows with Shebang integration (`#!/usr/bin/env -S tflow -w`). While this simplified dotfiles management, subsequent operational adoption and architectural analysis revealed severe structural tensions:

1. **Category Confusion between Global Environment and Session Orchestration**:
   `config.toml` simultaneously served as the passive host environment specification (`image_protocol`, `theme`, `defaults`) and the multi-workflow session controller (`default_view`, `disabled_workflows`). When running standalone single workflows (`tflow -w file.toml`), the loader had to perform ad-hoc mutation (`table.remove("default_view")`) to avoid dangling-reference crashes.
2. **Global Alias Collisions and Lost Self-Containment**:
   ADR 0001 enforced strict global uniqueness for `alias` declared in `[views.<name>]`. To be runnable standalone without explicit view flags, single-file workflows were coerced into declaring `alias = "main"`. When multiple workflows declaring `alias = "main"` were installed into `workflows/`, the configuration compiler aborted with fatal alias collision errors. Global aliases destroyed workflow reusability.
3. **The Multi-Workflow Grouping Dilemma**:
   When launching a cohesive set of workflows (e.g. a project-specific DevOps toolkit), treating directory paths as ambiguous targets created nesting recursion (e.g. `workflows/` inside a suite directory) and entrypoint ambiguity.
4. **The False Abstraction of "Everything is a Workflow"**:
   Attempting to model an orchestration suite as an ordinary `[workflow]` resulted in asymmetric "God workflows" that managed other workflows while lacking business logic of their own. Furthermore, allowing workflows to declare code-level dependencies on other workflows introduced transitive dependency chains, diamond dependency collisions, and lost self-containment.

---

## Decision Drivers

* **Pure Self-Containment (Zero-Coupling Workflows)**: An atomic workflow must be 100% self-contained, closed, and runnable as a standalone CLI tool without external workflow dependencies or global namespace claims.
* **Strict Two-Tier Flatness (No Recursive Nesting)**: Eliminate dependency hell by strictly prohibiting nested suites or workflow-level code dependencies.
* **Explicit CLI Type Safety**: Disambiguate single-workflow execution from suite orchestration at the CLI interface.
* **Pure Environment Baseline**: Ensure global host configuration remains strictly unopinionated regarding specific workflows.
* **Terminal Theming Sovereignty with Granular Style Overrides**: Maintain global user control over terminal color palettes while empowering suites and users to surgically override workflow-specific semantic style slots.

---

## Architectural Decisions

### 1. The Circuit Board & Chip Paradigm (Separation of Manifest and Workflow)

The system formally recognizes two distinct, non-interchangeable specification entities:

```text
┌─────────────────────────────────────────────────────────────┐
│ Suite Manifest ([suite]) ──【The Circuit Board / Container】 │
│   • Governs orchestration, entrypoint, and [aliases]        │
│   • Flatly mounts member workflows                          │
│                                                             │
│   ┌─────────────────────────┐   ┌─────────────────────────┐ │
│   │ Atomic Workflow ([wf])  │   │ Atomic Workflow ([wf])  │ │
│   │ (git-tools.toml)        │   │ (docker.toml)           │ │
│   │ • 100% self-contained  │   │ • 100% self-contained  │ │
│   │ • Zero external deps    │   │ • Zero external deps    │ │
│   └─────────────────────────┘   └─────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

1. **Atomic Workflow (`[workflow]`) — The Chip**:
   - Strictly a self-contained business application.
   - Contains only its own metadata, `entrypoint` (local default view), `[views]`, `[commands]`, and `[styles]`.
   - **Prohibitions**: Cannot declare external workflow dependencies, cannot import other workflows, and cannot declare global aliases.
   - **Guarantees**: Any workflow file can be executed standalone via Shebang with zero outside dependencies.
2. **Suite Manifest (`[suite]`) — The Circuit Board**:
   - Strictly an orchestration specification; it is **not** a workflow and cannot declare views or engines.
   - Contains suite metadata, `entrypoint` (designating which member workflow view opens first), `[workflows]` mount table, and `[aliases]` routing table.

---

### 2. Strict Non-Nesting Invariant (Flat 2-Tier Architecture)

To permanently eliminate dependency hell, diamond dependencies, and deep path resolution (`a:b:c:view`):

- **Tier 1 (Leaf)**: Atomic Workflows (`[workflow]`). They cannot contain or reference other workflows.
- **Tier 2 (Orchestrator)**: Suite Manifests (`[suite]`). They can only flatly mount atomic workflows.
- **Invariant**: Suite manifests cannot mount other suite manifests. Workflows cannot declare dependencies. The structural depth of the system is strictly clamped to 1.
- **Inter-Workflow Communication**: Sibling workflows mounted in the same session interact strictly via **runtime route navigation** (`type = "navigate"` or `type = "call"` with `target = "<member>:<view>"`), never via physical code bundling.

---

### 3. Disambiguated CLI Dispatch (`-w` vs `-s`)

The CLI interface formally differentiates atomic workflow execution from suite orchestration:

| Flag | Long Name | Target File Header | Description | Shebang Declaration |
| :--- | :--- | :--- | :--- | :--- |
| **`-w`** | **`--workflow`** | `[workflow]` | Executes a single, self-contained atomic workflow. | `#!/usr/bin/env -S tflow -w` |
| **`-s`** | **`--suite`** | `[suite]` | Mounts and executes an orchestration suite manifest. | `#!/usr/bin/env -S tflow -s` |

- **Type-Strict Validation**: Passing a `[suite]` manifest to `-w` or a `[workflow]` file to `-s` fails immediately during argument validation with an explicit diagnostic and corrective tip.
- **Default Launch (`tflow` without flags)**: Automatically executes the user's default suite manifest located at `$XDG_CONFIG_HOME/tflow/default.toml`.

---

### 4. Pure Global Environment Baseline (`settings.toml`)

The root configuration file at `$XDG_CONFIG_HOME/tflow/settings.toml` is strictly reserved for the passive host environment:

```toml
# ~/.config/tflow/settings.toml
image_protocol = "kitty"
theme = "catppuccin"
log_file = "/tmp/tflow.log"

[defaults.picker.bindings]
select_next = ["down", "ctrl+j"]
select_previous = ["up", "ctrl+k"]
exit = ["ctrl+c"]
```

- **Purity Invariant**: `settings.toml` contains zero workflow identifiers, zero view selectors, zero `default_view` fields, and zero `disabled_workflows` tables.
- **Universal Inheritance**: Both `-w` single workflows and `-s` suites safely inherit this environment baseline without field stripping or sanitization hacks.

---

### 5. Centralized Suite Alias Routing (`[aliases]`)

All routing aliases are stripped from individual workflow views and centralized in the suite manifest:

```toml
# devops.toml
[suite]
name = "DevOps Toolkit"
entrypoint = "git:branches"

[workflows]
git = { file = "./git-tools.toml" }
docker = { file = "./docker-manager.toml" }

[aliases]
co = "git:checkout"
ps = "docker:containers"
```

- **Member Key as Default Shorthand**: The key under `[workflows.<key>]` automatically registers `<key>` as an alias mapping to `<key>:<entrypoint>`.
- **Zero Collision Guarantee**: Because aliases are declared exclusively by the suite author, sibling workflows cannot collide. Workflows remain 100% agnostic to external aliases.

---

### 6. Semantic Style Slot Overrides (Rejection of Local Themes)

To preserve terminal visual consistency, suite-level private themes are explicitly rejected. All workflows render using the active global theme scheme (`scheme:*`).

However, suites and users are granted surgical overriding authority over **workflow semantic style slots**:

```toml
# In suite.toml or settings.toml:
[styles.git.staged]
foreground = "scheme:accent"
bold = false
underline = true

[styles.docker.stopped]
dim = true
```

- **Deep Field Merging**: Overrides merge field-by-field over the workflow's built-in `[styles.<slot>]` defaults.
- **Scheme Adaptability**: Style overrides specify semantic tokens (`scheme:accent`), ensuring harmonious rendering across any global user theme (Catppuccin, Nord, Gruvbox) without hardcoding `#RRGGBB` values.

---

### 7. Ephemeral Pipeline Execution Contract (`npx` / UNIX Stream)

For headless or ephemeral execution (`curl ... | npx -y tflow -w -`):

- **Standard Input (`stdin`)**: Streamed TOML specifications passed via `-w -` are ingested into memory without physical disk writes.
- **Controlling Terminal (`/dev/tty`)**: Terminal rendering and keyboard event polling strictly attach to `/dev/tty`, isolating user interaction from piped data streams.
- **Standard Output (`stdout`)**: Exclusively carries structured invocation results emitted by `type = "return"` commands, preserving pure functional composability in UNIX pipelines.

---

## Schema Specifications

### A. Atomic Workflow Schema (`<id>.toml`)

```toml
#!/usr/bin/env -S tflow -w
[workflow]
api = 1
name = "Git Checkout"
entrypoint = "branches" # Workflow-local default view

[views.branches.query]
prompt = { type = "string", default = "Select branch:" }

[views.branches.engine]
type = "picker"

[views.branches.engine.config.items]
producer = "process"
command = ["git", "branch", "--format=%(refname:short)"]

[views.branches.commands.checkout]
key = "enter"
type = "return"
handler = { value = "item.value" }
```

### B. Suite Manifest Schema (`<suite>.toml`)

```toml
#!/usr/bin/env -S tflow -s
[suite]
api = 1
name = "System Ops"
entrypoint = "git:branches"

[workflows]
git = { file = "./git.toml" }
docker = { dir = "./docker-pkg" }

[aliases]
co = "git:checkout"

[styles.git.staged]
foreground = "scheme:accent"
```

---

## Consequences

### Positive
- **Guaranteed Reusability**: Any workflow can be shared, moved, or executed standalone without broken dependencies or alias collisions.
- **Zero Ambiguity**: `-w` runs a workflow; `-s` runs a suite manifest; `tflow` runs the default suite.
- **No Transitive Complexity**: Clamping structural depth to 1 eliminates cyclic dependencies, deep paths, and diamond dependency resolution.
- **Sanitization Elimination**: `settings.toml` is completely decoupled from workflow selection, eliminating runtime field mutation.
- **Cohesive Theming**: Preserves user terminal theming sovereignty while providing surgical slot overrides.

### Negative
- **Clean Break**: Workflows declaring `alias` in `[views.*]` must migrate aliases to their parent `suite.toml`.
- **Breaking Change for `config.toml`**: `default_view` and `disabled_workflows` are removed from `config.toml` (renamed to `settings.toml`); existing multi-workflow environments must declare a `default.toml` suite manifest.
