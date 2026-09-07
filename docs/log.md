# Documentation Changelog

This changelog tracks updates to the `tui-launcher` knowledge bundle.

## 2026-09-07

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
