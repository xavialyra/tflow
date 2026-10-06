# Development Log

> **Note:** This log records only concise summaries of notable architectural, configuration, and documentation changes. For detailed design rationale and implementation history, refer to the [Architecture Decision Records](adr/index.md) and commit history.

## 2026-10-06: Companion adaptive layout, sizing, and contract convergence (v0.1.0-alpha.10)

- Implemented adaptive multi-direction layout: automatic horizontal split with vertical divider on wide terminals (`width >= 60 && width >= height * 2`) and vertical split with horizontal divider on narrow or stacked terminals.
- Added view-level `companion_size` configuration supporting 2D pairs `[width, height]` (e.g. `["40%", 12]`) as well as 1D shorthand (e.g. `"40%"` or `50`), with seamless default fallback to 50%/50%.
- Tuned default workflow ergonomics and companion aspect ratios across `init.toml`, `apps`, `calculator`, `clipboard`, and `sys`.
- Promoted companion attachments into first-class `type = "companion"` commands, removing intermediate companion slot tables in favor of pure dynamic context projections.
- Fixed structured query deserialization when switching into companion views by adding newline-preserving parameter sanitization (`sanitize_parameter_text`).
- Isolated view command exposure to strictly declared bindings, preventing global workflow command leaks in palette and chrome.

## 2026-10-06: Alpha.8 release alignment

- Aligned the alpha.8 changelog and Companion guide with the implemented passive attachment model: ordinary stack navigation, explicit source payloads, Picker/Capture live updates, and Form/Embedded mount snapshots.
- Marked the obsolete pane-focus, promotion/demotion, and Escape-ladder proposal as historical rather than runtime behavior.
- Documented display-sized Capture image caching, resize reloads, request coalescing, popup-layer handling, and tmux protocol limitations.

## 2026-10-05: Companion data and scheduling boundaries

- Introduced typed source payloads shared by attachment and live updates; Capture no longer parses JSON-looking input or removes business fields to infer source context.
- Made live updates explicit for Picker/Capture, while Form/Embedded retain mount snapshots; documented that foreground entry creates a new instance.
- Routed producer tasks by mount role rather than engine: companion work uses the background class and foreground work uses the serial class.
- Updated Host layout ownership documentation and separated the historical ADR proposal from current configuration contracts.

## 2026-10-02: Companion convergence amendment

- Clarified ordinary Push / Close companion navigation and marked the obsolete focus handoff, Escape ladder, and focus-border contracts in ADR 0010 as historical.
- Unified validated default-query construction for companion attachment and navigation, and shared declared input binding resolution while preserving dynamic script input as computed data.
- Documented remaining Omnibar ownership and pane geometry/Resize work in [Companion Navigation and Host Ownership](explanation/companion-host-ownership.md). The earlier alpha.8 entries below describe the original design intent and must not be read as proof that all convergence goals are implemented.
- Replaced the shared Companion matrix entrypoint with eight standalone test fixtures in `tests/fixtures/companion-matrix/`, including cascading Picker + Picker, Capture + Picker, and Embedded + Capture.
- Fixed the Form + Capture example to initialize field values explicitly and render live published draft state through `$item`; headerless primary Views now render within the left pane rather than underneath the companion. Pane-specific Resize and complete Host chrome ownership remain pending.
- Form content producers now receive explicit navigation/companion input seeds when supplied. Added PTY coverage for live Form summaries, selected-job Form seeds, and multi-level Picker navigation.

## 2026-10-02 (v0.1.0-alpha.8)

- **Universal Companion Views ([ADR 0010](adr/0010-companion-views-and-unified-host-layout-architecture.md)):** Promoted preview mechanisms into first-class named companion views across all engines (`companion = "<target>"`, `companion_input = "$item"`).
- **Single-Stack Projection Topology (`[A|B]` vs `[A, B]`):** Modeled companion side-by-side presentation as linear stack projections (`[A|B]`) that can be cleanly promoted to active top views (`[A, B]`) and demoted on return without window manager bloat.
- **Companion Navigation & Deterministic Escape Ladder:** Monotonic Escape unwinding (clear query -> pop view -> exit). Ordinary navigation stack replaces dedicated pane-focus keys.
- **Decoupled Host Omnibar & Input Policies:** Extracted logical input buffer into host-level `HostInputState` with presentation modes (`Omnibar`, `BorderTitle`, `FloatingPrompt`) and engine `UnhandledInputBehavior` declarations (`ForwardToOmnibar`, `ConsumeLocally`, `Ignore`).
- **Split Pane Rendering:** Automatic split pane horizontal layout with focused/inactive border styling and theme integration.

