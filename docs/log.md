# Development Log

> **Note:** This log records only concise summaries of notable architectural, configuration, and documentation changes. For detailed design rationale and implementation history, refer to the [Architecture Decision Records](adr/index.md) and commit history.

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
