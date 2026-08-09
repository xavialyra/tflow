# tui-launcher

`tui-launcher` is a small dmenu-style TUI workflow launcher. It evaluates runtime-driven picker items and command scripts, keeps named views in a view stack, and exposes view-owned commands for the current selection.

The launcher has two configuration concepts:

- a plugin namespace, such as `apps` or `ssh`;
- a concrete view, such as `apps:main` or `apps:detail`.

Each view selects one built-in engine with `type = "picker"`, `type = "capture"`, or `type = "embedded"` and owns that engine's configuration.

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

`-d` / `--dmenu` reads plain-text candidates from standard input and writes the selected original line to standard output. It uses the picker's full-screen layout by default, so it can be composed with ordinary CLI commands without mixing terminal control sequences into the result stream:

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
- `--lines N` limits the visible result rows; without it, the result area fills the available picker view;
- `--initial TEXT` sets the initial query;
- `--dmenu0` reads and writes NUL-delimited records;
- `--index` prints the selected zero-based input index instead of its text;
- `--with-nth N|FMT` changes the displayed fields, such as `2,11` or `{1} {2}`;
- `--accept-nth N|FMT` changes the text written to standard output;
- `--match-nth N|FMT` changes the fields used for matching;
- `--nth-delimiter CHARACTER` sets the single ASCII field delimiter and defaults to Tab; use `--nth-delimiter=whitespace` to treat runs of whitespace as one field delimiter;
- setting any field format to `0` leaves that part unchanged.

Field ranges use the Fuzzel-style `{N..M}` and `{N..}` forms. A symlink whose basename is `dmenu` also starts the program in dmenu mode. Set `dmenu_view = "core:dmenu"` in the root config to choose the dedicated display view; no dmenu view-selection CLI flag is provided yet. Dmenu mode can use `--config`, but cannot be combined with `--check`. The dedicated view is a picker view with no items or commands:

```toml
# config/config.toml
dmenu_view = "core:dmenu"

# plugins/core/plugin.toml
[views.dmenu]
type = "picker"
display = "text"
```

## Views and plugins

A plugin contains one or more views. The plugin directory name is its unique ID, so every view has a canonical `plugin:view` reference:

```text
core:default
apps:default
apps:detail
```

A view may also define a short `alias` for picker input. Canonical references contain `:` and are always exact; tokens without `:` are resolved only as aliases.

A view's `type` directly selects its engine, and engine-specific fields live on that view:

```toml
[views.search]
type = "picker"
items = '{{ script("scripts/items.sh", runtime:view.current.query) }}'

[views.result]
type = "capture"
output = "{{ runtime:view.current.input }}"
title = "Result"

[views.shell]
type = "embedded"
command = ["sh", "-lc", "{{ runtime:view.current.input }}"]
title = "Shell"
```

The built-in engines are `picker`, `capture`, and `embedded`. Every configured path is a view: picker renders searchable items, capture renders a string result, and embedded hosts a PTY process. Navigation always supplies a view path and an input string; the target engine decides what that input means. For example, `shell:default ls` enters `shell:default` with `ls` as its input.

Expression syntax is validated when configuration is loaded and expressions are evaluated only when the consuming engine asks for a value. `config:path` and `runtime:path` are reference expressions; `path(...)` and `script(...)` are expression methods resolved by the consuming engine. A complete expression preserves its value type, while a mixed expression is a string template. Focus and lifecycle behavior belong to the engine and are not configurable View fields.

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

Every manifest defines its metadata in `[plugin]`. `name` is descriptive metadata and may be shared by multiple installed plugins. `api` defaults to `1` when omitted. Routing aliases belong to individual views:

```toml
[plugin]
api = 1
name = "applications"

[views.default]
type = "picker"
alias = "app"
items = '{{ script("scripts/items.sh", runtime:view.current.query) }}'

[views.default.commands.open]
key = "enter"
label = "Open"
run = { file = "scripts/open.sh" }
```

