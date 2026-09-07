---
title: "Architecture Convergence Implementation Plan"
type: "concept"
tags:
  - architecture
  - protocol
  - ownership
  - lifecycle
  - migration
description: "Implementable convergence plan for execution lifecycle, protocol ownership, task correlation, configuration separation, and measured scheduling changes."
---

# Architecture Convergence Implementation Plan

## Status, Baseline, and Scope

This plan was agreed on 2026-09-07 against commit `ff50f2c` plus the reviewed working tree. It is a target design, not a claim that every guarantee already holds. The reviewed tree already contains useful protocol foundations: a Router with staged View construction, `ViewInstanceId`, lifecycle callbacks, `MountTaskLease`/`MountTaskStarter`, and Engine-local task-generation checks. Work must preserve those foundations rather than reimplement them.

Keep the single-process, single-crate architecture, the three built-in Engines, and the workflow/script extension model. This plan does not introduce a Cargo workspace, dynamic Engine ABI, multi-process host, or Tokio runtime. It also does not require renderer independence from Ratatui.

The implementation objective is a traceable path from input or task completion to a committed state change, with one identified owner for each mutable state and a test that exercises the production path.

## Decisions

| Area | Decision | Consequence |
| :--- | :--- | :--- |
| Navigation | Router is the sole owner of the View stack, active location, instance identity, and lifecycle transition commit. | Session and App must not mirror active location or mutate the stack. |
| View state | A View owns its query/editor, selection, publication, and task-generation state. | Shared chrome only reads committed location and View-provided metadata. |
| Tasks | The task runtime transports cancellation and correlated completion. A receiving View validates its own task and generation. | Router rejects unmounted instances; it does not interpret Engine-specific generation rules. |
| Execution | `execution` owns child process groups, I/O policy, output limits, timeout, cancellation, and reaping. | App coordinates terminal handoff but must not construct or wait on raw `Command` values. |
| Configuration | Immutable compiled workflow data is shared separately from invocation and View interaction state. | Factories and continuation handlers stop cloning a mixed `Config` object. |
| Scheduling | Concurrency changes require measured queue interference and cancellation latency. | A worker pool or separate lanes is not pre-approved by this document. |

## Current Gap Map

| Capability | Reviewed state | Remaining work |
| :--- | :--- | :--- |
| Navigation preparation and commit | Router stages target construction and preserves the prior stack on preparation failure. | Cover production Engine construction and lifecycle failures with contract tests. |
| Instance routing | Router drops task events for a closed or replaced instance. | Make correlation rules explicit for every Engine. |
| Generation checks | Picker and Capture reject mismatched task generations locally. | Standardize the registration and validation pattern; Embedded must state whether it has asynchronous task generations. |
| Background process execution | `execution::runner` provides bounded, cancellable process-group execution. | Route every bounded script/process entry point through it. |
| Foreground command execution | `app` currently waits directly with `Command::status()`. | Replace it with the foreground execution protocol below. |
| Task runtime | A single serialized worker provides latest-wins lanes and cooperative cancellation. | Add measurements before changing concurrency. |
| Configuration ownership | Compiled data and invocation parameters remain mixed in `Config`. | Introduce immutable compiled configuration and a distinct invocation context incrementally. |

## Ownership Model

| Component | Owns | Must not own |
| :--- | :--- | :--- |
| App | Terminal polling, drawing entry points, dependency composition, terminal handoff coordination, and process-wide cancellation | A second navigation stack or View-private interaction state |
| Session | Dispatch around Router, command-service invocation, effect-result adaptation, and diagnostics | An active-location copy or mutable Picker input |
| Router | View stack, active location, `ViewInstanceId`, transition commit, lifecycle sequencing, and mounted-instance routing | Engine selection state, task-generation semantics, or process mechanics |
| View | Interaction state, publications, local task registration, task-generation validation, event handling, and rendering | Direct navigation-stack mutation or another View's state |
| Task runtime | Scheduling, lane replacement, cooperative cancellation, task handles, timestamps, and completion delivery | Engine behavior, current-View selection, or navigation decisions |
| Execution | Process construction, environment clearing, process groups, I/O policy, timeout, cancellation, reaping, and execution result | Router transitions, task scheduling policy, or View state |
| Shared UI | Committed location and generic View metadata rendering | Query text, completion rows, selection, cursor position, or terminal surface state |

`PreparedAction` and `ViewDecision` remain separate concepts. The former is a workflow-command result; the latter is a host-level decision. `protocol` is the only conversion boundary between them.

