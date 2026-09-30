# Development Log

This log records notable changes to the documentation and product during development. It is a concise engineering record rather than a release changelog.

## 2026-09-30

- Added configurable `chrome.border_type` in themes: themes may declare `[chrome].border_type = "rounded"` (default), `"plain"`, `"double"`, `"thick"`, `"quadrant-inside"`, or `"quadrant-outside"` to control popup dialog border glyphs. Documented in `docs/reference/theme-toml.md` and `docs/how-to/custom-themes.md`.
- Made `[workflow].entrypoint` optional, defaulting to `"main"` when omitted. Streamlined built-in workflows (`__commands`, `__parameters`), `examples/init.toml`, and `examples/default-suite` workflows, and updated `docs/reference/cli.md` and `docs/reference/workflow-toml.md` to reflect this zero-boilerplate behavior.

## 2026-09-28

- Streamlined `ActionBindings`: replaced `from_values(defaults, view)` and `validate_values(defaults, view)` with `from_defaults(defaults)` and `validate_defaults(defaults)`. In ADR 0008 all four engines only read root defaults (`[picker.bindings]`, etc.), so the legacy `view` parameter was vestigial. Removed 6 obsolete unit tests that tested dead-path `view` merging or duplicate entry resolution.
- Excluded the command palette command itself (`__commands.palette`) from the built-in palette's item list (`assets/builtin/workflows/__commands/workflow.toml`), so opening the command palette does not recursively list "Commands (ctrl+k)". Added an integration test assertion in `tests/launcher.rs`.
- Formatted `--inspect`'s `bindings` map with `@engine:` sigils for engine actions (`ResolvedAddress::binding_address`), so the contract's values match valid configuration syntax and round-trip losslessly when an aggregating script attaches them to `item.bindings`. `commands[].id` keeps the plain FQID as the wire shape for `invoke-command`.
- Unified all four built-in engines (`picker`, `form`, `capture`, `embedded`) to derive their `engine_commands()` labels from `action.label()` instead of hardcoding duplicated string literals.
- Documented runtime fault tolerance for `item.bindings` in `workflow-toml.md` and `producer-protocol.md`: non-string values or unresolvable commands are leniently skipped, and unparseable key names degrade to keyless View-layer entries rather than causing fatal Picker errors.

