---
title: "Suite Manifest Specification"
type: "reference"
tags:
  - suite
  - toml
  - specification
  - orchestration
  - manifest
description: "Authoritative reference for tflow suite manifests, workflow mounting, entrypoints, alias routing, and semantic style slot overrides."
---

# Suite Manifest Specification

An orchestration suite manifest (`default.toml` or `<name>.toml`) defines a cohesive collection of atomic workflows. It acts as the orchestration container (the "circuit board") that mounts self-contained workflows, defines the session entrypoint, centralizes routing aliases, and optionally applies suite-scoped style slot overrides.

By default, launching `tflow` loads `$XDG_CONFIG_HOME/tflow/default.toml`. An alternate suite is launched explicitly using `tflow -s <PATH>` (or `--suite <PATH>`).

## Architectural Guarantees

In accordance with [ADR 0005](../adr/0005-manifest-driven-suites-and-self-contained-workflows.md) and [ADR 0006](../adr/0006-feed-removal-workflow-scoped-commands-and-item-bindings.md):

- **Strict Two-Tier Flatness (No Nesting)**: A suite manifest can only mount atomic workflows. Suites cannot mount other suites, and workflows cannot declare dependencies on other workflows. The system depth is strictly clamped to 1.
- **Type Safety**: The CLI flag `-s` / `--suite` accepts only suite manifests. Passing an atomic workflow (`[workflow]`) to `-s` fails validation immediately.
- **Pure Self-Containment**: Workflows remain completely agnostic to external suite aliases. All public shorthand routes and alias dispatch are owned exclusively by the suite manifest.
- **Settings Separation**: Suites reject host-level settings (`theme`, `image_protocol`, `log_file`, and `[defaults]`). Host preferences belong exclusively to `settings.toml`.

---

## Schema Overview

```toml
[suite]
api = 1
name = "Development & Desktop Suite"
entrypoint = "core:main"

[workflows]
core = { dir = "./workflows/core" }
calculator = { dir = "./workflows/calculator" }
git = { file = "./workflows/git.toml" }
dmenu = "./workflows/dmenu"

[aliases]
calc = "calculator:main"
co = "git:branches"

[styles.git.staged]
foreground = "scheme:accent"
bold = false
underline = true
```

---

## Configuration Reference

### 1. Suite Header (`[suite]`)

The required `[suite]` section identifies the suite and its default entrypoint.

| Field | Type | Required / Default | Description |
| :--- | :--- | :--- | :--- |
| `api` | integer | Optional (default: `1`) | The suite manifest schema version. Must be `1`. |
| `name` | string | **Required** | A descriptive, human-readable name for the suite. Cannot be empty. |
| `entrypoint` | string or table | **Required** | The initial View mounted when the suite starts. |

#### Entrypoint Variants

##### Shorthand Target (String)
Specifies the canonical View reference (`<member_id>:<view_id>`):

```toml
[suite]
api = 1
name = "My Tools"
entrypoint = "git:branches"
```

##### Detailed Target with Initial Query (Table)
Specifies the target along with static query parameters passed to the entrypoint view upon session launch:

```toml
[suite]
api = 1
name = "Aggregated Hub Suite"

[suite.entrypoint]
target = "core:main"

[suite.entrypoint.query]
sources = [
  "calculator:main",
  "apps:main",
  "sys:main",
]
```

This form enables reusable "hub" workflows whose data sources or parameters are driven dynamically by the suite manifest without modifying the workflow itself.

---

### 2. Workflow Mount Table (`[workflows]`)

The `[workflows]` table explicitly mounts member workflows into the suite session under unique member identifiers (`<member_id>`). Placing workflow files in a directory does not automatically register them.

Member IDs:
- Must consist of alphanumeric characters and underscores.
- Cannot begin with double underscores `__` (reserved for host built-in workflows such as `__commands` and `__form`).

Each entry specifies the relative or absolute path to the atomic workflow:

| Format | Syntax Example | Description |
| :--- | :--- | :--- |
| **Directory Mount** | `core = { dir = "./workflows/core" }` | Mounts a directory-based workflow rooted at `./workflows/core/workflow.toml`. Sets `$TFLOW_WORKFLOW_DIR` for child processes. |
| **File Mount** | `git = { file = "./workflows/git.toml" }` | Mounts a single-file atomic workflow. |
| **String Path** | `dmenu = "./workflows/dmenu"` | Convenient string scalar syntax; automatically resolves to either a file or a directory based on filesystem checks. |

*Note: All relative paths are resolved relative to the directory containing the suite manifest file.*

#### Automatic Routing
Mounting a workflow under `<member_id>` automatically establishes shorthand routing:
- Running `tflow <member_id>` or navigating to `<member_id>` opens that workflow's declared `entrypoint` view.
- Commands belonging to the member are registered in the session command pool under fully qualified identifiers: `<member_id>:<command_id>`.

---

### 3. Alias Routing Table (`[aliases]`)

The `[aliases]` table defines suite-scoped public shorthand routes for views:

```toml
[aliases]
calc = "calculator:main"
app = "apps:main"
sys = "sys:main"
```

#### Rules & Constraints
- **Format**: `"<alias_name>" = "<member_id>:<view_id>"`.
- **Target Verification**: The target must resolve to a valid View declared by one of the mounted member workflows.
- **Collision Rules**:
  - An alias cannot redirect an existing member name to a different view.
  - A repeated alias pointing to the identical target is permitted.
  - Sibling workflows cannot collide because aliases are centralized in the suite manifest.

---

### 4. Semantic Style Slot Overrides (`[styles.<member_id>.<slot>]`)

A suite may surgically override semantic style slots declared by member workflows without altering global theme palettes:

```toml
[styles.git.staged]
foreground = "scheme:accent"
bold = false
underline = true

[styles.git.untracked]
foreground = "ansi:red"
```

The style resolution cascade follows strict priority (highest to lowest):
1. **Host Settings Overrides** (`settings.toml`: `[styles.<member_id>.<slot>]`)
2. **Suite Manifest Overrides** (`<suite>.toml`: `[styles.<member_id>.<slot>]`)
3. **Active Global Theme** (`themes/<name>.toml`: `[workflows.<member_id>.styles.<slot>]`)
4. **Workflow Baseline Defaults** (`workflow.toml`: `[styles.<slot>]`)

---

## Prohibited Fields (Purity Invariant)

To maintain architectural separation, the host validates suite manifest purity at load time. The following sections and keys cause an immediate startup failure if present in a suite manifest:

| Prohibited Section / Key | Reason & Corrective Action |
| :--- | :--- |
| `[workflow]` | A suite is not an atomic workflow. Use `-w` / `--workflow` to launch a single workflow. |
| `[views]` | Suites cannot define views. Views must belong to atomic workflows mounted under `[workflows]`. |
| `[commands]` | Suites cannot define commands. Business commands belong to atomic workflows. |
| `theme`, `image_protocol`, `log_file` | Host environment settings belong in `settings.toml`. |
| `[defaults]` | Global engine defaults belong in `settings.toml`. |
| Nested `[suite]` | Suites cannot mount other suites. The hierarchy is strictly two tiers. |

---

## Environment Variables

When a suite is active, child processes and commands executed by `tflow` receive:

- `TFLOW_SUITE`: The absolute filesystem path to the active suite manifest. Subcommands (such as `tflow --items` or `tflow --inspect`) read this variable to automatically resolve within the parent suite's configuration sandbox.
