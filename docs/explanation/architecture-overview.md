---
title: "Architecture Overview and Domain Boundaries"
type: "concept"
tags:
  - architecture
  - domains
  - design
  - boundaries
description: "High-level overview of tui-launcher's domain separation, module layout, and 14 architectural dependency rules."
---

# Architecture Overview and Domain Boundaries

`tui-launcher` is designed as an extensible terminal workflow host. Its long-term architectural structure strictly separates **configuration**, **session state**, **engine behavior**, and **terminal rendering** into distinct domains.

A module may expose a small crate-level facade, but code must live in the narrowest domain that owns its behavior.

## Core Domain Layout

The layout below describes architectural roles, not an exact inventory of the current filesystem. In the reviewed implementation, routing and session orchestration still live largely in `src/view.rs` and `src/protocol.rs`. The [Architecture Convergence implementation plan](architecture-convergence.md) records the reviewed baseline, target ownership, incremental stages, and contract gates.

```text
src/
  lib.rs
  main.rs

  app/
    mod.rs                    Top-level startup and App facade
    cli.rs                    Args, default config path, and bootstrap
    invocation.rs             Stdin capture and invocation result adaptation

  workflow/
    config/                   Loading, normalization, compilation, evaluation
    expression/               Bounded dynamic expression language
    parameter/                Parameter schemas and state instances
    command/                  Command contracts and preparation
    navigation.rs             Router and route resolution
    runtime.rs                Shared workflow runtime store

  session/                    Stateful workflow execution and orchestration
  engine/                     View Engine protocol and concrete implementations
  input/                      Terminal input model and layered keymaps
  ui/                         Chrome and Theme presentation modules
    chrome/                   ContentHost and shared Footer presentation
  terminal/                   TTY and terminal-control adapter
  execution/                  External process and script execution
  task/                       Generic background task lifecycle and scheduling
  lifecycle/                  Cancellation and signal lifecycle
  diagnostics/                Structured runtime logging
```

## Architectural Dependency Rules

The following 14 dependency rules define the intended architecture. They are design constraints, not a claim that every current module or test already enforces them. The [convergence design](architecture-convergence.md) records known gaps and refines the target protocol ownership:

1. **`workflow/config/model`** contains purely data and `serde` definitions. It must not depend on `session`, `ratatui`, or terminal I/O.
2. **`workflow/config/loader`** owns filesystem access and workflow package discovery. Runtime session code must never load files directly.
3. **`workflow/config/validation`** owns static safety and schema checks. Runtime code may invoke validation APIs, but must not duplicate validation logic.
4. **`workflow/config/evaluation`** projects explicitly allowlisted values from runtime state. Expressions must never receive the complete runtime JSON tree.
5. **`engine`** owns the View Engine protocol and concrete View implementations. The runtime `Router` owns View navigation and transitions; `session` owns the terminal host and orchestration around the `Router`.
6. **`ui/chrome`** is split conceptually into `ContentHost` and `Footer`:
   - `ContentHost` owns framing, inline/popup placement, and view content rendering.
   - `Footer` consumes committed `Router` location and generic View metadata for status, errors, and command hints.
   - Neither component owns or renders View-private input, query text, completion rows, or cursor state.
7. **Interactive query editing is Picker-owned**. Terminal byte decoding and event transport remain independent infrastructure; shared UI must not own or duplicate Picker input state.
8. **`ui/theme/model` and `color`** must not read files. Only `ui/theme/load` may depend on the filesystem.
9. **Engines consume stable configuration queries**. Code should not reach into `Config::compiled` or `config_value` directly.
10. **Encapsulation over visibility**: New crate-visible fields are not a substitute for an API; prefer private fields with focused constructors and accessors.
11. **`task` owns generic background scheduling, cancellation, and task handles**. It must not depend on a concrete engine or picker item types. Task closures must cooperate with cancellation; latest-wins replacement must use an explicit lane.
12. **`session` depends on abstract contracts** (`ViewFactory` and `TaskRuntime`), not on concrete `EngineRegistry` or `engine::picker` types.
13. **`execution` owns process and script mechanics**. It may be used by command preparation and task bodies, but must not own task scheduling or engine semantics.
14. **`EngineRegistry` is a closed dispatcher** for the three built-in Engines (`picker`, `capture`, `embedded`). Engine-specific validation remains inside each Engine module.
