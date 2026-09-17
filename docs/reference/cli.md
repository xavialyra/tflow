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
tlaunch inspect <VIEW>
tlaunch inspect --all
```

## Global Options

Global options must precede the target view selector.

| Option | Environment Variable | Description |
| :--- | :--- | :--- |
| `-c, --config <PATH>` | `TLAUNCH_CONFIG` | Path to the root `config.toml`. Overrides default search paths. |
| `-w, --workflow <PATH>` | — | Path to a single-file workflow (`.toml`) or a directory workflow package. Runs in isolated mode without requiring a global `config.toml`. |
| `--theme <THEME>` | — | Explicit theme selector (`terminal` or custom name). Overrides configured theme. |
| `--check` | — | Validates the configuration files and workflow manifests without launching the interactive TUI. Returns non-zero on error. |
| `--inspect <VIEW>` | — | Prints view contract details (alias, engine, queries, commands) and exits. |
| `--all` | — | Used with `inspect` to dump contracts for all configured views. |
| `-h, --help` | — | Prints version and usage information. |
| `-V, --version` | — | Prints the version of `tlaunch`. |

## Configuration Search Precedence

The launcher resolves the active configuration file in the following order:

1. `--config PATH` argument.
2. `TLAUNCH_CONFIG` environment variable (can point to a `.toml` file or a configuration directory).
3. `$XDG_CONFIG_HOME/tlaunch/config.toml`.
4. `$HOME/.config/tlaunch/config.toml`.

When `-w, --workflow <PATH>` is passed without an explicit `--config` and no global `config.toml` exists on disk, `tlaunch` automatically falls back to an in-memory zero-configuration base (default terminal theme and keybindings).

## Isolated Workflow Execution (`-w, --workflow`)

The `-w, --workflow <PATH>` option allows executing an isolated workflow directly without installing it into the system or user configuration directory.

### Target Formats

1. **Single-file workflow (`.toml`)**:
   - The file stem becomes the workflow ID (e.g. `dmenu.toml` -> workflow `dmenu`).
   - The workflow root is set to the directory containing the `.toml` file.
   - Scripts referenced by relative paths in the manifest are resolved relative to this directory.
2. **Directory workflow package**:
   - Points to a directory containing a `workflow.toml` (single workflow package) or a collection of workflow subdirectories.
   - Sets `WORKFLOW_DIR` to the package directory.

### Entry Point Resolution in Workflow Mode

When invoked with `-w <PATH>`:
- If a specific `[VIEW]` selector is provided (e.g. `tlaunch -w ./tool.toml search`), that view is resolved directly.
- If no `[VIEW]` selector is provided, `tlaunch` looks for a view declared with `alias = "main"`.
- If no view with `alias = "main"` is declared and no view argument is supplied, `tlaunch` fails gracefully with a list of available views to choose from:
  ```text
  Error: no default view with alias = "main" found in workflow; specify one of: view1, view2
  ```

### Shebang / Executable Script Integration

Single-file workflows can be made directly executable by adding a `tlaunch -w` Shebang at the top of the file:

```toml
#!/usr/bin/env -S tlaunch -w
[workflow]
api = 1
name = "quick-picker"

[views.main]
alias = "main"

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

By default (when run without arguments), `tlaunch` resolves the view declared with `alias = "main"`. If no view declares `alias = "main"`, it falls back to the legacy `default_view` setting in `config.toml`.

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
Running `dmenu --prompt="Select:"` executes `tlaunch` directly into the `dmenu` view with `--prompt="Select:"` bound to the view's query parameters without manual CLI dispatch flags.
