# Architecture Guide

This project is a terminal workflow host. The long-term structure should keep
configuration, session state, engine behavior, and terminal rendering as
separate domains. A module may expose a small crate-level facade, but new
feature code should live in the narrowest domain that owns its behavior.

## Current Boundaries

The structural migrations keep existing crate paths and runtime behavior stable
while separating the largest mixed modules:

```text
src/
  lib.rs
  main.rs

  app/
    mod.rs                  top-level startup and App facade
    cli.rs                  CLI arguments and process bootstrap
    invocation.rs           stdin capture and final result adaptation

  workflow/
    mod.rs
    config/
      mod.rs                Config facade and stable queries
      model.rs              typed configuration model
      loader.rs             filesystem and plugin package loading
      normalize.rs          TOML/runtime normalization
      compile.rs            template/state registry compilation
      validation.rs         schema and command validation
      evaluation.rs         evaluation scopes and runtime projection
    expression/
      mod.rs                bounded dynamic expression language
    query/
      mod.rs                StateRegistry and StateInstance facade
      schema.rs              query field types and schema compilation
      instance.rs            query state lifecycle and updates
    command/
      mod.rs
      model.rs              command context, effects, and output contracts
      prepare.rs            command lookup and action preparation
    navigation.rs           Router and route resolution
    runtime.rs              shared JSON runtime store

  session/
    mod.rs                  AppSession and main loop coordination
    input.rs                input transport and passthrough parser
    input_dispatch.rs       binding refresh, route completion, and dispatch
    navigation.rs           push/replace/call/return and rollback
    effects.rs              ViewEffect processing and input mutation
    chrome.rs               Chrome assembly and route completion rendering
    state.rs                session stack and binding state types
    publication.rs          runtime publication

  engine/
    mod.rs
    api.rs
    host.rs
    registry.rs
    evaluate.rs
    picker/
      preview/
        mod.rs
        image_decode.rs
        image_path.rs
    capture/
    embedded/
      mod.rs
      pty.rs
      terminal.rs
      session.rs

  input/
    mod.rs                  Key and InputDecoder
    editor.rs               neutral input buffer and Unicode editing
    keymap.rs               generic layered keymap and input router

  ui/
    mod.rs
    chrome/
      mod.rs
      input.rs                stable re-export of the input model
      layout.rs
      frame.rs
    theme/
      mod.rs
      model.rs
      color.rs
      binding.rs
      load.rs

  terminal/
    mod.rs                  TTY, ratatui backend, and terminal image protocol
    sanitize.rs             terminal-control sanitization

  execution/
    mod.rs
    process.rs
    runner.rs               bounded command execution
    script.rs               plugin script execution and path safety

  task/
    mod.rs                  generic background task lifecycle and scheduling

  lifecycle/
    mod.rs                  CancellationToken
    signal.rs               signal guard and final-output state

  diagnostics/
    mod.rs
    log.rs                   structured runtime logging
```

Generic task lifecycle and scheduling is owned by `task/`; it does not know
about engines or picker items. Task closures have a cooperative cancellation
contract: shutdown cancels queued and active work, then waits for the worker to
exit. Latest-wins replacement is lane-scoped; picker-specific item request
adaptation owns its `picker-items:<view_ref>` lane in `engine/picker/tasks.rs`.
The other formerly mixed Engine modules have been moved to their owning
top-level or feature-specific modules.

## Target Boundaries

The current module boundaries use this shape for the application workflow,
Engine, UI, and infrastructure domains:

```text
src/
  lib.rs
  main.rs

  app/
    mod.rs                    top-level startup and App facade
    cli.rs                    Args, default config path, and bootstrap
    invocation.rs             stdin capture and invocation result adaptation

  workflow/
    config/                   loading, normalization, compilation, evaluation
    expression/               bounded dynamic expression language
    query/                    query schemas and state instances
    command/                  command contracts and preparation
    navigation.rs             Router and route resolution
    runtime.rs                shared workflow runtime store

  session/                    stateful workflow execution and orchestration
  engine/                     View Engine protocol and concrete implementations
  input/                      terminal input model and layered keymaps
  ui/                         Chrome and Theme presentation modules
  terminal/                   TTY and terminal-control adapter
  execution/                  external process and script execution
  task/                       generic background task lifecycle and scheduling
  lifecycle/                  cancellation and signal lifecycle
  diagnostics/                structured runtime logging
```

The target tree is a direction, not a reason to create empty wrappers. A new
file is justified when it owns a distinct invariant, side-effect boundary, or
test domain.

## Dependency Rules

1. `workflow/config/model` contains data and serde definitions. It must not
   depend on `session`, ratatui, or terminal I/O.
2. `workflow/config/loader` owns filesystem access and plugin package discovery.
   It may use configuration model types, but runtime session code must not load
   files.
