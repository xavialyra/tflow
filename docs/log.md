# Development Log

This log records notable changes to the documentation and product during development. It is a concise engineering record rather than a release changelog.

## 2026-09-24 (v0.1.0-alpha.3)

- Implemented ADR 0007: Declarative popup 9-box optical anchors (`top-center`, `center`, etc.), responsive viewport-relative sizing (`width`/`height` percentage and bounds), and global terminal viewport coordinates.
- Added non-focus backdrop dimming with muted color projection and configurable `chrome.backdrop` theme styling with explicit modifier overrides and `dim_backdrop` toggle.
- Added `FieldType::Enum` support to the Form Engine with inline selector navigation (`< value >`), arrow/space cycling, and prefix typeahead filtering.
- Extended View query schemas and CLI parameter parsing with `ParameterType::Enum` options validation.
- Added customizable and disableable keybindings for Embedded and Form engines.
- Improved root Picker UX by clearing input on back navigation before closing the view.
- Ensured completion popups do not open when no candidate items match.
- Reverted built-in workflow alias support to maintain clean manifest-driven suite boundaries.

## 2026-09-23

- Decoupled GitHub Actions workflows into dedicated lightweight `ci.yml` (fast lint, check, and test gate on push/PR) and `release.yml` (full release build, asset assembly, and GitHub Release publication on tag push).
- Split Suite Manifest specification into dedicated reference (`docs/reference/suite-toml.md`), cleaned up `settings.toml` specification to focus on passive host environment, and structured `workflow.toml` reference with comprehensive schema tables and quick-look matrices.
- Clarified workflows, suites, and settings in Getting Started, added optional settings configuration, corrected the repository URL, and made setup paths respect XDG_CONFIG_HOME. Linked README suite setup to the tutorial.

## 2026-09-22

- Simplified workflow source discovery in the setup wizard. Sources are now explicit, stale cache reuse is rejected, and installed packages are replaced cleanly.
- Moved View binding strategy to `keymap_mode`; keymap tables now contain only key bindings.
- Improved navigation handoff and popup rendering while a View is waiting to publish. Existing content and footer state remain visible during the short grace period.
- Removed route-completion documentation from the host guides. Route completion is implemented as a workflow recipe.
- Added per-View Picker input placeholders and a bounded preview cache for remounted Views.
- Centralized product identity, paths, runtime prefixes, and `TFLOW_*` environment names.
- Added wizard integration coverage for source changes, removed files, generated configuration, and `--check` validation.

## 2026-09-21

- Updated tutorials and references to the current workflow-root command model and `keymap_mode` configuration.
- Completed the transition to explicit suite manifests and self-contained workflows.
- Fixed producer handling when a child exits before consuming all request input.
- Consolidated route resolution around `ViewLocation` and removed duplicate router logic.
- Moved route completion behavior into workflow commands and removed route logic from the Picker engine.
- Added configurable Picker left prefixes, prefix-aware Backspace navigation, and item-driven key bindings.
- Standardized headless inspection with `--inspect`, `--items`, and `--all`.
- Added selection restoration with the reserved `__focus` query value.

## 2026-09-19

- Defined the manifest-driven suite model and separated host settings from workflow configuration.
- Reworked command ownership, View binding, item aggregation, and headless workflow inspection.
- Consolidated fixtures and updated tutorials, references, and architecture records to the new configuration model.

## 2026-09-17 – 2026-09-05

- Added and refined the Picker, Capture, Embedded, Form, preview, navigation, theme, and producer-protocol implementations.
- Added runtime cleanup, cancellation, terminal restoration, process limits, and task-correlation guarantees.
- Added architecture decision records covering workflow extensions, script boundaries, command registration, suite manifests, and item-driven bindings.
- Built the Diátaxis documentation bundle with tutorials, how-to guides, references, explanations, and runnable fixtures.
- Renamed the product and consolidated its internal identity constants as `tflow`.
