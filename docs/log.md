# Documentation Changelog

This changelog tracks updates to the `tlaunch` knowledge bundle.

## 2026-09-16

- Refined ADR 0004 (`docs/adr/0004-scoped-command-registration.md`): renamed `CommandHandler::View` to `CommandHandler::Event` to precisely represent the event-driven dispatch model (`Action` closure execution vs `Event` message dispatch to `View::on_command(&mut self, id, context)`); registered `FormView`'s discrete navigation bindings (`Tab` / `Down` -> `form.focus_next`, `BackTab` / `Up` -> `form.focus_prev`, `Escape` -> `form.cancel`, `Ctrl-C`/`Ctrl-D` -> `form.exit`) as discrete `CommandScope::Engine` Event commands; and restricted `FallbackInputReceiver` strictly to continuous, free-form text typing and inline buffer editing.
- Updated ADR 0004 (`docs/adr/0004-scoped-command-registration.md`): specified `CommandRegistry` as the exclusive input forwarding channel for all keyboard interactions; introduced the **Hybrid Command Target Model** (`CommandHandler::Action` vs `CommandHandler::Event`) allowing stateful View commands to mutate `&mut self` without lock overhead; migrated `FormView`, `PickerProtocolView`, `CaptureProtocolView`, and `EmbeddedProtocolView` to register all discrete keybindings into `CommandScope::Engine`; introduced the `FallbackInputReceiver` trait contract for unmapped raw input streams (Embedded PTY byte streams, Picker/Form text typing and editing); and defined active `Enter` resolution across `View > Engine > Host` precedence in the Chrome Footer.

## 2026-09-13

- Refined ADR 0004 with stable CommandRef and scope generations, deterministic same-scope conflict handling, unified enabled-state queries, and explicit dispatcher/task boundaries.

- Added accepted ADR 0004, `docs/adr/0004-scoped-command-registration.md`: use one scoped command registry with explicit precedence; register Host, Engine, and View commands dynamically; keep callback context with the registering component; and dispatch input directly from key resolution to callback.

- Updated ADR 0003, the configuration reference, input/navigation model, and architecture overview: the command palette is specified as a built-in Popup Picker View opened through the Router with command descriptors passed as navigation parameters. Host-owned folding remains the only command-palette policy; the Session must not maintain a parallel Picker implementation.

## 2026-09-12

- Simplified the native Form Engine by removing field help text, the form help theme slot, and the native-form help popup fixture. Validation errors remain inline; updated the form and theme references and native form guide.

- Added accepted ADR 0003, `docs/adr/0003-unified-action-registration-and-host-folding.md`: register View and Engine actions through one dispatch model, keep Engine actions out of business-command presentation, and make command folding and the Ctrl-K opener host-owned and shared by normal and popup rendering.

## 2026-09-11

- Added the native Form Engine reference and guide: fixed query inputs, declared/script content producers, editable drafts and typed state, command-controlled submission, keyboard behavior, theme slots, and runnable object/string convention examples. Updated workflow, producer, theme, and documentation indexes.

- Fixed the Embedded form example's handling of empty and partial routed input, documented the distinction between raw route text and rendered CLI/call parameters, and added default-page route regression coverage.

- Added a runnable Embedded form workflow and guide covering initial scalar parameters, validation, JSON submission, Esc cancellation, and caller return processing. Clarified the separate stdout result pipe and PTY display streams in the Embedded guide; added PTY integration coverage for direct and nested invocation.

- Preserved the terminal theme's unhighlighted selected-row background and yellow-on-black input prefixes and shortcuts. Separated their black `surface` from `selection` / `on-selection`, which now default to terminal reset, and synchronized theme reference and guide descriptions.

- Made preview available in every Picker, initially collapsed with a default `Ctrl+P` toggle and no visibility setting. Documented built-in display/value details, optional page and feed content overrides, absent-provider fallback, empty/error behavior, and suspended script/image work while collapsed.

- Removed the legacy `[picker.badge_selected]` theme table and its override precedence. Selected badges now use only `[picker.badge.selected]`; documented rejection of the old entry and retained field-level selected-style merging.

- Removed legacy Picker `preview.blocks` configuration, JSON Pointer metadata projection, the parallel block renderer, and source-feed path exceptions. Preview sources are scripts, declared documents, or inherited providers; relative paths consistently follow the provider owner. Updated the preview reference, Picker guide, workflow/producer specifications, indexes, and ADR 0002.

- Marked ADR 0001's original M3 theming contract as historical and superseded by the flat scheme specification, preserving the original decision and updating the ADR registry.

- Clarified that theme color literals and references preserve outer whitespace trimming, while scheme map keys remain exact. Added regression coverage for non-ASCII hex rejection and black accent text on light backgrounds.

- Simplified themes to a flat, extensible scheme of ANSI or RGB literals and field-level component overrides. Removed palette configuration and fixed role names; documented baseline merging before color resolution, explicit false/reset overrides, selected workflow slot inheritance, and invalid-reference errors. Added the theme specification, migrated the custom theme guide and workflow references, and updated the documentation indexes.

