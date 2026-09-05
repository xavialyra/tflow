---
title: "workflow.toml Specification"
type: "reference"
tags:
  - workflow
  - views
  - engine
  - commands
  - schema
description: "Authoritative reference for workflow manifests, dual-mode layouts, view engines, query schemas, and command actions."
---

# workflow.toml Specification

Workflows define custom views, keybindings, actions, and engines. Workflows reside under `$XDG_CONFIG_HOME/tui-launcher/workflows/` and support two physical layouts:

1. **Single-File Workflows (`workflows/<id>.toml`)**: A standalone file where the workflow ID is derived from the file stem. Single-file workflows must be self-contained and are prohibited from referencing external relative script files.
2. **Directory Workflows (`workflows/<id>/workflow.toml`)**: A directory package containing a `workflow.toml` manifest and optional local scripts/assets. The workflow ID is derived from the directory name. Relative script references in manifests are resolved against the directory root, and `$WORKFLOW_DIR` is injected into child process environments.

## Workflow Header (`[workflow]`)

```toml
[workflow]
api = 1
name = "Applications"
```

- **`api`** (`integer`, required): The manifest API version. Currently `1`.
- **`name`** (`string`, required): Human-readable display name for the workflow.

## Views (`[views.<name>]`)

A workflow defines one or more views, referenced as `<workflow-id>:<name>`.

```toml
[views.main]
alias = "app"
```

- **`alias`** (`string`, optional): A globally unique shorthand identifier for the view. Duplicate aliases across workflows are rejected at validation time.

### Query Schema (`[views.<name>.query]`)

Views declare their input parameter schema:

```toml
[views.main.query]
type = "object"
input_order = ["source", "target"]
source = { type = "string", nullable = true }
target = { type = "string", nullable = true }
```

### Engine Configuration (`[views.<name>.engine]`)

Every view is powered by an engine. The three built-in engines are:

#### 1. `picker`
Searchable list view with optional dynamic feeds and preview panes.

```toml
[views.main.engine]
type = "picker"

[views.main.engine.config]
items = [
  { label = "Display Text", value = "raw_value", description = "Optional detail" }
]
# Or script source in directory workflows:
# items = { source = "script", file = "scripts/feed.sh" }
```

#### 2. `capture`
Displays text or captures text input, with clipboard copy support.

```toml
[views.log.engine]
type = "capture"

[views.log.engine.config]
content = { source = "script", file = "scripts/get_log.sh" }
clipboard = true
```

#### 3. `embedded`
Owns a child process and renders its PTY output directly.

```toml
[views.terminal.engine]
type = "embedded"

[views.terminal.engine.config]
command = ["sh", "-lc", "{{ page.input }}"]
escape-cancels = true
# Optional result capture:
result = { format = "json", required = true, max_bytes = 1048576 }
```

### Commands (`[views.<name>.commands.<cmd_id>]`)

Commands define key-triggered actions attached to the view:

```toml
[views.main.commands.open]
key = "enter"
label = "Open"
type = "run"

[views.main.commands.open.payload]
handler = { source = "script", file = "scripts/open.sh" }
args = ["--target={{ selection.value }}"]
exit = true
```

#### Multi-Line Inline Scripts

Workflows can declare multi-line scripts directly in the manifest using `script = """..."""`:

```toml
[views.main.commands.checkout]
key = "enter"
type = "run"
args = ["{{ selection.value }}"]
script = """
#!/usr/bin/env -S bash -euo pipefail
target="$1"
git checkout "$target"
"""
```

- **Materialization**: Inline scripts are materialized to temporary read-only files under `$XDG_RUNTIME_DIR/tui-launcher/scripts/` (falling back to `$XDG_CACHE_HOME/tui-launcher/scripts/`) with strict user permissions (`0600`).
- **Shebang Resolution**: The host parses the shebang line (`#!`), splitting interpreter arguments (such as `/usr/bin/env -S ...`) and directly invoking the interpreter, granting immunity from `noexec` mounts.
- **CWD Preservation**: The process working directory (`$PWD`) is preserved as the caller's active terminal directory at invocation time.
- **Attribution**: Materialized scripts include human-readable comments indicating their workflow definition origin.

#### Supported Action Types:
- **`run`**: Executes a command or script via interpreter/shell with caller `$PWD` preserved.
- **`navigate`**: Pushes a new view onto the stack (`replace = true` replaces current stack top).
- **`call`**: Calls a view with a return boundary, supporting modal `popup` presentation.
- **`return`**: Closes the current view and returns a value to the caller.
- **`edit-input`**: Programmatically updates the current query/filter input.
- **`invoke`**: Triggers a direct action on host services.

## Styling Slots (`[styles.<slot>]`)

Workflows declare semantic style overrides consumed by the theme engine:

```toml
[styles.branch]
foreground = "palette:accent-blue"
bold = true
```

Themes override workflow styles globally using `[workflows.<workflow-id>.styles.<slot>]`.
