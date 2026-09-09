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
| **[0001](0001-decentralized-workflow-extensions.md)** | [Decentralized Workflow Extensions Architecture](0001-decentralized-workflow-extensions.md) | **Accepted**; configuration direction superseded by ADR 0002 | 2026-09-06 |
| **[0002](0002-static-configuration-and-script-boundaries.md)** | [Static Configuration and Script Boundaries](0002-static-configuration-and-script-boundaries.md) | **Accepted**; implemented as a clean break | 2026-09-08 |