Unsupported API versions are rejected. `name` must not be empty. An alias cannot be empty or contain `:` or whitespace. Duplicate names and aliases are accepted; a duplicate alias becomes an error only when it is used, at which point the picker displays the canonical conflicting views and stays on the current view. Package directory names cannot contain `:` or whitespace because they form canonical view references. Script paths must remain below the package directory. Inline scripts are supported for small commands. Git source, release version, and lock data are not part of the runtime manifest yet.

The root config file selects the default views and can define shared picker binding defaults:

```toml
default_view = "core:default"

[defaults.picker.bindings]
open_commands = ["ctrl+k"]
```

The `core` plugin can aggregate picker views from several plugin packages:

```toml
[views.default]
type = "picker"
sources = ["sys:default", "apps:default"]
```

This table belongs in `plugins/core/plugin.toml`, not in the root `config.toml`.

A view with `sources` displays the results of those picker views. Aggregate views cannot define commands or items. A source view keeps its own `items` expression and remains the owner of the resulting item commands.

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

A canonical reference or unique alias enters a configured view through the normal view stack. For example, `apps:default terminal` and `app terminal` target the same view when `apps:default` owns `alias = "app"`. A bare plugin ID is ordinary query text and is not expanded to a `default` view. `Esc` returns to the parent view when the active engine assigns it that behavior. Capture and embedded views are normal children in the same view stack. `Ctrl-K` opens the configured command picker view (default: `core:command`) for the selected item's source view; selecting a command replaces that temporary command view with its navigation target.

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

The target owns its behavior. A capture view evaluates `output` relative to its plugin and requires a string result. An embedded view evaluates `command` to a non-empty argv array, so it can host arbitrary PTY processes:

```toml
[views.shell]
type = "embedded"
command = ["sh", "-lc", "{{ runtime:view.current.input }}"]

[views.btop]
type = "embedded"
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

View commands use the same named-key, Ctrl, and Alt binding syntax as picker actions. Plain characters remain search input. Picker actions take priority when a physical key is assigned to both; override or disable that picker action in the View's `bindings` before assigning the key to a View command.

## Picker items

A concrete picker view can define an `items` expression. The expression returns one JSON array, and every item must contain a `label` plus an optional `value` and `metadata`:

```toml
[views.main]
type = "picker"
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

The picker runtime exposes the current request under `runtime:view.current.request`. `runtime:view.current.input` and `query` contain the current view's parameters; `raw_input` retains the complete input-bar value including a route selector. The source script can accept a string, object, or any other JSON value without a picker-defined parameter schema. Script output is parsed as one JSON document and must be an array for an `items` expression. The returned array is authoritative: its order is preserved, and the picker does not sort or filter valid items. Query handling belongs to the expression or script. Later expression methods can provide reusable filtering and sorting when needed.

A root picker view evaluates the `items` expression of each view in `sources`. Each result shows its source view's alias, or its canonical reference when no alias is configured:

```text
app          Terminal
sys          System monitor
apps:detail  Package details
```

Typing `app terminal` enters the view owning alias `app` with `terminal` as its query, while `core:messages timeout` uses an exact canonical reference. An alias is recognized as a route only after a whitespace separator, so typing `app` alone remains ordinary query text; a canonical `plugin:view` reference may still be entered by itself. The input bar belongs to the session chrome: its route selector and query are kept across view changes, while the target engine receives only the query. `Esc` returns from a child view and restores the parent input snapshot; deleting the route selector with Backspace cancels that route and keeps the edited input in the parent view. Source commands remain owned by the source view.

## Command environment

Command scripts receive:

- `LAUNCHER_ITEM`;
- `LAUNCHER_VALUE`;
- `LAUNCHER_METADATA`;
- `LAUNCHER_PLUGIN` (the plugin directory ID);
- `LAUNCHER_PLUGIN_DIR` when the command belongs to a file-backed plugin;
- `LAUNCHER_VIEW`;
- `LAUNCHER_VIEW_REF`;
- `LAUNCHER_COMMAND`;
- `LAUNCHER_QUERY`;
- `LAUNCHER_LOG_FILE`.