- Kept `"key" = false` in the engine default tables and `[host.bindings]`, and made it the *only* spelling of "this layer declares no binding here": the undocumented `"noop"` alias is gone, and every other value (`true`, a number, an array, an empty string) is now a load error instead of a silent removal (`loader.rs::parse_host_binding_target`). The three mechanisms are now documented side by side in `workflow-toml.md`: a binding value names a command; `unbind` releases an association while the command keeps its identity; `false` means the layer never publishes an entry at all, so a command left without a key is no longer invokable by id either. Fixed the two reference pages that had `[host.bindings]` precedence backwards — the tables merge in increasing precedence, so `settings.toml` wins over the suite manifest, including a `false` there.
- Deleted `InputActionBinding::enabled`, a vestige of the removed Picker `disabled_keys` set: both constructors wrote `true` and the only reader was a guard in the embedded engine's `engine_commands`.
- Fixed a tautological test: `session_dispatches_to_fallback_receiver_when_key_unbound` injected its binding into the View layer, which every dispatch rebuilds from the active view, so the key reached the fallback whether or not `unbind` had released it. The entry now lives in the Host layer (which dispatch does not rebuild) and the assertion fails when the release is removed.
- Removed the boolean branch from the `--inspect` declaration contract, which validation had already made unreachable.
- Deleted the `InputActionBinding` staging type and everything that existed only to feed it: `ResolvedInputAction`, `EditorAction`, `InputBindingFactoryContext`, both `create_input_bindings` factories, `ProjectedBindingConfig::engine_fields`/`engine_field`, and `FactoryFieldPlan::binding`. It was the engine's own keymap row (`key` + resolved `Edit`/`Engine` action + a label for the footer hints + an `enabled` filter) from before command registration existed; the registry took over key resolution and chrome hints in `90194f3`, after which the Picker's copy was built and discarded (`let _bindings`) and the Embedded engine kept the `Vec` only to enumerate its engine commands. The embedded engine now stores `EmbeddedBindings` and walks it in `engine_commands` exactly like the picker, form, and capture engines do, so all four publish engine rows the same way; `"key" = false` in an engine table keeps meaning "this layer declares no binding", now simply as "no row is built at all".
- Removed the boolean binding tombstone (`"escape" = false`). A View binding now always names a command, and a boolean value is rejected at load time with a pointer to `unbind.keys`; `unbind.keys` is the one way to take a key away from a lower layer *inside a View* (the engine default tables and `[host.bindings]` keep `false` for the same thing at their own level), and it *releases* the key so it drops through to raw input handling (text entry, or an `embedded` child process) rather than being swallowed. The two were never interchangeable — the tombstone claimed the key and consumed the event — and the shipped docs told users to pass `Escape` to a child process with `"escape" = false`, which actually suppressed it. `CommandEntry` lost its `disabled` bit, `CommandRegistry` lost `is_explicitly_disabled`, and session dispatch lost the branch that stopped before the fallback receiver.
- Made command resolution strict: a `[views.<name>.bindings]` value now resolves **only** to a command of the current workflow. A bare name is never an engine action, so `"escape" = "exit"` means the workflow's own `exit` command; the previous silent fallback to `picker.exit` is the bug this removes. Engine actions are addressed explicitly as `@engine:<engine>.<action>` (e.g. `"ctrl+p" = "@engine:picker.toggle_preview"`), which the command index turns into a View-layer engine entry; a workflow command may also name its owner explicitly as `@workflow:<command>` (`@workflow:mytools.open` or `@workflow:open`), which resolves exactly like the bare/FQID spelling. Engine-layer defaults (`[picker.bindings]`, `[form.bindings]`, …) keep the bare action names, since they are the engine's own namespace. Migrated the fixtures, examples, and integration tests that relied on the fallback; `@engine:<command>` unbinding now names the FQID (`@engine:picker.clear_input`).
- Redesigned `unbind`: it is now a per-View table (`[views.<name>.unbind]`) with three fields that map to three independent axes — `keys` (physical keys), `commands` (command addresses, same grammar as binding values, e.g. `core.page` or `@engine:picker.clear_input`), and `layers` (priority layers `view`/`engine`/`host`). Command ownership (the unique index) and priority (layer) are no longer conflated in one sigil-overloaded string list, so the `@host:*`/`@engine:<command>` forms are gone; `__commands:main` and `__parameters:main` now use `layers = ["host"]`.
- Engine actions are now ordinary commands: `{id}` runs one exactly like a key press does, by handing it to the engine of the View that owns the entry (the View whose binding produced the row). The palette therefore lists and selects them instead of refusing. Two `bail!` sites, a duplicate target match in the session, and the interim `invokable` flag are gone: one dispatch decides how every entry runs, and a key press only chooses which entry.
- The built-in palette no longer searches the layer it stopped displaying, and it curates (`View`/`Host` layers) instead of gating: the engine's own default keymap is not a command the workflow provides, so it stays in Chrome hints.
- Removed the second copy of a command's definition: `CommandOrigin::Host` no longer embeds the `Command` that `CommandInvocation` already carries, so the return processor resolves its owning command with the same FQID lookup the View origin uses.
- Gate the shipped examples: `bundled_examples_validate` runs `--check` on `examples/default-suite` and `examples/init.toml`, so the documentation cannot drift from the configuration surface.
- Fixed a lost-shortcut bug in the command projection: `picker_entries()` deduplicated by command id before resolving keys, so a command bound to two keys kept only one — and which one depended on `HashMap` iteration order, i.e. it could differ between runs. The picker's own defaults hit this (`ctrl+c` and `ctrl+d` both drive `picker.exit`), so one of the two never appeared in the footer hints or the palette. `ActionBindings::bindings()` now yields a stable canonical order, and the projection lists one row per effective binding: a shadowed key still degrades to keyless, and the only row dropped is a keyless one for a command already listed with a key.
- Enforced command-index uniqueness: the index is the set of dot FQIDs, where an engine action owns `<engine>.<action>` and a workflow command owns `<workflow>.<command>`. A workflow command that lands on an engine action's id (a workflow named `form` declaring `exit`, say) is now rejected at load time instead of being silently resolved by priority — previously `picker_entries` deduplicated the two into one entry and `resolve_id` picked whichever came first. Sharing an engine's *name* remains legal (`form.open` is fine), because only the action ids are taken. Added `engine::engine_action_from_id` as the sigil-free identity lookup used by that check.
- Unified engine actions into the command index: each engine resolves a bare action name into `<engine>.<action>` + label via `engine::engine_action`, so a View binding can address an engine action exactly like a workflow command. Engines no longer re-parse `[views.*.bindings]`; their tables carry only root defaults, and all view-level rebinds flow through the registry.
- Removed the second engine key-dispatch path: built-in `View::event` implementations now reject direct key input because the session registry owns key resolution, and dispatches either `on_command` or `on_unbound_key`. The former `View::dispatch_key_event` is gone; a test-only adapter preserves isolated engine tests without adding a runtime path.
- Unified cross-workflow command context: a fully qualified command still resolves its own definition and script root, but `context.parameters`, `context.view`, and return-processor inputs always belong to the View that invoked it. An aggregate Picker therefore keeps its live parameters when dispatching an item binding such as `apps.open`; selected-item data remains in the Picker engine state.
- Completed Route B: `CommandEntry` no longer stores a `CommandTarget` or an owning View/source View. Its binding layer only controls priority; the command id identifies an engine action or workflow command, and every workflow command is prepared with one caller-View context.

