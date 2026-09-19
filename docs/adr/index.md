---
title: "Architecture Decision Records"
type: "reference"
tags:
  - adr
  - architecture
  - governance
description: "Index of formal architecture decision records (ADRs) tracking design choices and system evolution."
---

# Architecture Decision Records (ADRs)

This directory maintains the immutable, chronological record of architectural and design choices for `tlaunch`.

Each record captures the context, considered options, decision outcome, and consequences at a specific point in time. Records transition through explicit statuses: `Proposed` -> `Accepted` -> `Superseded`.

## Decision Registry

| Number | Title | Status | Date |
| :--- | :--- | :--- | :--- |
| **[0001](0001-decentralized-workflow-extensions.md)** | [Decentralized Workflow Extensions Architecture](0001-decentralized-workflow-extensions.md) | **Accepted**; configuration direction superseded by ADR 0002; M3 theming superseded by the [flat scheme specification](../reference/theme-toml.md) | 2026-09-06 |
| **[0002](0002-static-configuration-and-script-boundaries.md)** | [Static Configuration and Script Boundaries](0002-static-configuration-and-script-boundaries.md) | **Accepted**; implemented as a clean break | 2026-09-08 |
| **[0003](0003-unified-action-registration-and-host-folding.md)** | [Unified Action Registration and Host-Owned Command Folding](0003-unified-action-registration-and-host-folding.md) | **Accepted**; partially superseded by ADR 0004 for command registration and dispatch | 2026-09-12 |
| **[0004](0004-scoped-command-registration.md)** | [Scoped Command Registration](0004-scoped-command-registration.md) | **Accepted** | 2026-09-13 |
| **[0005](0005-manifest-driven-suites-and-self-contained-workflows.md)** | [Manifest-Driven Workflow Suites and Self-Contained Workflows](0005-manifest-driven-suites-and-self-contained-workflows.md) | **Accepted**; supersedes ADR 0001 alias registration and multi-workflow discovery | 2026-09-19 |
| **[0006](0006-feed-removal-workflow-scoped-commands-and-item-bindings.md)** | [Feed Removal, Workflow-Scoped Commands, and Item-Driven Bindings](0006-feed-removal-workflow-scoped-commands-and-item-bindings.md) | **Accepted**; supersedes ADR 0002 Picker feed aggregation and command projection | 2026-09-19 |
