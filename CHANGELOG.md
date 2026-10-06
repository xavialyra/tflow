# Changelog

All notable changes to `tflow` are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0-alpha.9] - 2026-10-02

### Added
- **First-Class Companion Commands (`type = "companion"`)**:
  - Promoted companion attachments into first-class workflow commands directly declaring `target` and `args` (or `query`).
  - Seamless keybinding experience: companion triggers are bound in `views.<name>.bindings` like any standard command.
  - Multi-companion support with smooth hot-swapping across commands (e.g. `preview`, `inspector`, `help`) with distinct keys and labels in the `Ctrl-K` palette.
  - Dynamic context projection: projected parameter templates evaluated pure-functionally (`$selection`, `$selection.<path>`, `$input`, `$query.<path>`).
- **Automatic Default Companion Resolution**:
  - Views declaring `companion = "<name>"` automatically resolve matching companion commands to inherit declared target and projection templates.

### Changed
- **Suite Workflows Migration to Explicit Contracts**:
  - Migrated `apps`, `clipboard`, `sys`, `calculator`, `init.toml`, and companion matrix fixtures to explicit companion commands and parameters.
  - Capture and companion scripts receive parameters via standard `context.parameters` rather than leaking internal engine states.

### Removed
- **Eliminated Companion Side-Channels & Intermediate Tables**:
  - Completely purged `CompanionData` and `query_seed()` implicit state transfer mechanisms.
  - Completely removed intermediate `[views.<name>.companions]` slot tables in favor of unified first-class commands.

## [0.1.0-alpha.8] - 2026-10-02

### Added
- **Universal Companion Views ([ADR 0010](docs/adr/0010-companion-views-and-unified-host-layout-architecture.md))**:
  - Promoted preview mechanisms into first-class named companion views across all engines (`companion = "<target>"`).
  - Clean view declaration: views declare default-open companions with explicit source payloads; there is no view-level `companion_input` contract.
  - Unified command operations: companion commands share `query` handling with `navigate`, while omitted-query toggles retain live tracking where supported.
  - Picker/Capture companions receive live source updates; Form/Embedded companions retain their mount snapshot.
- **Single-Stack Projection Topology (`[A|B]` vs `[A, B]`)**:
  - Linear view stack extension that models side-by-side companion panes cleanly without terminal multiplexer complexity.
  - Ordinary stack-based companion navigation; the previously proposed promotion/demotion operations are not runtime contracts.
- **Passive Companion Input Model**:
  - Companions remain side-by-side attachments while the primary retains input ownership; foreground entry uses the ordinary stack and `@host:open_companion`.
  - The focus handoff, dedicated focus keys, and Escape-ladder proposal are historical and are not runtime contracts.
- **Decoupled Host Omnibar & Engine Input Policies**:
  - Extracted logical input buffer into host-level `HostInputState` with presentation modes (`Omnibar`, `BorderTitle`, `FloatingPrompt`).
  - Active views declare `UnhandledInputBehavior` (`ForwardToOmnibar`, `ConsumeLocally`, `Ignore`).
  - Rendered top-level Omnibar widget directly in host chrome when active view has visible input presentation mode.
- **Split Horizontal Pane Rendering**:
  - Clean split presentation without redundant outer border boxes, with shared Host-computed pane geometry and a divider separating the companion pane.
  - Companions are passive attachments; there is no separate focused/unfocused pane state.

### Changed
- **Capture image pipeline**:
  - Cache display-sized images, reload after display-size changes, coalesce rapid requests, and bound decode/encode work.
  - Preserve stateful image rendering across popup layers and document tmux protocol limitations.

### Removed
- **Legacy Picker Preview Module**:
  - Completely purged obsolete `src/engine/picker/preview/` module, preview caching, and auxiliary worker hooks from `picker`.
  - Removed outdated `[views.*.picker.preview]` and `[views.*.preview]` configuration fields across compiler, validators, and engine schemas.

## [0.1.0-alpha.7] - 2026-10-02

### Added
- **Capture Engine Native ANSI Color & Viewport Scrolling**:
  - Direct SGR/ANSI escape sequence parsing via `ansi-to-tui`, rendering full-color terminal text inside `capture` views.
  - Native scrolling key bindings (`Up`/`k`, `Down`/`j`, `Ctrl+u`, `Ctrl+d`, `PageUp`, `PageDown`) and adaptive picker-aligned scrollbar rendering.
  - Plain-text clipboard sanitization (`capture.copy`) stripping ANSI escapes before placing content into the system clipboard.
- **Zero-Process Static Capture Content**:
  - Support for `[views.<name>.capture.output] content = "..."` and inline `output = "..."` for instant, process-free document and report presentation.
- **Unified Presentation Architecture (Capture as Universal Presenter)**:
  - Promoted `capture` to handle both ANSI terminal output and Ratatui Document AST with multi-protocol native terminal images (Kitty, Sixel, iTerm2).
- **Picker Preview Kernel-Level Capture Integration**:
  - Refactored `picker.preview` to embed `CaptureSession` and `CaptureRenderer`, bringing native ANSI coloring, unified scrollbars, and consolidated image pipelines to preview panes.
  - Script output protocol supports standard `{"version": 1, "output": "..."}` alongside `preview`.

### Changed (Internal Evolution)
- **Execution Model Modernization (De-producerization)**:
  - Replaced legacy `ProducerKind` with strongly typed `ExecutionMode` (`Declared`, `Script`).
  - Standardized script handling on `parse_script_source` and `script_context`.
- **Pure Strongly-Typed Engine Configuration**:
  - Purged `FactoryFieldPlan`, `fields_for_view`, and dynamic `fields` dictionaries across engine projections in favor of compile-time verified structs (`PickerConfig`, `CaptureConfig`, `FormConfig`, `EmbeddedConfig`).

## [0.1.0-alpha.6] - 2026-10-01

### Changed (Breaking Changes)
- **Explicit View Engine Declaration & Named Engine Sub-Tables ([ADR 0009](docs/adr/0009-canonical-view-engines-and-flat-command-syntax.md))**:
  - Every view must now declare `engine = "picker" | "capture" | "form" | "embedded"`.
  - Engine-specific configuration is declared in named sub-tables (`[views.<name>.<engine>]`), replacing the legacy `[views.<name>.engine]` table and eliminating property misattribution.
- **Flattened Commands & Return Processors**:
  - Eliminated `producer` and `handler` table nesting across commands and return processors.
  - Commands declare `file` or inline `script` directly for dynamic execution, or top-level operation fields (`argv`, `target`, `value`) for static declarations.
  - Applied the same flat syntax to `picker.items`, `capture.output`, and `form.content`.
- **Canonical 5-Field Picker Preview**:
  - Formalized `[views.<name>.preview]` to exactly five supported fields: `open`, `width`, `min_width`, `file`, and `script`.
  - Removed legacy aliases (`preview_ratio`, `preview_min_width`, `preview_default_open`), obsolete `inherit` mode, and static TOML `document` nodes.
- **Clean Break Policy**:
  - Removed backward compatibility shims, desugaring fallbacks, and migration hints. Invalid or legacy syntax produces standard parser schema errors.

### Documentation & Developer Experience
- Simplified `docs/log.md` to record high-level milestone summaries.
- Added [ADR 0009](docs/adr/0009-canonical-view-engines-and-flat-command-syntax.md).
- Updated all built-in workflows, examples, and test fixtures to the canonical syntax.
