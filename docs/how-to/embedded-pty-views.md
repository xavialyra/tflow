---
title: "How to Embed Interactive Shells and PTY Programs"
type: "guide"
tags:
  - embedded
  - pty
  - terminal
  - interactive
description: "How to wrap interactive terminal applications with the embedded engine and handle raw PTY I/O."
---

# How to Embed Interactive Shells and PTY Programs

The `embedded` engine allows you to host an arbitrary child process inside a view, allocating a pseudo-terminal (PTY) and rendering its raw ANSI/VT escape sequences directly into the launcher's layout.

## Problem

You want to run an interactive terminal utility (such as `lazygit`, `htop`, a repl, or an interactive shell) inside the launcher while preserving launcher-level overlays or capturing its output.

## Solution

### 1. Configure the Embedded Engine

Declare a view with `engine.type = "embedded"`:

```toml
[views.terminal.engine]
type = "embedded"

[views.terminal.engine.config]
command = ["sh", "-lc", "bash"]
escape-cancels = true
```

- **Byte-for-byte forwarding**: All keystrokes unmatched by higher-level session commands are sent directly to the child process's PTY.
- **Escape Handling**: When `escape-cancels = true` (default), pressing `Esc` cancels the view and unwinds the view stack. Set `escape-cancels = false` if the child process (e.g. `vim`) requires `Esc` for normal operation.

### 2. Capture Result Data from PTY

If your embedded view runs a tool that outputs structured results upon exit, enable `result` capture:

```toml
[views.query_builder.engine]
type = "embedded"

[views.query_builder.engine.config]
command = ["my-cli-wizard"]
result = { format = "json", required = true, max_bytes = 1048576 }
```

When the child process exits successfully, the launcher captures up to `max_bytes` from the result descriptor, parses it according to `format` (`json` or `text`), and passes it to subsequent command handlers or return boundaries.

### 3. Passthrough and Command Precedence

Session commands (such as `ctrl+k` for the command palette) still take precedence over the embedded child PTY. If you trigger an overlay, the child process continues running in the background while input is directed to the overlay.
