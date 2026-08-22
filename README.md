# tui-launcher

`tui-launcher` is a terminal workflow host. A configuration is a set of plugin-owned Views. Views use picker, capture, or embedded engines; commands navigate the View stack, run plugin scripts, call temporary Views, or return results.

## Run

The launcher reads:

```text
$XDG_CONFIG_HOME/tui-launcher/
├── config.toml
├── themes/
└── plugins/
    └── <plugin-id>/
        ├── plugin.toml
        └── scripts/
```

Without `XDG_CONFIG_HOME`, it uses `$HOME/.config/tui-launcher/`. The configuration path is selected in this order:

1. `--config PATH`
2. `TUI_LAUNCHER_CONFIG`
3. `$XDG_CONFIG_HOME/tui-launcher/config.toml`
4. `$HOME/.config/tui-launcher/config.toml`

Validate a configuration without opening the TUI:

```bash
tui-launcher --check
tui-launcher --check --config ./config/config.toml
```

A configured default View starts the normal launcher:

```toml
default_view = "core:default"
```

A View can also be selected directly:

```bash
tui-launcher apps:main
tui-launcher --config ./config/config.toml dmenu:main --index=true
```

Global options must precede the View. Direct invocation arguments use the selected View's declared query schema and must use explicit `--name=value`, `--name:=JSON`, or boolean flag forms.

## Plugins And Views

Every plugin has a manifest and a directory-owned runtime root:

```toml
# plugins/apps/plugin.toml
[plugin]
api = 1
name = "Applications"

[views.main]
alias = "app"

[views.main.engine]
type = "picker"

[views.main.engine.config]
items = [
  { label = "Terminal", value = "terminal" },
]
```

A View is referenced as `plugin:view`. An alias is optional and must be unique. Plugin scripts remain below their plugin directory and run with the launcher's user permissions.

The built-in engines are:

- `picker`: searchable item lists with optional feeds, preview panes, and selection commands;
- `capture`: displays or captures text and can copy it through the terminal clipboard protocol;
- `embedded`: owns a child process and presents its PTY output directly.

Engine fields live under `[views.<name>.engine.config]`. Query state belongs under `[views.<name>.query]`:

```toml
[views.main.query]
type = "object"
input_order = ["source", "target"]
source = { type = "string", nullable = true }
target = { type = "string", nullable = true }
```

Configuration values may contain `{{ namespace.path }}` expressions. Complete expressions preserve their JSON type; mixed expressions produce strings. Values are resolved only at the lifecycle stage where their namespaces exist. Dynamic paths do not execute commands or access files.

## Themes

The built-in `terminal` theme uses terminal defaults for ordinary content. A named theme is loaded from the configuration directory:

```toml
theme = "work"
```

```toml
# themes/work.toml
[palette]
brand = "#BC7588"

[scheme]
primary = "palette:brand"

[bindings.picker-selected]
foreground = "scheme:primary"
bold = true
```

Theme bindings style concrete launcher elements. Missing fields use their binding defaults. Theme files cannot define arbitrary runtime behavior.

## Commands And Navigation

Commands belong to a View and use a tagged action:

```toml
[views.main.commands.open]
key = "enter"
label = "Open"
type = "navigate"

[views.main.commands.open.payload]
target = "apps:detail"
query = "{{ selection.value }}"
```

Supported actions are `run`, `navigate`, `call`, `return`, `edit-input`, and `invoke`. `navigate` pushes a View. `replace = true` replaces the current stack entry. `call` creates a return boundary; `return` unwinds the nearest call or completes a direct invocation.

The normal default View owns route input. A canonical reference or alias followed by whitespace routes to a View:

```text
app terminal
apps:detail host-a
```

The target receives only the query text. The default View is committed with empty input before the target is pushed. A bare selector without whitespace is ordinary query text. Direct CLI Views do not enable prefix routing. Chrome displays non-default View prefixes separately from editable query input.

The built-in `commands` session command opens `selectors:commands` when that View is configured. Its action cannot be replaced; only its key can be configured:

```toml
[commands.bindings.commands]
key = "ctrl+k"
```

Session commands are resolved before View commands and engine actions. The footer only presents effective command state; it does not dispatch commands or own terminal input.

## Scripts

Scripts are file-backed plugin sources:

```toml
[views.main.commands.open.payload]
handler = { source = "script", file = "scripts/open.sh" }
args = ["--target={{ selection.value }}"]
```

`run` handlers use the command owner's plugin directory. Arguments are passed as argv, not interpolated into shell source. The default command shell is `/bin/sh`; an explicit `shell` or `run_shell` may select another interpreter.

Picker and capture sources use the same source shape:

```toml
[views.main.engine.config]
items = { source = "script", file = "scripts/items.sh" }
```

Script paths are checked against the owning plugin root. Static paths are checked by `--check`; dynamic paths are checked immediately before execution. Launcher-loaded run-handler source is limited to 1 MiB. Script execution has a 10-second timeout, a 64 KiB argv limit, a 64 KiB stderr limit, and a 1 MiB default stdout limit. Configured picker/capture stdout may be raised to 64 MiB.

## Embedded Views

An embedded View owns one child process and its PTY for its entire lifetime:

```toml
[views.shell.engine]
type = "embedded"

[views.shell.engine.config]
command = ["sh", "-lc", "{{ page.input }}"]
escape-cancels = true
```

Unmatched input is forwarded byte-for-byte to the PTY. A timed-out bare `Esc` cancels by default; set `escape-cancels = false` when the child owns `Esc`. Explicit passthrough commands override engine bindings on the same key; all unmatched bytes continue to the child. Opening selectors or overlays does not stop the child process.

With `result`, stdout is collected through a bounded result pipe and parsed as text or JSON:

```toml
result = { format = "json", required = true, max_bytes = 1048576 }
```

## Picker Input

Picker defaults are configured under `[defaults.picker.bindings]`; View keymap patches live under `[views.<name>.keymap]`:

```toml
[defaults.picker.bindings]
exit = ["ctrl+c", "ctrl+d"]
back = ["escape"]
select_previous = ["up"]
select_next = ["down"]
activate = ["enter"]

[views.main.keymap]
enter = false
"ctrl+p" = "toggle_preview"
```

A false keymap value disables an inherited action. Session bindings override View bindings, and View commands override engine keymap actions on the same key. Picker Back clears nonempty input first; empty non-root Picker input returns to its parent. Passthrough mode uses the same binding precedence before forwarding unmatched bytes.

## Runtime Guarantees

Dynamic configuration evaluation is bounded by depth, path segments, visited nodes, collection size, compiled templates, and one shared 16 MiB output budget per evaluation operation. Cancellation is checked while traversing and constructing values.

The launcher keeps plugin roots confined, rejects special script files, preserves process groups for cleanup, and restores terminal state on normal cancellation paths. Runtime errors and command status records are written to the configured log path or the XDG state directory.