Command `shell` selects the command interpreter. When omitted, the source view's `run_shell` is used, then `sh`. File-backed local commands run with the plugin directory as their working directory, so relative paths and `LAUNCHER_PLUGIN_DIR` are stable.

An embedded View starts its argv in the target plugin directory and receives `LAUNCHER_VIEW_REF`, `LAUNCHER_INPUT`, `LAUNCHER_PLUGIN`, and optional `LAUNCHER_PLUGIN_DIR` and `LAUNCHER_LOG_FILE`. Navigation does not implicitly carry source item metadata; use the command's `input` expression to pass the target parameter explicitly.

## Keys

Picker shortcuts are semantic engine bindings. Root defaults apply to every picker View:

```toml
[defaults.picker.bindings]
open_commands = ["ctrl+p"]
clear_input = ["ctrl+u"]
exit = ["ctrl+c", "ctrl+d"]
```

A View can override only the actions it needs; omitted actions inherit the root or built-in defaults, while an empty array disables an action:

```toml
[views.main.bindings]
open_commands = ["ctrl+k"]
delete_word = []
```

Available actions are `exit`, `open_commands`, `back`, `select_previous`, `select_next`, `delete_backward`, `clear_input`, `delete_word`, and `activate`. Bindings accept `enter`, `backspace`, `up`, `down`, `escape`, `ctrl+<letter>`, and `alt+<character>`. `Tab` is reserved for View completion; the arrow, Home/End, and Delete keys edit the input when they are not assigned to a picker action. One physical key cannot be assigned to multiple picker actions.

The default bindings are:

- `Enter`: execute the current view's Enter command;
- `Tab`: open View completion; while it is open, `Tab` cycles candidates;
- `Alt+<character>` or another unreserved configured key: execute a view command;
- `Up` / `Down`: move through results, or move through View completion candidates;
- `Left` / `Right`: move the input cursor;
- `Home` / `End`: move the input cursor to the beginning or end;
- `Esc`: close View completion when it is open; otherwise clear the query, then return to the parent view or quit at the root;
- `Backspace` / `Delete`: delete before or after the cursor;
- `Ctrl-C` / `Ctrl-D`: quit;
- `Ctrl-U`: clear the query;
- `Ctrl-W`: delete the previous word;
- `Ctrl-K`: open the command picker view for the selected item's source view.

View completion searches every configured View by alias, canonical reference, and plugin name. `Enter` accepts the highlighted candidate and navigates to its canonical reference. The completion list uses inverse highlighting for the selected row and does not add a selection marker.

Input, divider, and footer chrome are composed centrally from the active route, the current engine, and global errors. The input line keeps the cursor visible and scrolls long input around it without a prompt marker. The divider shows `plugin:view (alias)` followed by a horizontal rule; the alias is omitted when absent. Engine title and status remain on the left of the footer, while engine commands remain right-aligned. A current error temporarily replaces the complete footer and includes its occurrence time. The latest error replaces the previous one and is cleared after five seconds, a new query, a view change, a successful refresh, or a successful command. Errors and command status records are also appended to the runtime JSONL log at `$XDG_STATE_HOME/tui-launcher/runtime.jsonl` or `$HOME/.local/state/tui-launcher/runtime.jsonl`. `TUI_LAUNCHER_LOG_FILE` overrides the path.

When additional view commands do not fit, `Ctrl-K commands` navigates to the command picker view; `Up` / `Down` select a command, `Enter` runs it, and `Esc` returns to the previous view.

Picker item expressions are debounced globally by 120ms and evaluated by a background worker. Older results are discarded when a newer view/query request exists. Each script has a 10-second timeout, is limited to 64 KiB of JSON input, 1 MiB of stdout, and 64 KiB of stderr. Timed-out or oversized scripts report an error for that source. If `Enter` is pressed while item evaluation is pending, it waits for the matching result before executing the view command.
