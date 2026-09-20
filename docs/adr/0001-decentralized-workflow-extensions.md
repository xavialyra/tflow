---
title: "ADR 0001: Decentralized Workflow Extensions Architecture"
type: "concept"
tags:
  - adr
  - architecture
  - workflow
  - cli
  - unix
description: "Architecture Decision Record establishing decentralized workflow package organization, dual-mode files and directories, inline scripts, and CLI multiplexing."
---

# ADR 0001: Decentralized Workflow Extensions Architecture

* **Status**: Accepted; the M3 theming contract in section 6 was superseded on 2026-09-11 by the [flat scheme specification](../reference/theme-toml.md).
* **Date**: 2026-09-06
* **Scope**: `workflow/config`, `app/cli`, `ui/theme`

---

## Context

`tlaunch` is an extensible terminal workflow host designed for keyboard-driven navigation and command orchestration. In the host runtime, views are governed by explicit query schemas, structured view stacks (with modal popup and call/return boundaries), and bounded producer protocols.

Historically, external views and commands were bundled as "plugins" located strictly in subdirectories under `$XDG_CONFIG_HOME/tlaunch/plugins/<id>/plugin.toml`. As adoption patterns matured, several architectural tensions emerged:

1. **Category Mismatch**: Units previously modeled as "plugins" are strictly declarative workflow specifications (view definitions, routing aliases, query schemas, and command triggers) rather than invasive host plugins.
2. **High Sharing Friction**: Requiring a multi-level subdirectory and companion physical script files creates friction for terminal enthusiasts who share standalone recipes across Gists, dotfiles, or discussion forums.
3. **Packaging Over-Engineering**: Terminal power users prioritize decentralized, dotfiles-friendly, plain-text configuration over centralized package managers, registries, or opaque download tooling.

---

## Decision Drivers

* **Unix Philosophy**: Everything is a file; plain text is the universal interface; conventions over complex configuration registries.
* **Frictionless Sharing**: A workflow should be capable of existing as a self-contained, single-file document that can be installed via a direct copy or `curl`.
* **Orthogonal Namespaces**: Workflows operate as peers without imperative execution sequence or heuristic priority rules.
* **Preservation of Core Guarantees**: Retain the host's strict schema verification, bounded producer execution, and Material Design 3 (M3) semantic styling contract.

---

## Architectural Decisions

### 1. Domain Terminology and Storage Layout: Transition to `workflows/`

- **Nomenclature**: The term `plugin` is replaced by **`workflow`**. Manifest files represent declarative workflow packages rather than internal engine plugins.
- **Directory Convention**: Workflows reside in `$XDG_CONFIG_HOME/tlaunch/workflows/`.
- **Deterministic Namespace Mapping**: The workflow namespace ID is directly derived from the file stem or directory name. No ordering prefixes are used; all workflows exist as orthogonal peers.
- **Explicit Conflict Rejection**: If duplicate workflow IDs (including collisions between a single-file `workflows/<id>.toml` and a directory `workflows/<id>/`) or duplicate view aliases are detected across manifests, the configuration compiler aborts with an immediate fatal error detailing both conflicting source paths. Silent shadowing, heuristic precedence overrides, or runtime alias rebinding are strictly prohibited.
- **Clean Break**: No backwards compatibility is maintained for legacy `plugins/` directories or `[plugin]` table headers. As this tool is pre-release, the host cleanly and exclusively recognizes `workflows/` and `[workflow]` without legacy shims.

### 2. Dual-Mode Workflow Layout (Single File and Directory Coexistence)

The loader scans `workflows/` and uniformly resolves two physical layouts:

1. **Single-File Workflows (`workflows/<id>.toml`)**:
   - Primary vehicle for distribution and lightweight recipes.
   - Self-contained definitions invoking system binaries or inline scripts.
   - Namespace resolves to `<id>`.
   - **No External Relative Scripts**: Single-file workflows are strictly self-contained and prohibited from referencing relative external script files (`script = "path/to/file"`), ensuring no ambiguous `$WORKFLOW_DIR` boundary or leakage across sibling workflows.
2. **Directory Workflows (`workflows/<id>/workflow.toml`)**:
   - Suited for complex workflows requiring private test suites, multi-file Python/Bash scripts, or local static assets.
   - Namespace resolves to `<id>`.
   - Relative producer script references in manifests (e.g., `handler.file = "scripts/feed.sh"`) are confined to the workflow root and resolved by the host before execution.
   - The host injects `WORKFLOW_DIR` pointing to the workflow's root directory (`$XDG_CONFIG_HOME/tlaunch/workflows/<id>`) for inter-script asset references.
3. **Host-Side Absolute Path Resolution (Zero `$PATH` Pollution)**:
   - The host does not prepend or mutate the child process `$PATH`, completely eliminating the risk of system command hijacking (e.g., shadowing `git`, `cat`, `test`) and environment leakage to sub-processes.

### 3. Native Multi-Line Inline Scripts via Temporary Read-Only Files

Command specifications support inline script bodies alongside external file paths:

```toml
[views.main.commands.checkout]
key = "enter"
type = "run"
producer = "script"

[views.main.commands.checkout.handler]
script = """
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "checkout producer"
"""
```