3. `workflow/config/validation` owns static safety and schema checks. Runtime
   code may call validation APIs, but must not duplicate validation rules.
4. `workflow/config/evaluation` may project explicitly allowlisted values from
   runtime state. Expressions must never receive the complete runtime JSON tree
   by accident.
5. `engine` owns the View Engine protocol and concrete View implementations.
   `session` owns transitions and orchestration around those engines.
6. `input/editor` owns input editing semantics independently of ratatui.
   `ui/chrome` consumes the input model for rendering, never the reverse.
7. `ui/theme/model` and `ui/theme/color` must not read files. Only `ui/theme/load` may
   depend on the filesystem.
8. Engines may consume stable configuration queries. New code should not reach
   into `Config::compiled` or `config_value` directly.
9. New crate-visible fields are not a substitute for an API. Prefer private
   fields with constructors or focused accessors.
10. `task` owns generic background scheduling, cancellation, and task handles.
    It must not depend on a concrete engine or on picker item types. Task
    closures must cooperate with cancellation because runtime shutdown waits
    for the worker to exit; latest-wins replacement must use an explicit lane.
11. `session` may depend on the `ViewFactory` and generic `TaskRuntime`
    contracts, but it must not depend on `EngineRegistry` or
    `engine::picker` implementation types.
12. `execution` owns process and script mechanics. It may be used by command
    preparation and engine-specific task bodies, but it must not own task
    scheduling or engine semantics.
13. `workflow/config/validation` orchestrates engine validation through hooks;
    it must not branch on concrete engine identifiers for engine-specific
    field or cross-view semantics. Engine relation hooks are required rather
    than optional no-ops.

## Migration Order

### Completed in these migrations

- Extracted Chrome input editing, layout, and frame rendering.
- Extracted configuration model types, plugin loading, normalization, and
  validation while keeping `Config::load_app` and existing crate paths.
- Moved raw configuration loading and compilation into
  `workflow/config/loader.rs`; Theme loading and Engine validation are now
  coordinated by `app/cli.rs` while preserving the `Config::load*` call shapes.
- Added the narrow `EngineConfigValidator` contract so configuration validation
  no longer depends on the concrete `EngineRegistry` type.
- Added `EngineValidationContext` and required engine relation validation
  hooks. Picker items/feed rules and capture script-target rules now live with
  their engines; `workflow/config/validation.rs` retains only generic
  validation orchestration.
- Added owner-based TaskRuntime cleanup and lane-scoped latest-wins scheduling;
  picker item work is isolated per View ref and shutdown remains cooperative.
- Added the narrow `EngineTerminal` capability contract for View stepping;
  Embedded uses terminal size and Picker Preview uses image output.
- Restricted EngineHost input reads to named read-only queries.
- Moved the input editor model to `input/editor.rs`; `ui/chrome/input.rs` keeps
  the stable Chrome re-export while Engine and Session use the input domain.
- Added a terminal-owned Image Protocol type and perform the config-to-terminal
  mapping at the application composition boundary.
- Added `lib.rs` as the module composition root and reduced `main.rs` to the
  binary error/exit adapter.
- Grouped configuration, expression, query, command, navigation, and runtime
  modules under `workflow/`.
- Grouped Chrome and Theme under `ui/` while preserving the
  `crate::chrome::*` and `crate::theme::*` paths through narrow root exports.
- Split query schema compilation from query instance/state lifecycle in
  `workflow/query/schema.rs` and `workflow/query/instance.rs`.
- Split TOML/runtime normalization from filesystem loading into
  `workflow/config/normalize.rs`.
- Moved `ConfigSource` next to evaluation snapshots while re-exporting the
  existing `crate::config::ConfigSource` path.
- Moved serde model default helpers into `workflow/config/model.rs`.
- Extracted session stack/binding state and runtime publication helpers.
- Directoryized the shared runtime store under `workflow/runtime.rs`.
- Grouped process groups, bounded runners, and script execution under
  `execution/`.
- Directoryized the generic input subsystem under `input/`.
- Grouped cancellation and signal handling under `lifecycle/`.
- Grouped runtime diagnostics under `diagnostics/`.
- Moved Embedded Terminal implementation under `engine/embedded/` and Picker
  image resources under `engine/picker/preview/`.
- Extracted the generic task runtime to `task/`; picker item request
  adaptation and latest-wins behavior remain in `engine/picker/tasks.rs`.
- Added the `ViewFactory` protocol so Session owns an abstract View factory
  while the application composition root retains the concrete `EngineRegistry`.
- Moved command models and action preparation to `workflow/command/model.rs` and
  `workflow/command/prepare.rs`.
- Split theme models, color and scheme resolution, semantic bindings, and file
  loading into `ui/theme/model.rs`, `ui/theme/color.rs`, `ui/theme/binding.rs`, and
  `ui/theme/load.rs`.