## 2026-09-27

- Made `unbind` remove the key-to-command association rather than filter dispatch: the registry resolves an *effective key* (`None` when unbound), so `resolve` cannot match it, `picker_entries` reports it keyless (like a shadowed binding), and `chrome_commands_show` therefore produces no hint for it. Identity invocation (`resolve_id`) still works.
- Removed the residual hardcoded Chrome footer logic: the legacy command-ID recognition (`Enter` / `commands`) and narrow-footer overflow fallback are gone. Footer hints now come solely from `chrome_commands_show` resolved through `View > Engine > Host`, with no special command-ID branches. Deleted the dead `CommandBindingVisibility` (`overflow`/`hidden`) surface and the internal "session command" naming in favor of host layer.
- Renamed the remaining internal `keymap` identifiers to `bindings` (`input/bindings.rs`, `BindingAction`, `*Bindings`, `view_bindings`, `validate_bindings`) and dropped the unused `owner` field from the command palette payload.
- Implemented the breaking ADR 0008 migration: command FQIDs now use dot notation (`workflow.command`) while view routes keep colons (`workflow:view`); the legacy `keymap` table and command-level `key` fallback are removed in favor of a single key-centric `bindings` table; `keymap_mode` is renamed to `binding_mode`; layer-based `unbind` (`@view`/`@engine`/`@host`, optional `:<command>`/`:*`, or physical keys) suppresses inherited physical dispatch without leaking orchestration FQIDs.
- Flattened `settings.toml`: engine default tables (`[picker]`, `[capture]`, `[embedded]`, `[form]`) now live at the root with key-centric entries, removing the `defaults.` prefix and the action-centric `action = [keys]` form. Added `[host.bindings]` with dot-delimited command FQIDs.
- Simplified `run` command execution to non-interactive mode, eliminating terminal suspend/resume cycles, screen flickering, and redundant `mode` protocol constraints.
- Added ADR 0008: workflow-defined features, protocol command invocation, host-layer bindings, unified key-centric bindings model, and data-driven Chrome command display.

## 2026-09-25

- Made preview image protocol selection automatic by default, probing the interactive terminal with a bounded timeout and falling back to halfblocks while retaining explicit protocol overrides.
- Replaced the Picker's rendered pseudo-cursor with the terminal cursor and limited `[cursor]` theme customization to the host's optional RGB color; embedded views reset the host color override.

## 2026-09-24 (v0.1.0-alpha.3)

- Enhanced Form Engine boolean fields to share unified inline selector indicators (`< true >` / `< false >`) and discrete option cycling with enum fields, including arrow/space navigation, letter jumping (`t`/`f`), input safety against invalid characters, and paste normalization.
- Implemented configurable foreground command execution timeouts (`timeout_ms` in `run` operations and Producer protocol) with graceful process-group kill, terminal reclamation, and host state resumption, closing Architecture Convergence Stage 1.
- Implemented the Stage 5 scheduler benchmark and evidence harness (`src/task/stage5_evidence.rs`), providing reproducible end-to-end task scheduling verification, 250 ms p95 SLO assertions, and on-demand markdown/json metric artifact generation (`STAGE5_FULL=1`).
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

## 2026-09-27

- Updated ADR 0008 with unified command addressing: dot notation for command FQIDs (`workflow.command`), colon notation for view routes (`workflow:view`), and system layer sigils (`@host:*`, `@engine:*`).
- Established layer-based unbinding (`unbind = ["@host:*"]`) in `bindings` to isolate modal popups without leaking orchestration details or breaking under user key remappings.
- Consolidated built-in assets under `assets/builtin/` with `default.toml` as base/default theme and open `__` convention for bundled fallback workflows.

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