## 2026-09-10

- Added Picker preview source/document reference, script protocol, inherited feed ownership and page overrides, nested styled content, host scrolling, runtime worker isolation, validation limits, and a runnable mixed image/text fixture. Updated the Picker how-to and ADR 0002 to distinguish static outer panes from producer-supplied internal documents.

- Documented the 3-second automatic expiration of INFO feedback in the footer and popup bottom border, including idle refresh, replacement timing, early input/navigation dismissal, and retained logs.

- Documented optional `run.success_message` for host-generated INFO feedback after successful external commands, including clipboard workflows and immediate-exit logging.

- Added `tlaunch inspect --all` to export stable JSON contracts for every configured View, including alias, Engine, query, and command metadata.

## 2026-09-09

- Renamed the product, crate, binary, configuration paths, runtime prefixes, and CLI environment variable to `tlaunch` / `TLAUNCH_CONFIG`.
- Clarified the version-1 producer context contract: aggregate Picker owner commands use the selected feed's independent parameter snapshot, while page commands use the aggregate View parameters; public selection remains under `context.engine.state.item` and feed provenance stays host-owned.
- Reorganized task-oriented guides by Engine: Picker, Capture, and Embedded Views now have separate configuration guides; cross-Engine command and producer behavior is documented separately.
- Renamed the current static-values/runtime-data reference to `producer-protocol.md` and made the producer contract the sole reference entry point.

## 2026-09-08

- Implemented ADR 0002's initial producer scope: literal `producer = "declared" | "script"` / `handler` configuration, strict version-1 JSON stdin/stdout protocol, typed command operations, Picker item feeds, Capture output, and post-commit return processors.
- Added producer integration coverage for command stdin, Picker request input, post-mount Capture output, and return processing after caller restoration. Enforced strict producer handler tables, literal handler boundaries, operation matching, output bounds, cancellation, and child reaping.
- Added updated workflow reference, static-values reference, producer-oriented tutorials/how-to guides, and runtime guarantee documentation; ADR 0002 and the architecture convergence plan now identify implemented behavior and remaining deferred capabilities.
- Added accepted ADR 0002, `docs/adr/0002-static-configuration-and-script-boundaries.md`: define static TOML and unified `producer = "declared" | "script"` / `handler` configuration for commands, Picker items, Capture output, and return processors.
- Refined ADR 0002 with typed operation matching, version-1 JSON request/response examples, feed-owner parameter semantics, host-assigned item provenance, and explicit `target`/`query` navigation binding.
- Specified Capture provider execution after mount and return processor execution after restoring the caller, including cancellation, explicit null, absent processors, failures, and stale-result handling. Deferred a general View builder and dynamic Engine configuration; the initial `run` mode is foreground only. Complete schemas and execution-policy details remain implementation follow-up work.
- Marked the decision as not implemented, documented its partial supersession of ADR 0001's earlier configuration direction, and linked it from the ADR registry and root documentation index.
- Completed the clean-break migration: removed legacy command, Picker, Capture, and return-handler paths; removed the unused Capture `title` configuration and Engine title control; narrowed Picker providers to explicit requests and cancellation; enforced global command precedence; corrected nested return ownership; and aligned the reference and how-to documentation with the static protocol implementation.
- Removed the remaining theme field aliases and manifest field-discarding path, deleted no-op inline-workflow normalization, removed the top-level selected-item source fallback, and aligned static Engine configuration projection APIs with the producer architecture. Documented the child-process environment contract in the CLI reference.

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
  - Established decentralized workflow layout under `$XDG_CONFIG_HOME/tlaunch/workflows/`.
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
  - Updated `docs/index.md`, `docs/tutorials/index.md`, `docs/tutorials/getting-started.md`, `docs/reference/index.md`, `docs/reference/config-toml.md`, `docs/reference/cli.md`, `docs/how-to/index.md`, `docs/how-to/picker-views.md`, `docs/explanation/architecture-overview.md`, `docs/explanation/input-and-navigation-model.md`, and `docs/explanation/runtime-guarantees.md` to reflect workflow terminology and contracts.

## 2026-09-05

- Initialized OKF v0.2 knowledge bundle and restructured documentation under Diátaxis framework:
  - Created root `index.md` and reserved `log.md`.
  - Added Tutorials: `getting-started.md`, `first-plugin.md`.
  - Added How-To Guides: `picker-views.md`, `view-navigation-and-popups.md`, `custom-themes.md`, `embedded-views.md`.
  - Added Reference: `cli.md`, `config-toml.md`, `workflow-toml.md`, `producer-protocol.md`.
  - Added Explanation: `architecture-overview.md`, `input-and-navigation-model.md`, `runtime-guarantees.md`.
  - Replaced monolithic `architecture.md` and `input-navigation.md`.
  - Streamlined root `README.md` to lean quick-start and doc pointers.
