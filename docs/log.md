# Documentation Changelog

This changelog tracks updates to the `tui-launcher` knowledge bundle.

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

## 2026-09-05

- Initialized OKF v0.2 knowledge bundle and restructured documentation under Diátaxis framework:
  - Created root `index.md` and reserved `log.md`.
  - Added Tutorials: `getting-started.md`, `first-plugin.md`.
  - Added How-To Guides: `dynamic-picker-feeds.md`, `view-navigation-and-popups.md`, `custom-themes.md`, `embedded-pty-views.md`.
  - Added Reference: `cli.md`, `config-toml.md`, `plugin-toml.md`, `expressions.md`.
  - Added Explanation: `architecture-overview.md`, `input-and-navigation-model.md`, `runtime-guarantees.md`.
  - Replaced monolithic `architecture.md` and `input-navigation.md`.
  - Streamlined root `README.md` to lean quick-start and doc pointers.
