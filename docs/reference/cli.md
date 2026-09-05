---
title: "Command-Line Interface (CLI) Reference"
type: "reference"
tags:
  - cli
  - arguments
  - flags
  - configuration
description: "Authoritative reference for tui-launcher command-line options, environment variables, and direct invocation."
---

# Command-Line Interface (CLI) Reference

## Synopsis

```text
tui-launcher [OPTIONS] [VIEW] [VIEW_ARGUMENTS...]
tui-launcher inspect <VIEW>
```

## Global Options

Global options must precede the target view selector.

| Option | Environment Variable | Description |
| :--- | :--- | :--- |
| `-c, --config <PATH>` | `TUI_LAUNCHER_CONFIG` | Path to the root `config.toml`. Overrides default search paths. |
| `--check` | — | Validates the configuration files and workflow manifests without launching the interactive TUI. Returns non-zero on error. |
| `--inspect <VIEW>` | — | Prints view contract details (alias, engine, queries, commands) and exits. |
| `-h, --help` | — | Prints version and usage information. |
| `-V, --version` | — | Prints the version of `tui-launcher`. |

## Configuration Search Precedence

The launcher resolves the active configuration file in the following order:

1. `--config PATH` argument.
2. `TUI_LAUNCHER_CONFIG` environment variable.
3. `$XDG_CONFIG_HOME/tui-launcher/config.toml`.
4. `$HOME/.config/tui-launcher/config.toml`.

## Direct View Invocation

You can open a specific view directly instead of the configured `default_view`:

```bash
tui-launcher <workflow-id>:<view-name> [ARGUMENTS...]
# Or using an alias:
tui-launcher <alias> [ARGUMENTS...]
```

### Direct Invocation Arguments

Arguments passed after the target view are matched against the target view's declared `[views.<name>.query]` schema:

- **String values**: `--name=value`
- **JSON values**: `--name:=JSON` (e.g. `--count:=10` or `--tags:='["rust", "tui"]'`)
- **Boolean flags**: `--flag` (sets `flag = true`) or `--no-flag` (sets `flag = false`)

```bash
tui-launcher dmenu:main --index=true --prompt="Choose item:"
```

## Contract Inspection (`inspect`)

You can inspect the declared interface and routing contract of any view without running the interactive TUI:

```bash
tui-launcher inspect <workflow-id>:<view-name>
# Or using an alias or flag:
tui-launcher inspect <alias>
tui-launcher --inspect <alias>
```

This prints formatted details including:
- Workflow ID and view name
- Route alias
- Engine type (e.g. `picker`, `capture`, `embedded`)
- Declared query parameters and types
- Configured commands and keybindings

## CLI Symlink Multiplexing (`argv[0]`)

`tui-launcher` supports direct entry point multiplexing based on `argv[0]`:

When `tui-launcher` is invoked via a symlink, hardlink, or copy whose file stem matches a declared view alias or `<workflow-id>:<view-name>`:
1. The matching view is resolved and launched immediately.
2. Any trailing command-line flags or arguments are directly bound to the target view's query schema.

For example, create a symlink to an alias:
```bash
ln -s $(which tui-launcher) ~/.local/bin/dmenu
```
Running `dmenu --prompt="Select:"` executes `tui-launcher` directly into the `dmenu` view with `--prompt="Select:"` bound to the view's query parameters without manual CLI dispatch flags.
