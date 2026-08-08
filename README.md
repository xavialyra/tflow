# tui-launcher

`tui-launcher` is a small dmenu-style TUI workflow launcher. It evaluates runtime-driven launcher items and command scripts, keeps named views in a view stack, and exposes view-owned commands for the current selection.

The launcher separates three concepts:

- a plugin namespace, such as `apps` or `ssh`;
- a concrete view reference, such as `apps:main` or `apps:detail`;
- a view type, such as `launcher`, `capture`, or `embedded`.

Several concrete views can use the same view type. For example, `default` and `app` can both be launcher views while keeping separate item expressions and commands.

## Run

The launcher reads its configuration from the XDG configuration directory:

```text
$XDG_CONFIG_HOME/tui-launcher/
├── config.toml
└── plugins/
    └── apps/
        ├── plugin.toml
        └── scripts/
```

When `XDG_CONFIG_HOME` is unset, `$HOME/.config/tui-launcher/` is used. The loader reads `config.toml` and only its sibling `plugins/` directory. Plugin configuration is never embedded in the executable, and no project-local or system directory is scanned unless `TUI_LAUNCHER_CONFIG` or `--config` explicitly points there.

The configuration path is selected in this order:

1. `--config PATH`;
2. `TUI_LAUNCHER_CONFIG`;
3. `$XDG_CONFIG_HOME/tui-launcher/config.toml`;
4. `$HOME/.config/tui-launcher/config.toml`.

The repository includes `mise.toml` for project-local development. It pins Rust 1.97.1 with the default profile, adds the debug and release target directories to `PATH`, and points `TUI_LAUNCHER_CONFIG` at the checked-in project configuration:

```bash
mise install
mise exec -- cargo run
```

Outside the mise environment, the launcher uses the XDG path. Use `--config PATH` only when deliberately selecting another complete configuration root. The path's sibling `plugins/` directory is used for plugin packages.

```toml
disabled_plugins = ["trans"]
```

Validate the active configuration without opening the TUI:

```bash
mise exec -- cargo run -- --check
```

## dmenu mode

`-d` / `--dmenu` reads plain-text candidates from standard input and writes the selected original line to standard output. It uses the launcher's full-screen layout by default, so it can be composed with ordinary CLI commands without mixing terminal control sequences into the result stream:

```bash
printf '%s\n' 'Option 1' 'Option 2' 'Option 3' \
  | tui-launcher --dmenu

# Fuzzel-style field selection uses a literal delimiter. Normalize ps output
# first because ps separates columns with runs of variable-width spaces.
ps aux |
  awk '{$1 = $1; print}' |
  tui-launcher --dmenu --with-nth=2,11 --nth-delimiter=' '

printf '1\tFirst\n2\tSecond\n' |
  tui-launcher --dmenu --with-nth=2

# Treat runs of spaces and tabs as one field delimiter.
ps aux |
  tui-launcher --dmenu --with-nth=2,11 --nth-delimiter=whitespace

# NUL-delimited records also use NUL-terminated output.
printf 'one\0two\0three\0' |
  tui-launcher --dmenu0

find . -name '*.rs' | tui-launcher --dmenu
```

The dmenu UI reads keyboard input and draws through `/dev/tty`. It loads the configured dedicated dmenu view (default: `core:dmenu`) for display settings, but does not run that view's items, sources, or commands. Standard input is consumed as a newline-delimited snapshot before the selector opens; `--dmenu0` uses NUL-delimited records and NUL-terminated output. On acceptance, standard output contains the complete selected input line followed by a newline. `Esc`, `Ctrl-C`, and `Ctrl-D` cancel with a non-zero exit status. If the query does not match an entry, the query text itself is returned. Rofi metadata after a NUL separator is parsed into generic candidate metadata; the current text renderer ignores it.

The available dmenu options are:

- `--prompt TEXT` changes the query prompt;
- `--lines N` limits the visible result rows; without it, the result area fills the available launcher view;
- `--initial TEXT` sets the initial query;
- `--dmenu0` reads and writes NUL-delimited records;
- `--index` prints the selected zero-based input index instead of its text;
- `--with-nth N|FMT` changes the displayed fields, such as `2,11` or `{1} {2}`;
- `--accept-nth N|FMT` changes the text written to standard output;
- `--match-nth N|FMT` changes the fields used for matching;
- `--nth-delimiter CHARACTER` sets the single ASCII field delimiter and defaults to Tab; use `--nth-delimiter=whitespace` to treat runs of whitespace as one field delimiter;
- setting any field format to `0` leaves that part unchanged.

