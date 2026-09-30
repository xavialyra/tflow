# Changelog

All notable changes to `tflow` are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
