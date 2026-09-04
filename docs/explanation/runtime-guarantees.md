---
title: "Runtime Guarantees and Safety Limits"
type: "concept"
tags:
  - security
  - guarantees
  - limits
  - sandbox
description: "Explanation of security boundaries, resource budgets, execution sandboxing, and terminal state invariants."
---

# Runtime Guarantees and Safety Limits

`tui-launcher` enforces strict execution limits and sandboxing policies to guarantee terminal integrity, system responsiveness, and bounded resource consumption.

## 1. Filesystem & Plugin Isolation

Plugins operate under strict directory boundaries:
- **Root Confinement**: All script references (`source = "script", file = "..."`) are resolved and validated strictly within the plugin directory root (`$XDG_CONFIG_HOME/tui-launcher/plugins/<plugin-id>/`).
- **Path Traversal Prevention**: Relative path traversal attempts (e.g., `../../etc/passwd`) or symlink escapements are caught during configuration validation (`--check`) and runtime startup.
- **Special File Rejection**: The launcher refuses to load configuration or script files from device nodes, sockets, or named pipes.

## 2. Process Execution Boundaries

External scripts spawned by command handlers or dynamic feeds are subject to bounded execution parameters:

| Resource | Default Limit | Maximum / Configurable Limit |
| :--- | :--- | :--- |
| **Execution Timeout** | 10 seconds | Configurable per command |
| **Argument Vector (`argv`)** | 64 KiB total | Fixed safety bound |
| **Standard Error (`stderr`)**| 64 KiB total | Fixed diagnostic buffer |
| **Standard Output (`stdout`)**| 1 MiB | Up to 64 MiB for picker feeds |
| **Template Budget** | 16 MiB | Shared memory evaluation ceiling |

## 3. Process Group Lifecycle & Cleanup

Terminal applications often risk leaving orphaned child processes when interrupted:
- **Process Groups**: When an embedded process or script handler is spawned, it is placed in its own dedicated process group.
- **Cooperative Shutdown**: Upon view cancellation or normal exit, signals (`SIGTERM`, followed by `SIGKILL` if unresponsive) are propagated to the entire process group.
- **Zombie Reaping**: Child processes are cleanly reaped to prevent process table leakage.

## 4. Terminal State Restoration Invariants

Messing up the user's terminal state (e.g. leaving raw mode active or the alternate screen unreleased) is a fatal user-experience failure:
- **Panic Hooks**: The launcher installs custom panic hooks ensuring that raw mode is disabled, the alternate screen buffer is vacated, and the cursor is restored before printing any stack trace.
- **Signal Handlers**: Handlers for `SIGINT`, `SIGTERM`, and `SIGHUP` execute the full terminal teardown sequence.
