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
```

## Global Options

Global options must precede the target view selector.

| Option | Environment Variable | Description |
| :--- | :--- | :--- |
| `-c, --config <PATH>` | `TUI_LAUNCHER_CONFIG` | Path to the root `config.toml`. Overrides default search paths. |
| `--check` | — | Validates the configuration files and plugin manifests without launching the interactive TUI. Returns non-zero on error. |
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
tui-launcher <plugin-id>:<view-name> [ARGUMENTS...]
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