- **Execution Model**: Inline scripts are materialized to temporary read-only files under `$XDG_RUNTIME_DIR/tlaunch/scripts/` (falling back to `$XDG_CACHE_HOME/tlaunch/scripts/`).
  - Leveraging `$XDG_RUNTIME_DIR` mounts directly into `tmpfs` (RAM), delivering in-memory execution speed without physical disk wear.
  - Strict user-only permissions (`0600`) prevent unauthorized access or tampering in shared environments.
  - Written via atomic file creation (`<hash>.<pid>.tmp` renamed to `<hash>`) to guarantee race-free concurrency.
  - Minimal content-addressed caching (hashed by script body) avoids redundant filesystem allocations during rapid picker feedback loops. To maintain architectural simplicity, the host relies on `tmpfs` lifecycle boundaries without introducing complex eviction policies or daemon cleanup machinery.
- **Host-Driven Shebang Resolution (Native `noexec` Immunity)**:
  - The host inspects the first line for a `#!` shebang. If declared, the host extracts the interpreter path and splits accompanying arguments (supporting multi-argument options such as `/usr/bin/env -S bash -euo pipefail` or `/usr/bin/python3 -u`). If undeclared, it defaults to `/bin/sh`.
  - If the resolved interpreter executable does not exist or cannot be accessed on `$PATH`, the host fails early with an explicit, user-friendly diagnostic error before process dispatch.
  - The host directly invokes the interpreter (`Command::new(interpreter)...`), passing the temporary script path as an argument. Because the script is opened in read-only mode by the system interpreter (rather than executed directly via kernel `execve`), inline scripts are completely immune to `noexec` restrictions across `/tmp`, `/run`, or cache directories.
- **Deterministic Diagnostics & Source Attribution**:
  - File-backed execution preserves line numbers and clear stack traces when scripts fail, while keeping standard input/output fully attached for interactive terminal workflows.
  - **Source Attribution**: The host embeds human-readable origin comments immediately following the shebang (e.g., `# [tlaunch] source: workflows/<id>.toml -> [views.<name>.commands.<key>]`). Temporary files incorporate semantic prefixes (`<id>_<command>_<hash>`), ensuring that any runtime stack trace or syntax error printed to stderr identifies the originating workflow and command definition.

### 4. Working Directory (CWD) Invariant

- **Strict Caller Context Preservation**: The child process working directory (CWD) strictly remains the user's current terminal directory (`$PWD`) at invocation time.
- **Decoupling Context from Assets**: CWD is never modified by the host to point inside workflow directories. User workspace context (e.g., current Git repository or file tree) is fully preserved, while workflow internal assets are located exclusively via host-resolved absolute paths or `$WORKFLOW_DIR`.

### 5. Multiplexed CLI Entry (`argv[0]`) & Schema Inspection

- **Symlink Multiplexing via Existing CLI Pipeline**: If the launcher binary is executed under an `argv[0]` alias matching a configured view (e.g. via `ln -s tlaunch ~/.local/bin/dmenu`), it is syntactically equivalent to running `tlaunch <argv[0]> "$@"`. The host canonicalizes `argv[0]` to the target view identifier and delegates directly to the existing CLI argument and query parameter binding pipeline (`bind_invocation_parameters`), requiring no separate CLI engine or competing execution path.
- **CLI Flag to Query Mapping**: Trailing CLI arguments are passed as explicit CLI flags matching fields in `[views.<name>.query]`. Undeclared flags are rejected with a validation error. Interactive text input maps directly to the single field specified by `input = "<field>"`.
- **Contract Inspection**: A dedicated CLI mode (`tlaunch inspect <view>`) outputs the view's query schema, command bindings, and return types, enabling automatic shell completion generation.

### 6. Alignment with M3 Theming (Historical Contract)

The M3 contract below records the original decision. It was superseded on 2026-09-11: the active theme now uses an extensible flat scheme with built-in names such as `accent` and `muted`, literal scheme values, and field-level style merging. See the [current theme specification](../reference/theme-toml.md). The workflow style table nomenclature remains current.

- **M3 Scheme Primacy (superseded)**: Workflows continue to consume Material Design 3 semantic color tokens (`scheme:primary`, `scheme:on-surface-variant`, etc.) in style slot declarations (`[styles.<slot>]`), ensuring out-of-the-box harmony across all user themes without per-workflow styling patches.
- **Theme Table Nomenclature**: Themes directly configure workflow styles under `[workflows.<id>.styles]`.

---

## Consequences

### Positive
- **Frictionless Sharing**: A workflow can be shared as a single text block or Gist. Installation requires only dropping the file into `~/.config/tlaunch/workflows/`.
- **Decoupled Architecture**: Workflows are treated as pure declarative bundles, cleanly separated from host engine mechanics.
- **CWD Predictability**: CLI workflows seamlessly operate on the user's active terminal directory without path-context distortion.
- **Interpreter Versatility**: Inline scripts support arbitrary shebang interpreters with accurate line-number diagnostics.
- **Safe Process Isolation**: No `$PATH` pollution or binary hijacking.
- **Zero Technical Debt**: Clean break avoids compatibility shims, deprecation warnings, and legacy translation overhead.

### Negative
- **Clean Break Requirement**: Existing configurations under `plugins/` must be manually moved to `workflows/` and updated to `[workflow]`. No fallback or automatic migration is provided.