Field ranges use the Fuzzel-style `{N..M}` and `{N..}` forms. A symlink whose basename is `dmenu` also starts the program in dmenu mode. Set `dmenu_view = "core:dmenu"` in the root config to choose the dedicated display view; no dmenu view-selection CLI flag is provided yet. Dmenu mode can use `--config`, but cannot be combined with `--check`. The dedicated view is a launcher view with no items or commands:

```toml
# config/config.toml
dmenu_view = "core:dmenu"

# plugins/core/plugin.toml
[views.dmenu]
type = "launcher"
display = "text"
```

## Views and plugins

A plugin is a namespace containing one or more views. View references use the fully qualified `plugin:view` form:

```text
core:default
apps:main
apps:detail
```

A view's `type` references a viewtype profile in the root configuration. A profile selects a core engine and passes engine-specific expressions through its configuration:

```toml
[viewtypes.launcher.engine]
type = "launcher"

[viewtypes.launcher.engine.config]
items = "{{ runtime:view.current.items }}"
commands = "{{ runtime:view.current.command }}"

[viewtypes.capture.engine]
type = "capture"

[viewtypes.capture.engine.config]
output = "{{ runtime:view.current.input }}"
title = "Result"

[viewtypes.embedded.engine]
type = "embedded"

[viewtypes.embedded.engine.config]
command = ["sh", "-lc", "{{ runtime:view.current.input }}"]
title = "Shell"
```

The built-in engines are `launcher`, `capture`, and `embedded`. Every configured path is a view: launcher renders searchable items, capture renders a string result, and embedded hosts a PTY process. Navigation always supplies a view path and an input string; the target engine decides what that input means. For example, `shell:default ls` enters `shell:default` with `ls` as its input.

Expression syntax is validated when configuration is loaded and expressions are evaluated only when the consuming engine asks for a value. `config:path` and `runtime:path` are reference expressions; `path(...)` and `script(...)` are expression methods resolved by the consuming engine. A complete expression preserves its value type, while a mixed expression is a string template. Focus and lifecycle behavior belong to the engine and are not viewtype data fields.

`exit` is not a view. A local command with `exit = true` returns an exit event after its script finishes.

A plugin is packaged as a directory so its configuration and scripts stay together:

```text
plugins/apps/
├── plugin.toml
├── scripts/
│   ├── items.sh
│   └── open.sh
└── lib/
    └── common.sh
```

The plugin ID is derived from the package directory name. The optional `[plugin]` metadata table currently only carries the launcher protocol version; `api` defaults to `2` when it is omitted. Its view definitions are relative to that directory namespace:

```toml
[views.main]
type = "launcher"
items = '{{ script("scripts/items.sh", runtime:view.current.query) }}'

[views.main.commands.open]
key = "enter"
label = "Open"
run = { file = "scripts/open.sh" }
```

Set `[plugin].api = 2` explicitly when desired. Unsupported API versions are rejected. Plugin directory names must not contain `:` or whitespace because they form the first part of a view reference. Script paths must remain below the plugin directory. Inline scripts are still supported for small commands. Git source, release version, and lock data are not part of the runtime manifest yet; a plugin directory can still be maintained as a Git checkout.

The root config file contains the default view and viewtype profiles:

```toml
default_view = "core:default"

[viewtypes.launcher.engine]
type = "launcher"
```

The `core` plugin can aggregate launcher views from several plugin packages:

```toml
[views.default]
type = "launcher"
sources = ["sys:default", "apps:default"]
```

This table belongs in `plugins/core/plugin.toml`, not in the root `config.toml`.

A view with `sources` displays the results of those launcher views. Aggregate views cannot define commands or items. A source view keeps its own `items` expression and remains the owner of the resulting item commands. The viewtype profile controls which runtime expressions the engine evaluates; the source view still owns the underlying item and command data.

Commands can open another concrete view. The command belongs in the owning plugin manifest:

```toml
[views.main.commands.apps]
key = "alt+a"
label = "Apps"
view = "apps:default"
```

The runtime keeps a view stack. Opening `apps:main` from `core:default` produces:

```text
[core:default, apps:main]
```

