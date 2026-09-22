---
title: "Runtime Guarantees and Safety Limits"
type: "concept"
tags:
  - security
  - guarantees
  - limits
  - trust-boundary
  - producers
description: "Explanation of workflow trust boundaries, producer resource budgets, process cleanup, and terminal state invariants."
---

# Runtime Guarantees and Safety Limits

`tflow` applies execution limits and process-cleanup policies to protect terminal integrity, responsiveness, and bounded resource consumption. These controls do not sandbox workflow code.

## 1. Filesystem Boundaries and Trusted Workflows

Workflow scripts execute with the current user's permissions and must be treated as trusted code. Directory confinement, special-file rejection, resource limits, and process groups reduce accidental exposure and resource abuse; they do not provide operating-system isolation from a malicious workflow.

Workflows operate under these boundaries:

- **Directory workflow confinement**: In `workflows/<workflow-id>/workflow.toml`, producer `handler.file` paths are resolved and validated within the workflow root.
- **Single-file isolation**: A single-file workflow cannot reference a relative external script file. Use a producer `handler.script` or an absolute host binary/file path.
- **Caller working directory**: Host process invocations preserve the caller's `$PWD`. Directory workflows receive `$TFLOW_WORKFLOW_DIR` for companion resources.
- **Inline materialization**: Multi-line inline scripts are materialized under `$XDG_RUNTIME_DIR/tflow/scripts/` (with cache fallback) using `0600` permissions and source attribution comments. The host interprets shebang arguments and can invoke scripts from a `noexec` filesystem.
- **Path traversal prevention**: Relative traversal attempts and symlink escapes are rejected during `--check` and runtime preparation.
- **Special file rejection**: Configuration and script files cannot be device nodes, sockets, or named pipes.

All configuration is static and producer handlers are data. Runtime data enters scripts only through their documented JSON request.

## 2. Producer Protocol and Process Limits

Producer scripts receive one JSON request on stdin and must write one complete version-1 JSON response on stdout. Stderr is reserved for diagnostics.

| Resource | Policy |
| :--- | :--- |
| **Execution timeout** | 10 seconds for bounded scripts |
| **Argument vector** | 64 KiB total safety bound |
| **Standard error** | 64 KiB |
| **Standard output** | 1 MiB for command and Capture producers |
| **Picker item producer stdout** | 64 MiB, to support large candidate sets |

Producer stdout is strict: surrounding whitespace is allowed, but malformed JSON, a second JSON document, unknown fields, an unsupported version, a nonzero exit status, or a response with the wrong operation type is failure. No failed protocol response is applied. Capture and command producers use the default stdout bound; Picker items use the larger bound.

## 3. Process Group Lifecycle and Cleanup

When a bounded script or external process is spawned:

- It is placed in a managed process group.
- Cancellation, timeout, or View close terminates the group, escalating from `SIGTERM` to `SIGKILL` when required.
- The launcher waits for and reaps the managed child before reporting the task terminal state.
- A task result that is stale, cancelled, or associated with an unmounted View cannot publish state even if the process completed.

Capture output work begins after the Capture View mounts. Return processor work begins only after the child closes and the recorded caller is active. A provider or processor failure does not reopen a closed View or retry a potentially side-effecting script.

## 4. Terminal State Restoration Invariants

Foreground and Embedded processes have distinct terminal policies, but every path must restore launcher-owned terminal state:

- **Foreground effects**: The application suspends terminal ownership for the child, waits through completion/cancellation, then restores terminal settings before resuming TUI polling and rendering.
- **Embedded effects**: The Embedded View owns its PTY surface and closes/cancels/reaps its process when the View closes.
- **Panic hooks**: Raw mode is disabled, the alternate screen is vacated, and the cursor is restored before a panic report is printed.
- **Signal handling**: `SIGINT`, `SIGTERM`, and `SIGHUP` trigger the terminal teardown sequence.

A restoration failure is a host-level failure, not a normal in-TUI producer diagnostic.

## 5. What These Guarantees Do Not Provide

The launcher does not sandbox workflow code, roll back script side effects, or limit what a trusted script can do with the user's permissions. Resource bounds constrain launcher-managed I/O and process lifetime; they are not security isolation.

For the exact workflow and producer schema, see [workflow.toml Specification](../reference/workflow-toml.md). For literal values and runtime data boundaries, see [Producer Protocol](../reference/producer-protocol.md).