The two existing `ViewContext` types must not be merged mechanically. The protocol context identifies a committed host View. The Engine context is an Engine-facing snapshot containing its input, parameters, runtime projection, publication, and revision. During migration, classify every field as host lifecycle data, View-owned projection, or compatibility data; use explicit conversions and rename genuinely distinct types where needed.

## Navigation Commit Contract

For push, replace, and call transitions, Router performs the following sequence:

1. Resolve the target and validate the target query without modifying committed state.
2. Allocate a new instance identity and construct the target View with an inert `MountTaskLease` only.
3. Deliver `Mounted` to the staged View. If construction or mounting fails, close the staged View as applicable, retain the original stack and footer location, and do not start work.
4. Cover the source View, then commit the new stack entry, location, and instance identity together.
5. Deliver `Activated` to the committed target. If activation fails before the transition is externally reported, restore the prior active View and report a rejected transition.
6. Create `MountTaskStarter` from the committed View's lease and authorize startup work only after commit.
7. Report transition completion to the source View. Callback failure is diagnostic only and cannot retroactively reject a committed transition.

A replace transition is atomic through source cleanup. A call/return transition resumes the recorded caller instance, not the View active when a result arrives. A failed preparation must leave the prior input, route label, task authority, and stack usable. A post-commit external effect is never rolled back implicitly; it must report failure and clean up resources according to its execution mode.

## Task Correlation and Delivery Contract

Every asynchronous result has the correlation tuple:

```text
(ViewInstanceId, TaskId, generation)
```

`TaskId` is scoped to the owning View instance. `generation` is a monotonically increasing revision for one logical task stream in that View. A View must register the current generation before submitting work and invalidate it before replacing input, closing, or starting newer work. The registration record is the authority for accepting a completion.

The delivery path is:

```text
View registers (task, generation)
  -> committed MountTaskStarter submits work with correlation
  -> TaskRuntime delivers TaskEvent(instance, task, generation, outcome)
  -> Router drops events for an unmounted instance
  -> owning View accepts only its current registered task and generation
```

The task runtime must attach the submitted correlation unchanged and record submission, start, cancellation-request, and terminal-completion timestamps. Router's mounted-instance check is a safety boundary, not a substitute for generation validation. A View receiving a task event for an unknown task or stale generation returns `Stay` and must not mutate state.

`Closing` cancels all work under that View's mount prefix. Cancellation is cooperative for Rust closures: task bodies must observe their token at bounded computation and I/O points. A cancellation request makes an eventual successful closure result ineligible for publication. Process-backed work additionally terminates and reaps the managed process group.

## Execution Lifecycle Contract

Execution modes share process-group cleanup but have different host behavior:

| Mode | Initiator and result path | Terminal policy | Completion policy |
| :--- | :--- | :--- | :--- |
| Background | View task through `TaskRuntime` | Host remains active | Deliver correlated `TaskEvent`; apply timeout and output bounds. |
| Embedded | Embedded View and PTY adapter | Embedded View owns its PTY surface | Stream/poll PTY state; close, cancel, and reap when the View closes. |
| Foreground | Host effect requested by a committed View | App suspends terminal before execution and restores it in a finally-style path | Return an explicit effect result after restoration; do not block Router dispatch through raw `Command::status()`. |

The foreground implementation is a protocol change, not a file move. Introduce an execution request with a declared mode and policy, then implement this state machine:

```text
Prepared -> Authorized -> TerminalSuspended -> Running
  -> {Completed | Failed | Cancelled | TimedOut}
  -> TerminalRestored -> ResultDelivered
```

`execution` owns transitions from `Running` to a terminal process outcome. App owns `TerminalSuspended` and `TerminalRestored`, including restoration after spawn failure, cancellation, timeout, child failure, or panic unwinding. The terminal restoration error takes precedence in diagnostics when the terminal cannot safely be returned to the user. Foreground requests must use a production `execution` API backed by `ProcessGroupGuard`; test-only process waiting APIs are not an implementation path.

Before replacing the current foreground branch, add tests proving cancellation reaps descendants, terminal restoration occurs on each terminal outcome, managed launcher environment is cleared, and no input/render dispatch occurs while terminal ownership belongs to the child.

## Configuration Ownership Contract

The target data split is conceptual and may use different final type names:

```text
Arc<CompiledConfig>       immutable workflows, routes, validation products, command definitions
InvocationContext         CLI target, initial parameters, stdin artifacts, caller environment facts
View interaction state    editor/query, selections, publications, local revisions, task registrations
```

`CompiledConfig` construction performs loading, normalization, static validation, and route-index construction once. It exposes narrow configuration query APIs to factories and command preparation. `InvocationContext` is created for each app launch and is never stored in the compiled configuration. A continuation captures only the immutable command definition and the necessary command snapshot; it must not clone all invocation state.