A qualified path followed by optional input enters any configured view directly, while a unique `display_prefix` remains a shorter route. `Esc` returns to the parent view when the active engine assigns it that behavior. Capture and embedded views are normal children in the same view stack. `Ctrl-K` opens the configured command launcher view (default: `core:command`) for the selected item's source view; selecting a command replaces that temporary command view with its navigation target.

## Expressions

Expressions use `{{ ... }}` and are evaluated by the engine that consumes them:

```toml
items = "{{ runtime:view.current.items }}"
commands = "{{ runtime:view.current.command }}"
label = "query: {{ runtime:view.current.query }}"
```

A reference uses `namespace:path`, such as `config:commands.script` or `runtime:view.current.query`. Method calls can receive references and other method results as arguments:

```toml
items = '{{ path(runtime:view.current, "$.items") }}'
values = '{{ script("scripts/query.sh", runtime:view.current.query) }}'
selected = '{{ path(script("scripts/query.sh"), "$.items") }}'
run = "{{ config:commands.script }} --query {{ runtime:view.current.query }}"
```

The built-in expression methods are `path` and `script`. `path(value, jsonpath)` applies a JSONPath expression to any JSON value; no match returns `null`, one match keeps its value type, and multiple matches return an array. `script(target, input)` runs a plugin-relative shell script, writes the optional JSON input to stdin, and parses the output as JSON. The script owns the input shape; the expression only chooses which JSON value to pass. Script input/output are bounded and execution has a timeout. A complete placeholder keeps the returned JSON type. A mixed template must produce a string; arrays and objects cannot be implicitly interpolated into it. Methods are invoked only when the engine requests evaluation, so dynamic results can depend on the current runtime state and engine lifecycle.

## View commands

Commands belong to a concrete view. `Enter` is not a special data-source action; it is an ordinary command binding:

```toml
[views.main.commands.open]
key = "enter"
label = "Open"
run = '''
gio launch "$LAUNCHER_VALUE"
'''
```

A command is either a local `run` action or navigation to another view. Navigation can evaluate an input string from the current runtime:

```toml
[views.main.commands.inspect]
key = "alt+i"
label = "Inspect"
view = "inspect:default"
input = "{{ runtime:view.current.selected_item.value }}"
```

The target owns its behavior. A capture profile evaluates `output` relative to the target plugin and requires a string result. An embedded profile evaluates `command` to a non-empty argv array, so it can host arbitrary PTY views:

```toml
[viewtypes.shell.engine]
type = "embedded"

[viewtypes.shell.engine.config]
command = ["sh", "-lc", "{{ runtime:view.current.input }}"]

[viewtypes.btop.engine]
type = "embedded"

[viewtypes.btop.engine.config]
command = ["btop"]
title = "System monitor"
```

Commands cannot combine `run` and `view`. A local command with `exit = true` transfers the terminal to its script and exits the launcher after it finishes:

```toml
[views.main.commands.connect]
key = "enter"
label = "Connect"
exit = true
run = '''
exec ssh "$LAUNCHER_VALUE"
'''
```

The footer is assembled from the current view commands and, when an item is selected, the commands of the item's source view. Item JSON does not contain command definitions.

View commands use the same named-key, Ctrl, and Alt binding syntax as launcher actions. Plain characters remain search input. Launcher actions take priority when a physical key is assigned to both; override or disable that launcher action in the ViewType keymap before assigning the key to a View command.

## Launcher items

A concrete launcher view can define an `items` expression. The expression returns one JSON array, and every item must contain a `label` plus an optional `value` and `metadata`:

```toml
[views.main]
type = "launcher"
items = '{{ script("scripts/items.sh", runtime:view.current.query) }}'
```

```json
[
  {"label":"Termius","value":"termius.desktop","metadata":{"kind":"app"}}
]
```

`script(target, input)` receives the optional input as JSON on stdin. The expression chooses the input value; the script owns its input shape. For a structured request:

```toml
items = '{{ script("scripts/items.sh", runtime:view.current.request) }}'
```

The launcher runtime exposes the current request under `runtime:view.current.request`. The source script can accept a string, object, or any other JSON value without a launcher-defined parameter schema. Script output is parsed as one JSON document and must be an array for an `items` expression. The returned array is authoritative: its order is preserved, and the launcher does not sort or filter valid items. Query handling belongs to the expression or script. Later expression methods can provide reusable filtering and sorting when needed.

