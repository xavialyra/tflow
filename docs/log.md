# Documentation Changelog

This changelog tracks updates to the `tui-launcher` knowledge bundle.

## 2026-09-09

- Clarified the version-1 producer context contract: aggregate Picker owner commands use the selected feed's independent parameter snapshot, while page commands use the aggregate View parameters; public selection remains under `context.engine.state.item` and feed provenance stays host-owned.
- Added a dynamic-feed how-to for projecting feed-owner commands into an aggregate Picker and reading their script context.

## 2026-09-08

- Implemented ADR 0002's initial producer scope: literal `producer = "declared" | "script"` / `handler` configuration, strict version-1 JSON stdin/stdout protocol, typed command operations, Picker item feeds, Capture output, and post-commit return processors.
- Added producer integration coverage for command stdin, Picker request input, post-mount Capture output, and return processing after caller restoration. Enforced strict producer handler tables, literal handler boundaries, operation matching, output bounds, cancellation, and child reaping.
- Added updated workflow reference, static-values reference, producer-oriented tutorials/how-to guides, and runtime guarantee documentation; ADR 0002 and the architecture convergence plan now identify implemented behavior and remaining deferred capabilities.
- Added accepted ADR 0002, `docs/adr/0002-static-configuration-and-script-boundaries.md`: replace embedded expressions with literal TOML and unified `producer = "declared" | "script"` / `handler` configuration for commands, Picker items, Capture output, and return processors.
- Refined ADR 0002 with typed operation matching, version-1 JSON request/response examples, feed-owner parameter semantics, host-assigned item provenance, and explicit `target`/`query` navigation binding.
- Specified Capture provider execution after mount and return processor execution after restoring the caller, including cancellation, explicit null, absent processors, failures, and stale-result handling. Deferred a general View builder and dynamic Engine configuration; the initial `run` mode is foreground only. Complete schemas and execution-policy details remain implementation follow-up work.
- Marked the decision as not implemented, documented its partial supersession of ADR 0001's expression requirements, and linked it from the ADR registry and root documentation index.
- Completed the clean-break migration: removed the expression evaluator and legacy command, Picker, Capture, and return-handler paths; removed the unused Capture `title` configuration and Engine title control; narrowed Picker providers to explicit requests and cancellation; enforced global command precedence; corrected nested return ownership; and aligned the reference and how-to documentation with the static protocol implementation.
- Removed the remaining theme field aliases and manifest field-discarding path, deleted no-op inline-workflow normalization, removed the top-level selected-item source fallback, and renamed static Engine configuration projection APIs away from expression-evaluation terminology. Documented the child-process environment contract in the CLI reference.

## 2026-09-07

- Corrected the workflow manifest reference: capture views accept required `output`, and `[workflow].api` is optional with a default of `1`.
- Added the ignored Stage 5 scheduler evidence collector and recorded release artifact `target/stage5/scheduler-20260907-092158Z/`: five 40-iteration serialized-worker samples met the initial 250 ms pooled-p95 queue-wait and cancellation-to-reap objectives. The architecture plan retains the serialized worker based on this TaskRuntime/process baseline and explicitly records that private preview and full input/render workload evidence remain outstanding.
- Added `docs/explanation/architecture-convergence.md` as an implementation plan based on commit `ff50f2c` plus the reviewed working tree.
- Recorded current partial protocol foundations and six incremental work areas: foreground execution lifecycle, task correlation, configuration ownership, protocol placement, measured scheduling, and documentation alignment.
- Specified navigation commit sequencing, task correlation ownership, the foreground terminal-handoff state machine, configuration roles, telemetry, service objectives, migration gates, and production-path contract coverage.
- Updated the root and explanation indexes, clarified the architecture overview's implementation-plan link, and corrected runtime safety wording to describe trusted workflow code rather than sandboxing.

## 2026-09-06

- Initialized Architecture Decision Records (ADRs) under `docs/adr/`:
  - Created `docs/adr/index.md` registry.
  - Added `docs/adr/0001-decentralized-workflow-extensions.md` (ADR 0001).
  - Established decentralized workflow layout under `$XDG_CONFIG_HOME/tui-launcher/workflows/`.
  - Defined dual-mode coexistence (single-file `.toml` and directory packages) and inline script execution.
  - Formulated CLI entry multiplexing (`argv[0]`) and inspection.
  - Linked ADR registry to root `docs/index.md` under Architecture Governance.
  - Refined ADR 0001: established host-driven shebang resolution with robust multi-argument tokenization (e.g. `/usr/bin/env -S`) and native `noexec` immunity, temporary script materialization under `$XDG_RUNTIME_DIR` with source attribution comments and semantic file prefixing, enforced caller CWD preservation ($PWD), replaced $PATH prepending with host-side absolute path resolution, restricted single-file workflows from referencing external relative scripts, clarified CLI `argv[0]` multiplexing as direct delegation to existing argument binding pipelines, maintained cache lifecycle simplicity, and reaffirmed a pre-release clean-break policy without legacy shims.
- Refactored theme configuration schema and updated `custom-themes.md`:
  - Realigned domain boundaries by moving query prefix styling from `[chrome.input_prefix]` to `[picker.input_prefix]`.
  - Added dedicated styling slots: `[chrome.footer_title]`, `[chrome.footer_status]`, and `[chrome.border]`.
  - Rewrote `docs/how-to/custom-themes.md` to document structured theme tables and remove obsolete/non-existent binding references.
- Implemented ADR 0001 (Decentralized Workflow Extensions) and updated documentation:
  - Transitioned domain from plugins to workflows: replaced `plugins/` with `workflows/`, and `[plugin]` with `[workflow]`.
  - Implemented dual-mode workflow loading: single-file `.toml` and directory packages (`workflow.toml`).
  - Added strict conflict detection for duplicate workflow IDs and duplicate view aliases.
  - Implemented caller CWD preservation ($PWD) and `$WORKFLOW_DIR` environment injection for directory workflows.
  - Implemented multi-line inline script materialization with `0600` permissions, source attribution, and host-side shebang tokenization.
  - Added CLI `argv[0]` multiplexing and contract inspection (`inspect <view>` / `--inspect <view>`).
  - Renamed `docs/tutorials/first-plugin.md` -> `first-workflow.md` and `docs/reference/plugin-toml.md` -> `workflow-toml.md`.
  - Updated `docs/index.md`, `docs/tutorials/index.md`, `docs/tutorials/getting-started.md`, `docs/reference/index.md`, `docs/reference/config-toml.md`, `docs/reference/cli.md`, `docs/how-to/index.md`, `docs/how-to/dynamic-picker-feeds.md`, `docs/explanation/architecture-overview.md`, `docs/explanation/input-and-navigation-model.md`, and `docs/explanation/runtime-guarantees.md` to reflect workflow terminology and contracts.

## 2026-09-05

- Initialized OKF v0.2 knowledge bundle and restructured documentation under Diátaxis framework:
  - Created root `index.md` and reserved `log.md`.
  - Added Tutorials: `getting-started.md`, `first-plugin.md`.
  - Added How-To Guides: `dynamic-picker-feeds.md`, `view-navigation-and-popups.md`, `custom-themes.md`, `embedded-pty-views.md`.
  - Added Reference: `cli.md`, `config-toml.md`, `plugin-toml.md`, `expressions.md`.
  - Added Explanation: `architecture-overview.md`, `input-and-navigation-model.md`, `runtime-guarantees.md`.
  - Replaced monolithic `architecture.md` and `input-navigation.md`.
  - Streamlined root `README.md` to lean quick-start and doc pointers.
