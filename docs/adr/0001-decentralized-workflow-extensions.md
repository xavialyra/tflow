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

* **Status**: Accepted
* **Date**: 2026-09-06
* **Scope**: `workflow/config`, `app/cli`, `ui/theme`

---

## Context

`tui-launcher` is an extensible terminal workflow host designed for keyboard-driven navigation and command orchestration. In the host runtime, views are governed by explicit query schemas, structured view stacks (with modal popup and call/return boundaries), and bounded expression evaluation.

Historically, external views and commands were bundled as "plugins" located strictly in subdirectories under `$XDG_CONFIG_HOME/tui-launcher/plugins/<id>/plugin.toml`. As adoption patterns matured, several architectural tensions emerged:

1. **Category Mismatch**: Units previously modeled as "plugins" are strictly declarative workflow specifications (view definitions, routing aliases, query schemas, and command triggers) rather than invasive host plugins.
2. **High Sharing Friction**: Requiring a multi-level subdirectory and companion physical script files creates friction for terminal enthusiasts who share standalone recipes across Gists, dotfiles, or discussion forums.
3. **Packaging Over-Engineering**: Terminal power users prioritize decentralized, dotfiles-friendly, plain-text configuration over centralized package managers, registries, or opaque download tooling.

---

## Decision Drivers

* **Unix Philosophy**: Everything is a file; plain text is the universal interface; conventions over complex configuration registries.
* **Frictionless Sharing**: A workflow should be capable of existing as a self-contained, single-file document that can be installed via a direct copy or `curl`.
* **Orthogonal Namespaces**: Workflows operate as peers without imperative execution sequence or heuristic priority rules.
* **Preservation of Core Guarantees**: Retain the host's strict schema verification, bounded expression evaluation, and Material Design 3 (M3) semantic styling contract.

---

## Architectural Decisions

### 1. Domain Terminology and Storage Layout: Transition to `workflows/`

- **Nomenclature**: The term `plugin` is replaced by **`workflow`**. Manifest files represent declarative workflow packages rather than internal engine plugins.
- **Directory Convention**: Workflows reside in `$XDG_CONFIG_HOME/tui-launcher/workflows/`. The legacy `.d` directory suffix is intentionally omitted in favor of a clean, standard plural collection directory.
- **Deterministic Namespace Mapping**: The workflow namespace ID is directly derived from the file stem or directory name. No ordering prefixes are used; all workflows exist as orthogonal peers.
- **Explicit Conflict Rejection**: If duplicate workflow IDs or duplicate view aliases are detected across manifests, the configuration compiler aborts with an explicit error. Silent shadowing or heuristic precedence overrides are prohibited.

### 2. Dual-Mode Workflow Layout (Single File and Directory Coexistence)

The loader scans `workflows/` and uniformly resolves two physical layouts:

1. **Single-File Workflows (`workflows/<id>.toml`)**:
   - Primary vehicle for distribution and lightweight recipes.
   - Self-contained definitions invoking system binaries or inline scripts.
   - Namespace resolves to `<id>`.
2. **Directory Workflows (`workflows/<id>/workflow.toml`)**:
   - Suited for complex workflows requiring private test suites, multi-file Python/Bash scripts, or local static assets.
   - Namespace resolves to `<id>`.
3. **Subprocess `$PATH` Prepending**:
   - For directory workflows, the host automatically prepends `workflows/<id>/scripts/` to the child process `$PATH` during execution. Scripts and commands can invoke companion executables by bare name (e.g., `script = "checkout.sh"`) without relative path resolution.

### 3. Native Multi-Line Inline Scripts

Command specifications support inline script bodies alongside external file paths:

```toml
[views.main.commands.checkout]
key = "enter"
type = "run"
script = """
#!/usr/bin/env bash
set -euo pipefail
target="$1"
git checkout "$target"
"""
```

Inline scripts are piped directly to the configured shell by the execution engine, eliminating mandatory companion `.sh` files for concise logic.

### 4. Multiplexed CLI Entry (`argv[0]`) & Schema Inspection

- **Symlink Multiplexing**: If the launcher binary is executed under an `argv[0]` alias matching a configured view (e.g. via `ln -s tui-launcher ~/.local/bin/dmenu`), it routes directly to that view. The view's `query` schema parses and validates trailing command-line flags.
- **Contract Inspection**: A dedicated CLI mode (`tui-launcher inspect <view>`) outputs the view's query schema, command bindings, and return types, enabling automatic shell completion generation.

### 5. Alignment with M3 Theming and Diagnostic Checks

- **M3 Scheme Primacy**: Workflows continue to consume Material Design 3 semantic color tokens (`scheme:primary`, `scheme:on-surface-variant`, etc.) in style slot declarations (`[styles.<slot>]`), ensuring out-of-the-box harmony across all user themes without per-workflow styling patches.
- **Theme Table Compatibility**: Themes support `[workflows.<id>.styles]` as a first-class section, while preserving `[plugins.<id>.styles]` as a backwards-compatible alias.
- **Preflight Diagnostics**: Workflows can declare required dependencies via `[workflow.requires] binaries = [...]`. `tui-launcher --check` statically verifies their availability in `$PATH`.

---

## Consequences

### Positive
- **Frictionless Sharing**: A workflow can be shared as a single text block or Gist. Installation requires only dropping the file into `~/.config/tui-launcher/workflows/`.
- **Decoupled Architecture**: Workflows are treated as pure declarative bundles, cleanly separated from host engine mechanics.
- **Dotfiles Native**: Clean alignment with Git submodules, Chezmoi, and GNU Stow without file-tree conflicts.
- **CLI Versatility**: Turns any declared view into a first-class standalone CLI tool via `argv[0]` symlinks.

### Negative & Mitigations
- **Configuration Migration**: Existing plugin directories require renaming from `plugins/` to `workflows/`.
  - *Mitigation*: The configuration loader will accept `plugins/` as a fallback when `workflows/` is absent, warning the user of deprecation.
