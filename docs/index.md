---
okf_version: "0.2"
title: "tlaunch Knowledge Bundle"
description: "Architecture, developer guides, and configuration references for tlaunch"
generated:
  by: "maintainers"
  at: "2026-09-05"
tags:
  - tlaunch
  - terminal
  - workflow
  - rust
---

# tlaunch Documentation

Welcome to the `tlaunch` knowledge base. `tlaunch` is an extensible terminal workflow host where configurations define workflow-owned Views powered by picker, capture, native form, or embedded PTY engines.

This documentation is organized into four core categories following the **Diátaxis** framework, packaged as an **Open Knowledge Format (OKF v0.2)** bundle for both human developers and autonomous AI agents.

## Documentation Index

### 1. [Tutorials](tutorials/index.md)
*Learning-oriented paths for beginners and new contributors.*
- [Getting Started](tutorials/getting-started.md) — Install, configure your first view, and run `tlaunch`.
- [Your First Workflow](tutorials/first-workflow.md) — Create a complete workflow with custom views and items.

### 2. [How-To Guides](how-to/index.md)
*Task-oriented recipes for solving specific practical problems.*

*Engine-specific View configuration:*
- [Picker Views](how-to/picker-views.md) — Configure static items, dynamic feeds, aggregation, and previews.
- [Capture Views](how-to/capture-views.md) — Configure static or script-produced text output.
- [Form Views](how-to/form-views.md) — Generate editable fields from query parameters and use commands to submit values.
- [Embedded Views](how-to/embedded-views.md) — Host interactive terminal programs through a PTY.

*Cross-cutting workflow tasks:*
- [Commands and Producer Scripts](how-to/commands-and-producers.md) — Configure commands and read the shared producer context.
- [View Navigation & Popups](how-to/view-navigation-and-popups.md) — Configure popup modals, view stack transitions, and call/return flows.
- [Custom Themes](how-to/custom-themes.md) — Define flat color schemes and component style overrides.

### 3. [Reference](reference/index.md)
*Authoritative, technical specifications and syntax references.*
- [CLI Reference](reference/cli.md) — Command-line flags, configuration check mode, and direct view invocation.
- [config.toml Specification](reference/config-toml.md) — Root configuration format, theme selection, and session command overrides.
- [Theme TOML Specification](reference/theme-toml.md) — Flat scheme colors, built-in defaults, and style merge rules.
- [workflow.toml Specification](reference/workflow-toml.md) — Workflow manifests, view definitions, query schemas, and engine configuration.
- [Picker Preview](reference/picker-preview.md) — Preview sources, feed ownership, nested documents, scrolling, and limits.
- [Producer Protocol](reference/producer-protocol.md) — Literal configuration boundaries and the version-1 producer request/response contract.

### 4. [Explanation](explanation/index.md)
*Understanding-oriented deep dives into architectural designs and philosophy.*
- [Architecture Overview](explanation/architecture-overview.md) — Domain separation across configuration, session, engine, and rendering.
- [Architecture Convergence](explanation/architecture-convergence.md) - Implementable migration plan for runtime ownership, execution lifecycle, task correlation, and measured scheduling decisions.
- [Input & Navigation Model](explanation/input-and-navigation-model.md) — Route resolution, key binding precedence, and lossless input transport.
- [Runtime Guarantees](explanation/runtime-guarantees.md) — Workflow trust boundary, execution resource budgets, and cleanup invariants.

---

## Architecture Governance

- **[Architecture Decision Records (ADRs)](adr/index.md)** — Chronological log of formal design and architectural choices.
  - [ADR 0001: Decentralized Workflow Extensions](adr/0001-decentralized-workflow-extensions.md) — Decentralized workflow package layout, dual-mode storage, inline scripts, and CLI multiplexing.
  - [ADR 0002: Static Configuration and Script Boundaries](adr/0002-static-configuration-and-script-boundaries.md) — Static configuration, typed operations, restricted data providers, and explicit navigation and return protocols.
  - [ADR 0003: Unified Action Registration and Host-Owned Command Folding](adr/0003-unified-action-registration-and-host-folding.md) — One dispatch registry for View and Engine actions, View-only business-command presentation, and host-owned folding across normal and popup views.