- Moved Session input dispatch, route completion, reconciliation, and binding
  refresh to `session/input_dispatch.rs` while retaining transport in
  `session/input.rs`.
- Moved `ViewEffect` processing and input mutation to `session/effects.rs`.
- Moved navigation, call/return, rollback, and lifecycle transitions to
  `session/navigation.rs`.
- Moved Chrome assembly and route completion rendering to `session/chrome.rs`.
- Removed `Config`'s `Deref<Target = CompiledConfig>` façade. `CompiledConfig`
  is now private, Router uses named read-only queries, and test mutations use
  explicit `#[cfg(test)]` helpers.
- Moved evaluation scopes, public runtime projections, and dynamic value
  resolution to `workflow/config/evaluation.rs` while retaining the `crate::config`
  façade.
- Moved compiled configuration construction, view/feed expansion, and state
  registry setup to `workflow/config/compile.rs`.
- Moved generic configuration validation and dynamic requirement checks to
  `workflow/config/validation.rs`; engine-specific view and relation checks are
  dispatched through `EngineConfigValidator` hooks to the owning engines.
- Preserved the existing behavior contract with the current unit and
  integration tests.

The remaining `AppSession` implementation in `session/mod.rs` owns
construction, the main loop, runtime-log presentation, and stable façade
methods. Engine steps now consume the narrow `EngineTerminal` capability
contract; the concrete TTY adapter remains outside the Engine protocol.

Each step should keep the same `AppSession` constructors and run the focused
session tests plus the complete launcher suite.

### Next: boundary types and application composition

- Keep `workflow/` as an explicit application domain, with Config, Expression,
  Query, Command, Navigation, and Runtime as separate child boundaries.
- Do not introduce a generic `common/` catch-all alongside the domain tree.
- Keep CLI argument parsing and process bootstrap behind `app/`; `main.rs` is
  now a thin binary entry point.
- Extend `EngineTerminal` only when a new View requires a distinct terminal
  capability; do not expose the complete TTY adapter through the Engine API.
- Keep Router separate from both Session and Chrome because both consume its
  route models.

## Test Ownership

Tests should follow the invariant they protect:

- `workflow/config/model.rs`: serde shape and defaults.
- `workflow/config/loader.rs`: plugin discovery, merge precedence, disabled
  plugins, and path resolution.
- `workflow/config/normalize.rs`: keymap, engine-shape, disabled-plugin, and
  built-in command normalization.
- `workflow/query/schema.rs`: query types, field defaults, and input ordering.
- `workflow/query/instance.rs`: query state binding, updates, rendering, and
  validation.
- `workflow/config/validation.rs`: generic schema orchestration, command nesting,
  target safety, and template requirements.
- `engine/picker/mod.rs` and `engine/capture/mod.rs`: picker data-source/feed
  relations and capture script-source policies.
- `workflow/config/evaluation.rs`: scope composition, allowlisted runtime projections,
  dynamic value resolution, and evaluation budgets.
- `workflow/config/compile.rs`: compiled configuration construction, feed expansion,
  template bootstrap validation, and state registry setup.
- `input/editor.rs`: Unicode cursor and edit invariants.
- `ui/chrome/frame.rs`: clipping, footer overflow, layout geometry, and rendering
  styles.
- `ui/theme/model.rs`: serde shape and resolved theme composition.
- `ui/theme/color.rs`: palette, ANSI, scheme roles, and color reference errors.
- `ui/theme/binding.rs`: semantic binding names, defaults, and style patches.
- `ui/theme/load.rs`: named theme selectors and config-directory resolution.
- `input/keymap.rs`: binding priority, layer replacement, and tombstones.
- `session/input.rs`: input transport and passthrough parsing.
- `session/input_dispatch.rs`: input grammar, binding refresh, reconciliation,
  route completion, and editor dispatch.
- `session/effects.rs`: effect processing and input mutation.
- `session/navigation.rs`: push/replace/call/return and rollback.
- `session/chrome.rs`: Chrome assembly and route completion rendering.
- `workflow/runtime.rs`: shared runtime revisions and atomic publication.
- `execution/`: process cleanup, bounded commands, and script safety.
- `task/mod.rs`: generic task submission, completion, cancellation, and
  shutdown behavior.
- `terminal/` and `lifecycle/`: terminal lifecycle, cancellation, and signal
  restoration.
- `diagnostics/log.rs`: structured runtime log behavior.
- `tests/cli.rs`: configuration bootstrap and error ordering, including Theme
  precedence over dynamic configuration compilation errors.
- Integration tests: terminal lifecycle, process cleanup, and invocation output.

Before changing a boundary, run:

```text
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

A structural change is complete only when the public facade remains stable,
module dependencies point in the direction above, and the full suite passes.
