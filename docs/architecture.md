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
      compile.rs            template/parameter registry compilation
      validation.rs         schema and command validation
      evaluation.rs         evaluation scopes and runtime projection
    expression/
      mod.rs                bounded dynamic expression language
    parameter/
      mod.rs                ParameterRegistry, ParameterBinding, and ParameterState facade
      schema.rs              parameter field types and schema compilation
      state.rs               parameter state lifecycle, binding, and updates
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
    navigation.rs           push/replace/call/return and prepared Host transition/commit
    effects.rs              ViewEffect processing and input mutation
    chrome.rs               Chrome assembly and route completion rendering
    state.rs                session stack and binding state types
    publication.rs          runtime publication

  engine/
    mod.rs
    api.rs
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
about engines or picker items. One `AppSession` owns one serialized FIFO worker
shared by every mount and lane. Lanes scope latest-wins replacement and
cancellation; they do not provide parallel execution. A task that does not
observe cooperative cancellation promptly blocks later work across mounts.
Task closures have a cooperative cancellation contract: the Session-owned
shutdown path cancels queued and active work and joins the worker. Concurrent
shutdown callers and worker-initiated shutdown are outside the runtime contract.
Latest-wins replacement is lane-scoped; picker-specific item request adaptation owns its
`picker-items:<view_ref>` lane in `engine/picker/tasks.rs`. Mount setup receives
only an inert `MountTaskLease`; Session creates the post-commit
`MountTaskStarter` from the matching mount identity, and prepared jobs reject a
starter from another mount before scheduler submission. Picker preview decode
uses a separate bounded pool per mount. Task closures must not capture the
owning Engine/Session and rely on owner-drop cancellation.
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
    parameter/                parameter schemas and state instances
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
    `engine::picker` implementation types. Root mounts use one private
    construction path; only the default-view invocation state and input
    routing policy differ between session entry points.
12. `execution` owns process and script mechanics. It may be used by command
    preparation and engine-specific task bodies, but it must not own task
    scheduling or engine semantics.
13. `EngineRegistry` is a closed dispatcher for the three built-in Engines.
    Engine-specific validation remains in each Engine module; adding a built-in
    Engine requires one explicit dispatch branch. Test-only registrations may
    replace a branch for failure injection but are not a production plugin API.

## Migration Order

### Completed in these migrations

- Replaced the production Engine registration map with closed dispatch for
  Picker, Capture, and Embedded; dynamic registrations remain test-only.
- Collapsed Picker mount preparation from two `Any` boundaries and an opaque
  setup closure to one Engine-owned runtime-data bundle built with an inert
  task lease.
- Reduced `ViewContextIdentity` to mount identity plus one context revision;
  Picker retains its private request/input/parameter generations.
- Removed the unused shared runtime `RwLock` mirror and source-text architecture
  tests. Runtime snapshots are passed explicitly to task submissions.
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
- Kept Embedded terminal polling and Picker preview decode in their
  mount-owned runtime state; renderers consume immutable render snapshots and
  create only frame-local drawing details.
- Replaced the broad Engine boundary with explicit EngineRuntime inputs,
  mount-owned raw receivers, and structured EngineDecision output.
- Moved the input editor model to `input/editor.rs`; `ui/chrome/input.rs` keeps
  the stable Chrome re-export while Engine and Session use the input domain.
- Added a terminal-owned Image Protocol type and perform the config-to-terminal
  mapping at the application composition boundary.
- Added `lib.rs` as the module composition root and reduced `main.rs` to the
  binary error/exit adapter.
- Grouped configuration, expression, parameter, command, navigation, and runtime
  modules under `workflow/`.
- Grouped Chrome and Theme under `ui/` while preserving the
  `crate::chrome::*` and `crate::theme::*` paths through narrow root exports.
- Completed the internal Query-to-Parameter migration under
  `workflow/parameter/`, with `schema.rs` and `state.rs`; the external
  configuration key remains `query` for compatibility.
- Split TOML/runtime normalization from filesystem loading into
  `workflow/config/normalize.rs`.
- Moved `ConfigSource` next to evaluation snapshots while re-exporting the
  existing `crate::config::ConfigSource` path.
