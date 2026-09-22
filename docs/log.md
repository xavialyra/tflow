# Documentation Changelog

This changelog tracks updates to the `tflow` knowledge bundle.

## 2026-09-22

- Generalized the navigation handoff from "a replaced base View" to the topmost newly mounted instance. A freshly pushed popup now participates in the same readiness-driven grace: while its `publication.ready` is false, the surface underneath is kept and the popup's border is not drawn, so it appears fully formed when it publishes instead of flashing a cleared empty box and a blanked app footer. A declared popup still appears immediately (no publication means ready), and a hung one still falls back to its own loading frame when the existing 150 ms window expires. The footer is covered by the same decision: during grace the settled status and commands are carried over while location and live error/info notifications always track the current frame, so a route switch no longer blanks `x of y` before the new Picker publishes. `settle` still never captures a popup frame, so returning to the base cannot resurrect overlay pixels, and the retained record now also stores the footer model the settled frame rendered. Unit tests cover the carried footer status and the deferred loading popup.

- Removed the route-completion how-to and every host-documentation reference to it. Route completion is a workflow recipe built from host primitives, not a host feature, so the bundle no longer documents or points at it: the deleted `how-to/route-completion.md`, the picker-views aggregation example that used it, and the `core:route_separator` pointers in the settings and workflow references are gone. The `left_prefix`, `show_left_prefix`, `left_prefix_backspace`, and `clear_input` reference entries remain as host-owned behavior with the route-recipe framing dropped.
- The Picker preview caches its rendered document by preview provider for the session, so a View remounted by navigation or `replace = true` (multi-select toggles, `__focus` restoration) paints the document the previous instance already rendered instead of flashing `Loading preview…` while the script re-runs. The key is the resolved preview `owner`, not the full request identity: a self-navigation exists precisely to update parameters (a Picker's selected set, for instance), which are part of the identity, so an identity key would always miss the remount it is meant to cover. The cache is bounded to 32 providers and shared by every Picker the session's View factory mounts; only script previews use it, because declared and inherited documents already install synchronously. A cached install defers image decoding to the post-commit `start`, so the input path still starts no tasks. Unit tests cover the immediate render across a changed identity and the deferral, and `preview_correlation_tests` drives two real Picker mounts through a parameter-updating request and a hung refresh script to prove the second instance's first frame comes from the cache.
- Workflow packages no longer hardcode the member id they are mounted as. `TFLOW_BIN` is now published before headless dispatch, so a producer can re-enter the host in `--check`, `--inspect`, and `--items` mode instead of guessing a `target/debug` checkout; the development fixture and the installed workflow set resolve their own View references from `TFLOW_WORKFLOW_DIR` and `TFLOW_SUITE` (matching the manifest entry whose target is the package root), and every workflow script now reads the product-namespaced `TFLOW_*` variables and `tflow` state/cache paths. The core aggregate producer also resolves sibling members through the suite manifest instead of assuming a directory name.
- Renamed the child-injected environment variables from `WORKFLOW_DIR` to `TFLOW_WORKFLOW_DIR` and from `LAUNCHER_INPUT` to `TFLOW_INPUT`. They were the only host-owned variables without the product namespace, while child processes inherit the caller's environment unchanged; a caller variable with the legacy name could therefore leak into a single-file workflow child or a non-Embedded script, and the pair was inconsistent with the branded `TFLOW_SUITE`, `TFLOW_SETTINGS`, and `TFLOW_BIN`. The setup wizard, the reference and how-to guides, the launcher and wizard tests, and the fixture scripts now use the prefixed names.
- Added the per-View Picker engine option `input_placeholder` (unset by default). It renders a literal, muted hint in the query row while the input is empty, after any `[defaults.picker] left_prefix` marker and with the pseudo-cursor kept visible in the input's first cell. It is presentation only: it never enters the query, the editor buffer, or the published input state, and it is clipped to the available width. The hint uses the new `[picker.placeholder]` theme slot. The shared development fixtures declare it on `core:default` and `apps:main`, and `tests/launcher.rs` covers the rendered hint, the visible cursor, its replacement by typing, and the pushed-View prefix combination.
- Centralized the product name and every process-boundary name in `src/identity.rs`: the XDG config, state, cache, and runtime directory basenames, the sandbox and temporary-file prefixes, the materialized-script attribution comment, the terminal requirement error, and the `TFLOW_SUITE`, `TFLOW_SETTINGS`, `TFLOW_BIN`, `TFLOW_WORKFLOW_DIR`, and `TFLOW_INPUT` environment variables. Product-branded names derive from `CARGO_PKG_NAME`, so the next rename edits one constant instead of five domains, and a unit test pins the `TFLOW_*` prefix to the product name. The Picker preview defaults are now named (`DEFAULT_PREVIEW_RATIO`, `DEFAULT_PREVIEW_MIN_WIDTH`) instead of repeating `0.35` and `24`.
- Fixed the setup wizard (`distribution/init.toml`), which generated every user configuration and could only ever install stale workflows. Its cache marker latched on the first copy, so rerunning the wizard reinstalled the same snapshot no matter how often the selection changed; the marker now records a fingerprint of the workflow source (path, newest modification, file count, package list) and the cache is rebuilt whenever that changes.
- The wizard installs by replacing each package directory instead of merging into it. A merged copy kept every file the source had dropped — a dead `items.sh`, an old `items.py` without the source badge — so a reinstall never repaired an install.
- The generated suite manifest is now TOML 1.0: the entry point's `query` is written as a nested `[suite.entrypoint.query]` table instead of a multi-line inline table, which strict parsers reject. The generated `settings.toml` also carries the picker display defaults (`left_prefix`, `left_prefix_backspace`), and an existing settings file is patched with those two keys when they are absent.
- The wizard no longer needs environment setup to find the workflows: `TFLOW_WORKFLOWS_BOOTSTRAP_DIR` (or `TFLOW_WORKFLOWS_DIR`) wins, and a bundled `workflows/` beside the distribution or a sibling `tflow-workflows/` checkout is discovered by walking up from `TFLOW_WORKFLOW_DIR`. It reports the resolved source and cache in its summary, and the suite target is read from the core package's `[workflow].entrypoint` rather than assuming `core:main`.
- The wizard's install step now fails loudly when the source cannot be found, reports components that were selected but are absent from the source, removes deselected components from the installed tree, keeps bytecode out of the install, and runs `--check` against what it wrote, reporting the result in its summary.
- Added `tests/wizard.rs`: it drives the wizard's embedded scripts against a private copy of the workflow source and pins the two regressions (a cache that lags its source, a merge copy that keeps deleted files), plus the generated manifest, settings and the shipped wizard's own `--check`.
- Renamed the product from `tlaunch` to `tflow`: the crate and binary, the CLI environment variables (`TFLOW_SUITE`, `TFLOW_SETTINGS`, `TFLOW_BIN`, `TFLOW_WORKFLOWS_BOOTSTRAP_DIR`, `TFLOW_WORKFLOWS_DIR`), the configuration, state, and cache paths (`$XDG_CONFIG_HOME/tflow`, `$XDG_STATE_HOME/tflow/runtime.jsonl`, `$XDG_CACHE_HOME/tflow`), and the runtime script, sandbox, and temporary-directory prefixes, plus every tutorial, how-to, reference, ADR, and the setup wizard. The pre-release policy applies: no aliases, shims, or legacy paths are provided, so an existing `~/.config/tlaunch` installation and any user script reading `TLAUNCH_*` must be recreated. (The 2026-09-09 entry below keeps the name that was current then.)

## 2026-09-21

- Repaired the onboarding tutorials and the reference: `[views.<name>.commands.<id>]` was still taught throughout, but the loader has rejected it since ADR 0006 in favour of workflow-root `[commands.<id>]` plus a `[views.<name>.keymap]` binding. Every tutorial example is now validated with `--check` and `--items`, and the stale `scope = "view"` advice is gone.
- Added a route-completion how-to (removed on 2026-09-22): the aggregate View base keymap, the `navigate`-only completion command, the popup View, the shared `--inspect --all` candidate lookup, the Space separator, and the optional `left_prefix` prompt. It documents the behaviour that disappeared from the engine when route entry moved into the workflow.
- Described the current aggregate binding model in the reference and how-tos. Item `bindings` (not command projection) carry a source View's commands into an aggregate Picker, `context.parameters` stays the aggregate View's, and `context.engine.state.item` remains the normalized selection.
- `mode = "item"` Views may now declare their own keymap as a base layer; the focused item's `bindings` override it per physical key (precedence: item → View base → Engine). Previously the two modes were mutually exclusive and only the selected item could bind a key, so a View whose list filtered down to nothing lost its own commands — route completion included. `--check` accepts the same table in both modes, `--inspect` reports it, and the development fixture now declares its Tab/Space route commands in `[views.default.keymap]` instead of injecting them into every item.
- Removed the ignored `stage_5_scheduler_evidence` crate-unit entrypoint and the `src/task/stage5_evidence.rs` collector it drove, so the suite has no ignored tests. The recorded Stage 5 baseline in the architecture plan still stands, and the plan now states that reproducing it needs an equivalent harness instead of documenting a command that no longer exists.
- A producer that closes its request pipe is no longer a failure. The bounded runner treated a broken pipe while writing the request as fatal, so an inline producer that answers without reading stdin (``printf``-style scripts) could fail spuriously when it exited before the host finished writing. The runner now stops writing and still collects the producer's output. This removes the intermittent `form` and preview test failures.
- Fixed route completion in the development fixture when the query matches no aggregated item.
- Consolidated route resolution. `RouteTarget` and `RouteDisplay` were both just "a canonical View reference plus its optional suite alias", so they were replaced by the existing `ViewLocation`, and the `workflow/navigation` module (`Router` label map plus its own selector resolver, which duplicated `CompiledConfig::resolve_view`) was deleted. `RouteCatalog::resolve` now returns `Option<ViewLocation>` through the config's public-alias lookup, so `ViewServices.routes` and the footer read one type. The architecture overview and input/navigation model were updated, and the latter no longer claims an engine-side prefix-routing feature.
- Added the opt-in `[defaults.picker] left_prefix_backspace` setting. Backspace on an empty, prefixed input line now does nothing unless it is configured: `"parent"` returns to the parent View (like Escape) and `"root"` returns to the root View in one step. It only applies while a left prefix is rendered, so `show_left_prefix = false` also disables it for that View. The development fixture sets `"root"` to keep the previous behavior.
- Removed all route logic from the Picker engine. Route-aware input (selector-plus-whitespace submission, route-prefix highlighting, and the route catalogs used to resolve selectors and schemas) is gone; `Key::Char` now always inserts a character. The engine keeps only presentational left-marker rendering.
- Unified the display settings: the global `route_prefix` / `parent_hint` booleans and the per-view `show_route_prefix` option were replaced by one presentational concept. `[defaults.picker] left_prefix` is unset (no marker), `"$route"` (the target View's label/alias), or any literal string; the per-view Picker engine option `show_left_prefix` (boolean, default `true`) hides it. It renders only on non-root Pickers, and Backspace still navigates only while a marker is actually rendered.
- Re-implemented the legacy "selector + space jumps" behavior entirely in the workflow. The development fixture's `core` workflow binds `space` to `core:route_separator`; the script resolves the alias from `completion_routes.py` and either navigates (`clear_input`) or re-mounts the entry View with a literal space appended (`replace`, carrying the current parameters and `__focus`).
- Route completion now consumes the caller's input: the fixture's Tab command returns `navigate` with `clear_input = true`, so a half-typed route is not resurrected when a completion jump is undone with Backspace. Added the optional `clear_input` field to the `navigate` operation; the host clears the source Picker's editor before applying the transition.
- Picker Backspace only navigates while a left prefix is actually rendered; a prefix-less View (including a `show_left_prefix = false` popup) never returns. The old unconditional return-to-root is now the opt-in `left_prefix_backspace = "root"`.
- Widened the fixture's route-completion candidates to every aliased route in the suite (`--inspect --all`, excluding `core:`), instead of only the entry point's aggregate `sources`. Alias-less derived Views such as `sys:output` stay out, so a typed alias still resolves to a single route.
- Removed the Picker engine's built-in route-completion overlay (Tab open/accept, Up/Down cycling, and its in-body modal rendering). The editor now consumes Tab as an ordinary unbound key, and a View can bind `tab` to its own command or open a popup View instead.
- Documented the reserved `__focus` / `__engine = { focus = "..." }` query key used to restore a Picker's selection after a `replace`, in the workflow reference and the navigation/popups guide.

## 2026-09-21

- Corrected headless CLI dispatch: removed the ambiguous positional `tflow inspect ...` / `tflow items ...` subcommands, which collided with View selectors, in favor of the existing `--inspect` and `--items` long options.
- Made `--inspect` accept an optional value and made `--all` self-sufficient: `tflow --inspect`, `tflow --all`, and `tflow --inspect --all` all dump every configured View, while `tflow --inspect <VIEW>` dumps one. No migration-style error is emitted for the removed `tflow inspect --all` form.
- Extended `argv[0]` multiplexing suppression to `--all` and `--inspect=<VIEW>` / `--items=<VIEW>` forms so headless modes are never rewritten as View launches.

## 2026-09-19

- Added proposed ADR 0006 (`docs/adr/0006-feed-removal-workflow-scoped-commands-and-item-bindings.md`):
  - Proposed complete abolition of legacy Picker feeds (`[[views.<name>.engine.config.feeds]]`), command projection, hidden item provenance, and internal `snapshot.owner_view` tracking.
  - Formulated promotion of business commands from View tables to Workflow root level (`[commands.<id>]`), establishing Suite-level fully qualified command identifiers (`<workflow>:<command>`).
  - Specified explicit binary View binding strategies (`mode = "static"` vs `mode = "item"`), with `mode = "static"` as the default for single-purpose workflows.
  - Specified zero-lock dispatch-time late-binding for dynamic `mode = "item"` views, completely bypassing `CommandRegistry` write locks during cursor navigation.
  - Decomposed headless aggregation into two orthogonal, non-mutating query interfaces: `tflow --items` (strict JSON array data stream with error propagation) and `tflow --inspect` (metadata/command contract inspection), leaving combinator logic explicitly to user scripts.
  - Added query parameter injection for `[suite.entrypoint]` and sandboxed child process environment inheritance via `$TFLOW_SUITE`.
  - Registered ADR 0006 as `Proposed` pending implementation milestones.

## 2026-09-19

- Removed redundant fixture trees `tests/fixtures/native-form/` and `tests/fixtures/preview/`; consolidated documentation and integration tests on the canonical `tests/fixtures/config/` suite and self-contained temporary test fixtures.
- Finalized ADR 0005 implementation: renamed `config-toml.md` to `settings-toml.md`, transitioned test fixtures to explicit `default.toml` suites and `settings.toml` host environments, eliminated dead backwards-compatibility code and shims, and verified non-fatal error reporting for unresolvable sibling routes.
- Documented workflow-local command defaults; suite and settings command registration is rejected.
- Documented rejection of suite host-environment fields and conflicting member aliases.
- Updated form, preview, and embedded-form commands to select suite fixtures explicitly; theme instructions now use settings.toml.
- Migrated getting-started and first-workflow tutorials to required local entrypoints, explicit default suite mounts, and standalone `-w` invocation.
- Updated CLI, workflow, and settings references for ADR 0005 explicit manifests and settings separation.
- Linked ADR 0004 and ADR 0005 from the bundle root governance index.

## 2026-09-19

- Added accepted ADR 0005 (`docs/adr/0005-manifest-driven-suites-and-self-contained-workflows.md`):
  - Defined manifest-driven workflow suite orchestration (`[suite]`), strict two-tier non-nesting, and 100% self-contained atomic workflow contracts (`[workflow]`).
  - Disambiguated CLI dispatch semantics: `-w, --workflow <PATH>` for single atomic workflows vs `-s, --suite <PATH>` for multi-workflow suite manifests.
  - Decoupled passive host environment (`~/.config/tflow/settings.toml`) from workflow governance, eliminating `default_view` and `disabled_workflows` sanitization hacks.
  - Centralized routing aliases into suite manifest `[aliases]` tables, stripping aliases from individual workflow views to eliminate collision.
  - Rejected suite-level private themes in favor of surgical semantic style slot overrides (`[styles.<id>.<slot>]`) inheriting active global theme schemes.
  - Standardized ephemeral pipeline execution (`npx` / UNIX pipe streaming) with `/dev/tty` TUI isolation and stdout structured output.
  - Updated `docs/adr/index.md` to register ADR 0005.

## 2026-09-17

- Updated the CLI Reference (`docs/reference/cli.md`): documented `-w, --workflow <PATH>` for running single-file workflows (`.toml`) and directory packages in isolated mode; documented zero-configuration in-memory fallback when global `config.toml` is absent; specified entry point resolution prioritizing `alias = "main"` globally and requiring `alias = "main"` or explicit view selection in isolated workflow mode; documented the `--theme <THEME>` flag; and documented Shebang integration for executable workflow files (`#!/usr/bin/env -S tflow -w`).

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

- Added `tflow inspect --all` to export stable JSON contracts for every configured View, including alias, Engine, query, and command metadata.

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
  - Established decentralized workflow layout under `$XDG_CONFIG_HOME/tflow/workflows/`.
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
