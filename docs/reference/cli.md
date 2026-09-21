---
title: "Command-Line Interface (CLI) Reference"
type: "reference"
tags:
  - cli
  - arguments
  - flags
  - configuration
description: "Authoritative reference for tlaunch command-line options, environment variables, and direct invocation."
---

# Command-Line Interface (CLI) Reference

## Synopsis

```text
tlaunch [OPTIONS] [VIEW] [VIEW_ARGUMENTS...]
tlaunch -w <WORKFLOW_PATH> [OPTIONS] [VIEW] [VIEW_ARGUMENTS...]
tlaunch -s <SUITE_PATH> [OPTIONS] [VIEW] [VIEW_ARGUMENTS...]
tlaunch inspect <VIEW>
tlaunch inspect --all
```

## Global Options

Global options must precede the target view selector.

| Option | Environment Variable | Description |
| :--- | :--- | :--- |
| `-c, --settings <PATH>` | `TLAUNCH_SETTINGS` | Passive host settings; `--config` is an alias for this flag. |
| `-s, --suite <PATH>` | `TLAUNCH_SUITE` | Explicit suite manifest; mutually exclusive with `-w`. |
| `-w, --workflow <PATH>` | — | Path to a single-file workflow (`.toml`) or a directory workflow package. Uses its local entrypoint and inherits host settings. `-` reads a workflow from stdin. |
| `--theme <THEME>` | — | Explicit theme selector (`terminal` or custom name). Overrides configured theme. |
| `--check` | — | Validates the configuration files and workflow manifests without launching the interactive TUI. Returns non-zero on error. |
| `--inspect <VIEW>` | — | Prints view contract details (alias, engine, queries, commands) and exits. |
| `--all` | — | Used with `inspect` to dump contracts for all configured views. |
| `-h, --help` | — | Prints version and usage information. |
| `-V, --version` | — | Prints the version of `tlaunch`. |

## Configuration Search Precedence

Settings resolve from `--settings`, then `TLAUNCH_SETTINGS`, then
`$XDG_CONFIG_HOME/tlaunch/settings.toml` (or `$HOME/.config/tlaunch/settings.toml`
when XDG_CONFIG_HOME is unset). Absent default settings use built-in defaults.

Without `-w` or `-s`, the launcher loads `TLAUNCH_SUITE` when set, otherwise
`$XDG_CONFIG_HOME/tlaunch/default.toml` (or `$HOME/.config/tlaunch/default.toml`).
Workflow directories are never scanned for implicit membership. Suite manifests
explicitly mount their members; suites cannot mount suites.

## Isolated Workflow Execution (`-w, --workflow`)

The `-w, --workflow <PATH>` option allows executing an isolated workflow directly without installing it into the system or user configuration directory.

### Target Formats

1. **Single-file workflow (`.toml`)**:
   - The file stem becomes the workflow ID (e.g. `dmenu.toml` -> workflow `dmenu`).
   - The workflow root is set to the directory containing the `.toml` file.
   - Scripts referenced by relative paths in the manifest are resolved relative to this directory.
2. **Directory workflow package**:
   - Points to a directory containing a `workflow.toml` for one atomic workflow.
   - Sets `WORKFLOW_DIR` to the package directory.

### Entry Point Resolution in Workflow Mode

An explicit view selector overrides `[workflow].entrypoint`. Without a selector,
the required workflow-local `entrypoint` names the initial view. Workflow views
cannot declare aliases; suites own `[aliases]` and member shorthand routes.

`-w` rejects suite manifests and `-s` rejects atomic workflows, with a corrective
flag hint. `-w -` reads TOML into memory; interactive rendering and keyboard input
use `/dev/tty`. Invocation results go to stdout.

### Shebang / Executable Script Integration

Single-file workflows can be made directly executable by adding a `tlaunch -w` Shebang at the top of the file:

```toml
#!/usr/bin/env -S tlaunch -w
[workflow]
api = 1
name = "quick-picker"
entrypoint = "main"

[views.main.engine]
type = "picker"
...
```

Make the file executable (`chmod +x quick-picker.toml`) and execute it directly: `./quick-picker.toml [ARGS...]`.

## Child Process Environment

Child processes inherit the caller's environment. The launcher adds only these workflow-specific variables:

- `WORKFLOW_DIR` for scripts, foreground `run` commands, and Embedded processes belonging to a directory workflow. It contains that workflow's root directory.
- `LAUNCHER_INPUT` for Embedded processes only. It contains the current View input projection.

Producer scripts receive query, selection, command, and return data as their documented JSON request on stdin. That data is not copied into launcher-specific environment variables. Ordinary caller variables such as `PATH`, `HOME`, `TERM`, and locale settings remain inherited, and the launcher does not proactively clear pre-existing variables with other names.

## Direct View Invocation

Without an explicit view, suite execution uses `[suite].entrypoint`. A member key resolves to that member’s workflow entrypoint. Explicit aliases are declared in the suite’s `[aliases]` table.

You can also open a specific view directly:

```bash
tlaunch <workflow-id>:<view-name> [ARGUMENTS...]
# Or using an alias:
tlaunch <alias> [ARGUMENTS...]
```

### Direct Invocation Arguments

Arguments passed after the target view are matched against the target view's declared `[views.<name>.query]` schema:

- **String values**: `--name=value`
- **JSON values**: `--name:=JSON` (e.g. `--count:=10` or `--tags:='["rust", "tui"]'`)
- **Boolean flags**: `--flag` (sets `flag = true`) or `--no-flag` (sets `flag = false`)

```bash
tlaunch dmenu:main --index=true
```

## Contract Inspection (`inspect`)

You can inspect the declared interface and routing contract of any view without running the interactive TUI:

```bash
tlaunch inspect <workflow-id>:<view-name>
# Or using an alias or flag:
tlaunch inspect <alias>
tlaunch --inspect <alias>
```

This prints formatted details including:
- Workflow ID and view name
- Route alias
- Engine type (e.g. `picker`, `capture`, `embedded`)
- Declared query parameters and types
- Configured commands and keybindings

To inspect every configured View, use:

```bash
tlaunch inspect --all
```

The result has a top-level `views` array. Entries are ordered by canonical View reference and use the same contract fields as a single-View inspection. The list includes every configured Engine type; producer scripts are not executed.

## CLI Symlink Multiplexing (`argv[0]`)

`tlaunch` supports direct entry point multiplexing based on `argv[0]`:

When `tlaunch` is invoked via a symlink, hardlink, or copy whose file stem matches a declared view alias or `<workflow-id>:<view-name>`:
1. The matching view is resolved and launched immediately.
2. Any trailing command-line flags or arguments are directly bound to the target view's query schema.

For example, create a symlink to an alias:
```bash
ln -s $(which tlaunch) ~/.local/bin/dmenu
```
Running `dmenu` executes `tlaunch` directly into the `dmenu` view without manual CLI dispatch flags.