A root launcher view evaluates the `items` expression of each view in `sources`. `display_prefix` is shown in the result list and acts as a source selector when followed by a space:

```text
app terminal
ssh prod
```

When a view with an explicit `display_prefix` is not a source of the current view, the same prefix enters that launcher view through the normal view stack. For example, a standalone view with `display_prefix = "log"` is entered with `log timeout`; `timeout` becomes its query. Source selection stays in the current frame, while view routing pushes a new frame.

If `display_prefix` is omitted, the view name is used for source display and selection. The engine adds each source view and display prefix to the returned item after evaluating the expression. Source commands remain owned by the source view.

The core plugin provides a normal log launcher without adding it to `core:default`:

```toml
[views.messages]
type = "launcher"
display_prefix = "log"
items = '{{ script("scripts/items.sh", runtime:view.current) }}'
```

## Command environment

Command scripts receive:

- `LAUNCHER_ITEM`;
- `LAUNCHER_VALUE`;
- `LAUNCHER_METADATA`;
- `LAUNCHER_PLUGIN`;
- `LAUNCHER_PLUGIN_DIR` when the command belongs to a file-backed plugin;
- `LAUNCHER_VIEW`;
- `LAUNCHER_VIEW_REF`;
- `LAUNCHER_COMMAND`;
- `LAUNCHER_QUERY`;
- `LAUNCHER_LOG_FILE`.

Command `shell` selects the command interpreter. When omitted, the source view's `run_shell` is used, then `sh`. File-backed local commands run with the plugin directory as their working directory, so relative paths and `LAUNCHER_PLUGIN_DIR` are stable.

An embedded View starts its argv in the target plugin directory and receives `LAUNCHER_VIEW_REF`, `LAUNCHER_INPUT`, `LAUNCHER_PLUGIN`, and optional `LAUNCHER_PLUGIN_DIR` and `LAUNCHER_LOG_FILE`. Navigation does not implicitly carry source item metadata; use the command's `input` expression to pass the target parameter explicitly.

## Keys

Launcher shortcuts are semantic engine bindings. A ViewType can override only the actions it needs; omitted actions retain their defaults, while an empty array disables an action:

```toml
[viewtypes.launcher.engine.config.bindings]
open_commands = ["ctrl+p"]
clear_input = ["ctrl+u"]
exit = ["ctrl+c", "ctrl+d"]
delete_word = []
```

Available actions are `exit`, `open_commands`, `back`, `select_previous`, `select_next`, `delete_backward`, `clear_input`, `delete_word`, and `activate`. Bindings accept `enter`, `backspace`, `up`, `down`, `escape`, `ctrl+<letter>`, and `alt+<character>`. One physical key cannot be assigned to multiple launcher actions.

The default bindings are:

- `Enter`: execute the current view's Enter command;
- `Alt+<character>` or another unreserved configured key: execute a view command;
- `Up` / `Down`: move through results;
- `Esc`: clear the query, then return to the parent view or quit at the root;
- `Backspace`: delete the previous character;
- `Ctrl-C` / `Ctrl-D`: quit;
- `Ctrl-U`: clear the query;
- `Ctrl-W`: delete the previous word;
- `Ctrl-K`: open the command launcher view for the selected item's source view.

The launcher footer occupies one fixed row. It keeps the current view on the left and view commands on the right. A current error temporarily replaces the left side and includes its occurrence time; the latest error replaces the previous one and is cleared after five seconds, a new query, a view change, a successful refresh, or a successful command. Errors and command status records are also appended to the runtime JSONL log at `$XDG_STATE_HOME/tui-launcher/runtime.jsonl` or `$HOME/.local/state/tui-launcher/runtime.jsonl`. `TUI_LAUNCHER_LOG_FILE` overrides the path.

When additional view commands do not fit, `Ctrl-K commands` navigates to the command launcher view; `Up` / `Down` select a command, `Enter` runs it, and `Esc` returns to the previous view.

Launcher item expressions are debounced globally by 120ms and evaluated by a background worker. Older results are discarded when a newer view/query request exists. Each script has a 10-second timeout, is limited to 64 KiB of JSON input, 1 MiB of stdout, and 64 KiB of stderr. Timed-out or oversized scripts report an error for that source. If `Enter` is pressed while item evaluation is pending, it waits for the matching result before executing the view command.