## 2026-10-01 (v0.1.0-alpha.6)

- **Canonical View Engine Declarations ([ADR 0009](adr/0009-canonical-view-engines-and-flat-command-syntax.md)):** Required an explicit `engine = "picker" | "capture" | "form" | "embedded"` field on every view and moved engine-specific settings into named sub-tables (`[views.<name>.<engine>]`).
- **Flat Command & Producer Syntax:** Removed `producer` and `handler` wrappers from `[commands.<id>]`, `return_processor`, and engine data sources (`items`, `output`, `content`). Dynamic scripts declare `file` or `script` directly; static commands declare operation fields (`argv`, `target`, `value`) directly.
- **Simplified Picker Preview:** Standardized `[views.<name>.preview]` to five canonical fields (`open`, `width`, `min_width`, `file`, `script`), removing legacy preview aliases, `inherit`, and static TOML document nodes.
- **Clean Break:** Removed all legacy configuration shims, fallback parsers, and migration hints across the codebase, examples, and documentation.

## 2026-09-30

- Added configurable `chrome.border_type` in themes (`rounded`, `plain`, `double`, `thick`, `quadrant-inside`, `quadrant-outside`) to customize popup border glyphs.
- Made `[workflow].entrypoint` optional, defaulting to `"main"` when omitted.

## 2026-09-28

- Unified engine actions (`@engine:<engine>.<action>`) and workflow commands (`<workflow>.<command>`) in the central command index with strict namespace resolution and uniqueness checks.
- Redesigned `[views.<name>.unbind]` into three orthogonal axes (`keys`, `commands`, `layers`) and replaced boolean view binding tombstones with `unbind.keys`.
- Simplified engine binding internals by routing all key dispatch and Chrome hint projection through the session registry.
- Standardized `--inspect` output and added CI validation (`--check`) for bundled examples.

## 2026-09-27

- Implemented [ADR 0008](adr/0008-workflow-defined-features-host-bindings-and-data-driven-chrome.md): replaced hardcoded built-in palette and parameter views with workflow-defined implementations (`__commands`, `__parameters`), introduced `[host.bindings]`, and adopted dot-delimited command FQIDs (`workflow.command`).
- Flattened `settings.toml` root engine binding tables and simplified `run` commands to non-interactive execution.

## 2026-09-25

- Enabled automatic terminal graphics protocol probing by default with fallback to `halfblocks`.
- Switched the Picker input line to use the native terminal cursor with optional RGB color overrides.

## 2026-09-24 (v0.1.0-alpha.3)

- Implemented [ADR 0007](adr/0007-declarative-popup-presentation-and-viewport-relative-geometry.md): declarative 9-box popup anchors, viewport-relative sizing, and non-focus backdrop dimming.
- Added `enum` field support and unified inline selector navigation for `boolean` and `enum` fields in the Form engine and CLI parameters.
- Added configurable `timeout_ms` for foreground `run` operations and completed the Stage 5 scheduler benchmark harness.

## 2026-09-23

- Split GitHub Actions into fast `ci.yml` and tag-triggered `release.yml` workflows.
- Separated suite manifest (`suite-toml.md`), host settings (`settings-toml.md`), and workflow (`workflow-toml.md`) reference specifications.

## 2026-09-22

- Streamlined the setup wizard, added per-view Picker input placeholders, and introduced a bounded preview cache for remounted views.
- Improved navigation handoff grace periods to prevent visual flicker while target views load.

## 2026-09-21 – 2026-09-05

- Established the manifest-driven suite architecture ([ADR 0005](adr/0005-manifest-driven-suites-and-self-contained-workflows.md)) and workflow-scoped commands with item bindings ([ADR 0006](adr/0006-feed-removal-workflow-scoped-commands-and-item-bindings.md)).
- Built the four core engines (`picker`, `capture`, `form`, `embedded`), producer JSON protocol, theme system, and Diátaxis documentation bundle.
