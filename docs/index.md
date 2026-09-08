---
okf_version: "0.2"
title: "tui-launcher Knowledge Bundle"
description: "Architecture, developer guides, and configuration references for tui-launcher"
generated:
  by: "maintainers"
  at: "2026-09-05"
tags:
  - tui-launcher
  - terminal
  - workflow
  - rust
---

# tui-launcher Documentation

Welcome to the `tui-launcher` knowledge base. `tui-launcher` is an extensible terminal workflow host where configurations define workflow-owned Views powered by picker, capture, or embedded PTY engines.

This documentation is organized into four core categories following the **Diátaxis** framework, packaged as an **Open Knowledge Format (OKF v0.2)** bundle for both human developers and autonomous AI agents.

## Documentation Index

### 1. [Tutorials](tutorials/index.md)
*Learning-oriented paths for beginners and new contributors.*
- [Getting Started](tutorials/getting-started.md) — Install, configure your first view, and run `tui-launcher`.
- [Your First Workflow](tutorials/first-workflow.md) — Create a complete workflow with custom views and items.

### 2. [How-To Guides](how-to/index.md)
*Task-oriented recipes for solving specific practical problems.*
- [Dynamic Picker Feeds](how-to/dynamic-picker-feeds.md) — Connect scripts to stream items and dynamic preview panes.
- [View Navigation & Popups](how-to/view-navigation-and-popups.md) — Configure popup modals, view stack transitions, and call/return flows.
- [Custom Themes](how-to/custom-themes.md) — Define brand palettes, color schemes, and element bindings.
- [Embedded PTY Views](how-to/embedded-pty-views.md) — Host interactive terminal programs and process PTY output.

### 3. [Reference](reference/index.md)
*Authoritative, technical specifications and syntax references.*
- [CLI Reference](reference/cli.md) — Command-line flags, configuration check mode, and direct view invocation.
- [config.toml Specification](reference/config-toml.md) — Root configuration format, theme selection, and session command overrides.
- [workflow.toml Specification](reference/workflow-toml.md) — Workflow manifests, view definitions, query schemas, and engine configuration.
- [Static Values and Runtime Data](reference/expressions.md) — Literal configuration boundaries and the explicit producer protocol for runtime data.

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
