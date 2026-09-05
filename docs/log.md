# Documentation Changelog

This changelog tracks updates to the `tui-launcher` knowledge bundle.

## 2026-09-06

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