- Moved serde model default helpers into `workflow/config/model.rs`.
- Added behavior checks for factory context ownership, renderer immutability,
  generic session boundaries, live Engine event handling,
  root assembly, and picker renderer ownership. A registration-owned
  `MountPlanDataFactory` may inspect complete `Config` to compile opaque
  target-specific data; generic `MountPlanFactory`, runtime, renderer, binding,
  and mount-service contexts cannot inspect complete `Config`. Ordinary Engine
  events mutate the mounted Engine's private state directly and return an
  `EngineEmission` containing only a decision and optional publication. Host
  preflight and Host-owned state/effect commits happen afterward and do not
  roll back the Engine mutation.
  Normal Engine decisions commit reports and an atomic runtime batch before
  processing structural Host effects. Pop and Return
  consume the child stack before sequential parent lifecycle/continuation
  phases, and only a mount that remains active starts prepared work. Capability
  start is infallible after committed mount validation. Embedded's PTY drive remains explicitly
  external and irreversible. Root and navigation mount construction use the
  shared private preparation path, and root Session assembly is also shared.
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
- Moved navigation, call/return, Host transition preparation, and lifecycle
  transitions to `session/navigation.rs`; ordinary lifecycle events update live
  Engine-private state directly, while Host-owned transition state retains its
  local preparation and ordered commit rules.
- Moved Chrome assembly and route completion rendering to `session/chrome.rs`.
- Removed `Config`'s `Deref<Target = CompiledConfig>` façade. `CompiledConfig`
  is now private, Router uses named read-only queries, and test mutations use
  explicit `#[cfg(test)]` helpers.
- Moved evaluation scopes, public runtime projections, and dynamic value
  resolution to `workflow/config/evaluation.rs` while retaining the `crate::config`
  façade.
- Moved compiled configuration construction, view/feed expansion, and
  parameter registry setup to `workflow/config/compile.rs`.
- Moved generic configuration validation and dynamic requirement checks to
  `workflow/config/validation.rs`; engine-specific view and relation checks are
  dispatched through `EngineConfigValidator` hooks to the owning engines.
- Preserved the existing behavior contract with the current unit and
  integration tests.

The remaining `AppSession` implementation in `session/mod.rs` owns
construction, the main loop, runtime-log presentation, and stable façade
methods. Engine runtimes receive mount snapshots and explicit event/tick inputs,
mutate only their own private state, and never receive `Session` or Host-owned
state. Renderers consume immutable models while the concrete TTY adapter,
navigation, runtime publication, and effects remain outside the Engine protocol.

Overall migration status is **Incomplete** for one concrete reason: Rust
package boundaries cannot statically enforce every architecture policy in this
single crate. Embedded now has an inert prepared start plan followed by an
explicit post-Host-commit external PTY boundary; the child, descriptors, resize,
and input I/O remain intentionally irreversible and are not presented as
rollback-capable. Host transitions use local atomic validation with ordered
commits rather than cross-Engine/stack rollback; internal parameter terminology
is now consolidated while the compatible `view.query` configuration key
remains unchanged.

Each step should keep the same `AppSession` constructors and run the focused
session tests plus the complete launcher suite.

### Next: boundary types and application composition

- Keep `workflow/` as an explicit application domain, with Config, Expression,
  Parameter, Command, Navigation, and Runtime as separate child boundaries.
- Do not introduce a generic `common/` catch-all alongside the domain tree.
- Keep CLI argument parsing and process bootstrap behind `app/`; `main.rs` is
  now a thin binary entry point.
- Extend mount-owned capabilities only when a new View requires a distinct
  terminal or external resource; do not expose the complete TTY adapter through
  the Engine API.
- Keep Router separate from both Session and Chrome because both consume its
  route models.

## Test Ownership

Tests should follow the invariant they protect:

- `workflow/config/model.rs`: serde shape and defaults.
- `workflow/config/loader.rs`: plugin discovery, merge precedence, disabled
  plugins, and path resolution.
- `workflow/config/normalize.rs`: keymap, engine-shape, disabled-plugin, and
  built-in command normalization.
- `workflow/parameter/schema.rs`: parameter types, field defaults, and input
  ordering.
- `workflow/parameter/state.rs`: parameter state binding, updates, rendering,
  and validation.
- `workflow/config/validation.rs`: generic schema orchestration, command nesting,
  target safety, and template requirements.
- `engine/picker/mod.rs` and `engine/capture/mod.rs`: picker data-source/feed
  relations and capture script-source policies.
- `workflow/config/evaluation.rs`: scope composition, allowlisted runtime projections,
  dynamic value resolution, and evaluation budgets.
- `workflow/config/compile.rs`: compiled configuration construction, feed expansion,
  template bootstrap validation, and parameter registry setup.
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
- `session/effects.rs`: effect processing, preflight, and input mutation.
- `session/navigation.rs`: push/replace/call/return and prepared Host commits.
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
