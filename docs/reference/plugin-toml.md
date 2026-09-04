---
title: "plugin.toml Specification"
type: "reference"
tags:
  - plugin
  - views
  - engine
  - commands
  - schema
description: "Authoritative reference for plugin manifests, view engines, query schemas, and command actions."
---

# plugin.toml Specification

Each plugin is placed in a subdirectory under `$XDG_CONFIG_HOME/tui-launcher/plugins/<plugin-id>/` and must contain a valid `plugin.toml` manifest.

## Plugin Section (`[plugin]`)

```toml
[plugin]
api = 1
name = "Applications"
```

- **`api`** (`integer`, required): The manifest API version. Currently `1`.
- **`name`** (`string`, required): Human-readable display name for the plugin.

## Views (`[views.<name>]`)

A plugin can define multiple views referenced externally as `<plugin-id>:<name>`.

```toml
[views.main]
alias = "app"
```

- **`alias`** (`string`, optional): A globally unique shorthand identifier for the view.

### Query Schema (`[views.<name>.query]`)

Views can declare expected parameters for routing and CLI invocation:

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
# Or script source:
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

#### Supported Action Types:
- **`run`**: Executes a command or script via `/bin/sh` or a configured shell.
- **`navigate`**: Pushes a new view onto the stack (`replace = true` replaces the current stack top).
- **`call`**: Calls a view with a return boundary, supporting modal `popup` presentation.
- **`return`**: Closes the current view and returns a value to the caller.
- **`edit-input`**: Programmatically updates the current query/filter input.
- **`invoke`**: Triggers a direct action on host services.