Migration must first identify every `Config` clone and classify it as compiled data, launch-time state, or View-owned mutable state. Compatibility constructors may bridge the transition, but new code must not add another mixed-state clone.

## Scheduling Measurement and Decision Gate

Keep the serialized worker until evidence supports a change. Instrument task runtime transitions with monotonic timestamps and tags for Engine, task class, lane, and cancellation reason. Collect at least:

- queue wait: submission to task-body start;
- cancellation latency: cancellation request to terminal completion;
- process reap latency: cancellation request to managed process reaping;
- queue depth, active task count, and completed/cancelled/failed totals;
- dropped task events, grouped by unmounted instance, unknown task, and stale generation.

Use a repeatable workload with a slow feed, repeated Picker edits, preview work, and normal input/render polling. Capture at least three runs with the same workload and report median and p95. The initial service objectives are p95 unrelated-lane queue wait at or below 250 ms and p95 managed-process cancellation-to-reap at or below 250 ms under the benchmark workload. These are targets, not claims about the current implementation.

Change concurrency only when measurements show a sustained objective violation or a documented throughput requirement. Any worker-pool or lane split must preserve latest-wins behavior within a lane, bounded queue growth, correlation validation, deterministic shutdown, and the same contract tests. A pool cannot forcibly interrupt an uncooperative closure.

## Migration Stages

| Stage | Status | Change boundary | Exit gate |
| :--- | :--- | :--- | :--- |
| 0. Baseline | Partial | Record commit, working-tree assumptions, context meanings, state writers, config clones, and execution entry points. | A checked-in inventory maps each mutable state and each execution path to an owner. |
| 1. Foreground lifecycle | Not started | Replace direct App `Command::status()` with the declared foreground execution state machine and production process-group API. | Cancellation, timeout, non-zero exit, spawn failure, and restoration failure are covered on the production path. |
| 2. Task correlation | Partial | Extract or standardize the View-local registration and generation-validation helper; document Embedded behavior. | Picker, Capture, and Embedded either validate the tuple or explicitly declare no asynchronous task stream; stale results cannot mutate state. |
| 3. Configuration split | Not started | Introduce immutable compiled configuration and invocation context behind narrow query APIs. | Factories, services, and continuations no longer clone mixed configuration/invocation state. |
| 4. Protocol placement | Partial | Move contracts, Router/session orchestration, and configuration adapters only after ownership tests pass. | One Router commit path remains; contracts create neither concrete Engines nor processes; module movement preserves behavior. |
| 5. Scheduler decision | Not started | Add telemetry and benchmark before selecting serialized worker retention, bounded pool, or lane split. | Decision record includes measurements; any implementation satisfies queue, cancellation, and resource-bound tests. |
| 6. Documentation alignment | Not started | Update architecture maps and guarantees from verified implementation. | Documentation names the baseline, implemented guarantees, trust boundary, and remaining limitations accurately. |

Each stage preserves observable CLI behavior, workflow configuration, key dispatch, rendering, and call/return behavior unless a separately approved change says otherwise. Stages 1 and 2 are independent enough to proceed in either order; stage 3 must not block lifecycle fixes.

## Required Contract Coverage

Map existing tests to these scenarios before deleting or replacing them. Add production-path tests where unit tests only exercise adapters.

- Failed target resolution, query validation, View construction, mount, cover, activation, and replacement cleanup retain or restore the intended committed state.
- No task starts before commit, and preparation failure retains usable task authority for the prior View.
- Push, replace, popup close, call, return, and nested call distinguish active, covered, and unmounted instances correctly.
- A return continuation resumes the recorded caller with its committed snapshot; failure retains sufficient state for retry or reporting.
- Closed/replaced instances ignore late task events, and stale `(TaskId, generation)` events cannot overwrite a newer View publication.
- Picker, Capture, and Embedded obey the shared mounted-instance and lifecycle contract, while preserving their distinct input and I/O behavior.
- Foreground and bounded background process cancellation reaps managed children and descendants; terminal restoration is verified for foreground and embedded execution where applicable.
- Background work does not block normal input/render dispatch. Foreground execution has an explicit suspension interval and does not resume host rendering until restoration completes.
- Metrics are emitted under benchmark load, and any scheduling change demonstrates bounded queue/resource growth and preserves latest-wins behavior.
- Documentation never describes path confinement, output budgets, or process groups as an operating-system sandbox. Workflow scripts are trusted code executed with the current user's permissions.

The completion criterion is not a directory, trait, or line-count target. It is an owned, observable, and tested event-to-commit path for navigation, task delivery, and external execution.
