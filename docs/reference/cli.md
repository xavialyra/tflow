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
tlaunch inspect <VIEW>
```

## Global Options

Global options must precede the target view selector.

| Option | Environment Variable | Description |
| :--- | :--- | :--- |
| `-c, --config <PATH>` | `TLAUNCH_CONFIG` | Path to the root `config.toml`. Overrides default search paths. |
| `--check` | — | Validates the configuration files and workflow manifests without launching the interactive TUI. Returns non-zero on error. |
| `--inspect <VIEW>` | — | Prints view contract details (alias, engine, queries, commands) and exits. |
| `-h, --help` | — | Prints version and usage information. |
| `-V, --version` | — | Prints the version of `tlaunch`. |

## Configuration Search Precedence

The launcher resolves the active configuration file in the following order:

1. `--config PATH` argument.
2. `TLAUNCH_CONFIG` environment variable.
3. `$XDG_CONFIG_HOME/tlaunch/config.toml`.
4. `$HOME/.config/tlaunch/config.toml`.

## Child Process Environment

Child processes inherit the caller's environment. The launcher adds only these workflow-specific variables:

- `WORKFLOW_DIR` for scripts, foreground `run` commands, and Embedded processes belonging to a directory workflow. It contains that workflow's root directory.
- `LAUNCHER_INPUT` for Embedded processes only. It contains the current View input projection.

Producer scripts receive query, selection, command, and return data as their documented JSON request on stdin. That data is not copied into launcher-specific environment variables. Ordinary caller variables such as `PATH`, `HOME`, `TERM`, and locale settings remain inherited, and the launcher does not proactively clear pre-existing variables with other names.

## Direct View Invocation

You can open a specific view directly instead of the configured `default_view`:

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
tlaunch dmenu:main --index=true --prompt="Choose item:"
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

## CLI Symlink Multiplexing (`argv[0]`)

`tlaunch` supports direct entry point multiplexing based on `argv[0]`:

When `tlaunch` is invoked via a symlink, hardlink, or copy whose file stem matches a declared view alias or `<workflow-id>:<view-name>`:
1. The matching view is resolved and launched immediately.
2. Any trailing command-line flags or arguments are directly bound to the target view's query schema.

For example, create a symlink to an alias:
```bash
ln -s $(which tlaunch) ~/.local/bin/dmenu
```
Running `dmenu --prompt="Select:"` executes `tlaunch` directly into the `dmenu` view with `--prompt="Select:"` bound to the view's query parameters without manual CLI dispatch flags.
