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

## Direct View invocation

Pass a canonical reference or unique alias to start any configured View directly, regardless of its engine:

```bash
tui-launcher core:messages
tui-launcher core:default
tui-launcher --config ./config/config.toml dmenu:default
```

Global options must precede the View. Every argument after the View must use an explicit key declared by that View's fixed `query` table. CLI positional arguments, unknown keys, repeated keys, missing required states, and type mismatches are rejected before the terminal opens.

```toml
[views.default.query]
type = "object"
input_order = ["source", "target", "text"]
source = '''{{ state("string", null) }}'''
target = '''{{ state("string", null) }}'''
text = '''{{ state("string", "") }}'''
tags = '''{{ state("array<string>", []) }}'''
limit = '''{{ state("integer", 10) }}'''
```

`state(type)` declares a required value; `state(type, null)` declares an optional value; any other static default initializes the state. `--name=value` supplies a string, `--name:=VALUE` supplies typed JSON, `--flag`/`--no-flag` supply booleans, and declared arrays accept one comma-separated token. For example:

```bash
tui-launcher tr --source=en --target=zh --text="hello world" --tags=formal,short --limit:=5
```

The TUI input controller uses `input_order`, so the same state can be edited as `en zh 'hello world'`; it validates the complete line and commits all ordered fields atomically. Plain query metadata such as `type` and `input_order` remains present when the complete config path is passed to a script.

When stdin is not a TTY, the launcher captures it unchanged in a private temporary file and opens `/dev/tty` for interaction. `input:stdin.path`, `input:stdin.length`, and `input:stdin.is_tty` describe that immutable input to every engine and script. A `complete` command may explicitly declare a result handler and a JSON `params` object. After the session ends and the terminal is restored, the launcher evaluates that object against the completion-time View state and runtime snapshot, writes it to the handler's stdin, and uses the handler's raw stdout, stderr, and exit status as the invocation result.

`state()` declarations belong to the View configuration where they occur and are keyed relative to that View. Only states below a View's fixed `query` subtree are assignable from CLI/TUI input. Each stack entry owns an independent View state instance; push creates defaults, pop restores the parent values, and expression tasks capture the active instance snapshot when submitted. Aggregate pickers create independent source View scopes for their item providers. Scripts receive state only when an expression explicitly passes `this:` data.

## dmenu plugin

The bundled `dmenu:default` View is an ordinary picker plus plugin scripts. Core contains no dmenu CLI branch, parameter schema, record parser, filtering rule, or completion mode. The View declares typed states below `views.default.query`; its items script reads the complete materialized query object and `input:stdin.path`, performs source filtering, and returns standard picker items. TTY stdin is an empty candidate source, so direct invocation can accept free text. Its completion command explicitly passes the selected item, typed input, option state, and stdin descriptor to the result script, which maps them back to the original bytes. These scripts require `python3`.

```bash
printf '%s\n' 'Option 1' 'Option 2' 'Option 3' |
  tui-launcher dmenu:default

printf '1\tFirst\n2\tSecond\n' |
  tui-launcher dmenu:default --with-nth=2

ps aux |
  tui-launcher dmenu:default \
    --with-nth=2,11 \
    --nth-delimiter=whitespace

# NUL-delimited records also use NUL-terminated output.
printf 'one\0two\0three\0' |
  tui-launcher dmenu:default --dmenu0

find . -name '*.rs' | tui-launcher dmenu:default --prompt="> "
```

The generic invocation syntax replaces the former core `--dmenu` switch and `argv[0] == dmenu` handling. Invoke `dmenu:default` explicitly (or its `dmenu` View alias), and use `=`/`:=` forms rather than spaced option values. The plugin accepts `--prompt=TEXT`, `--initial=TEXT`, `--dmenu0`, `--index`, `--with-nth=N|FMT`, `--accept-nth=N|FMT`, `--match-nth=N|FMT`, and `--nth-delimiter=CHARACTER`. Use `--nth-delimiter=whitespace` for runs of spaces or tabs. Field ranges use `{N..M}` and `{N..}`; a field format of `0` disables that projection. Rofi metadata following a NUL separator in newline records is exposed as item metadata but is not written with the selected record.

The View itself uses only generic configuration:

```toml
[plugin]
api = 1
name = "dmenu"

[views.default]
type = "picker"
alias = "dmenu"
items = '''{{ script("scripts/items.sh", {
  input = input:$,
  query = this:query
}, 67108864) }}'''
cancel_exit_code = 1
prompt = '''{{ this:query.prompt }}'''
show_prefix = false

[views.default.query]
type = "object"
input_order = ["initial"]
prompt = '''{{ state("string", null) }}'''
initial = '''{{ state("string", "") }}'''
dmenu0 = '''{{ state("boolean", false) }}'''
index = '''{{ state("boolean", false) }}'''
with-nth = '''{{ state("string", null) }}'''
accept-nth = '''{{ state("string", null) }}'''
match-nth = '''{{ state("string", null) }}'''
nth-delimiter = '''{{ state("string", null) }}'''

[views.default.bindings]
open_commands = []
open_completion = []
back = []
exit = ["escape", "ctrl+c", "ctrl+d"]

[views.default.commands.accept]
key = "enter"
label = "Accept"
type = "complete"

[views.default.commands.accept.payload]
handler = "scripts/result.sh"

[views.default.commands.accept.payload.params]
options = "{{ this:query }}"
stdin = "{{ input:stdin }}"
selected = "{{ runtime:view.active.selected_item }}"
typed = "{{ runtime:view.active.input }}"
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
items = '{{ script("scripts/items.sh", runtime:view.active.query) }}'

[views.result]
type = "capture"
output = "{{ runtime:view.active.input }}"
title = "Result"

[views.shell]
type = "embedded"
command = ["sh", "-lc", "{{ runtime:view.active.input }}"]
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
items = '{{ script("scripts/items.sh", runtime:view.active.query) }}'

[views.default.commands.open]
key = "enter"
label = "Open"
type = "run"

[views.default.commands.open.payload]
handler = { file = "scripts/open.sh" }
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
type = "navigate"

[views.main.commands.apps.payload]
target = "apps:default"
```

The runtime keeps a view stack. Opening `apps:main` from `core:default` produces:

```text
[core:default, apps:main]
```

Inside the session input bar, a canonical reference or unique alias enters a configured View through the normal view stack. For example, typing `apps:default terminal` and `app terminal` targets the same View when `apps:default` owns `alias = "app"`. CLI invocation remains keyed and does not use these positional route strings. A bare plugin ID is ordinary query text and is not expanded to a `default` view. `Esc` returns to the parent view when the active engine assigns it that behavior. Capture and embedded views are normal children in the same view stack. `Ctrl-K` opens the configured command picker view (default: `core:command`) for the selected item's source view; selecting a command replaces that temporary command view with its navigation target.

## Expressions

Expressions use `{{ ... }}` and are evaluated by the engine that consumes them:

```toml
items = "{{ runtime:view.active.items }}"
commands = "{{ runtime:view.active.command }}"
label = "query: {{ runtime:view.active.query }}"
```

References use four namespaces: `config:` for static merged configuration, `this:` for the View instance that owns the expression, `runtime:` for mutable session/engine metadata, and `input:` for the immutable stdin descriptor. `$` or an empty path refers to a complete namespace root. `this:query` returns the current View instance's materialized query object while preserving ordinary metadata; `config:` never receives a state overlay.

```toml
items = '{{ path(runtime:view.active, "$.items") }}'
request = '{{ script("scripts/query.sh", {query = this:query, input = input:$}) }}'
selected = '{{ path(script("scripts/query.sh"), "$.items") }}'
handler = "{{ config:commands.script }} --query {{ runtime:view.active.query }}"
```

The built-in expression methods are `path` and `script`. `path(value, jsonpath)` applies a JSONPath expression to any JSON value; no match returns `null`, one match keeps its value type, and multiple matches return an array. `script(target, input, max_output_bytes)` runs a plugin-relative shell script, writes the optional JSON input to stdin, and parses the output as JSON. The script owns the input shape; the expression only chooses which JSON value to pass. Script output defaults to a 1 MiB limit; trusted data-source plugins may use the optional third argument to raise it as high as 64 MiB. Script input/output remain bounded and execution has a timeout. A complete placeholder keeps the returned JSON type. A mixed template must produce a string; arrays and objects cannot be implicitly interpolated into it. Methods are invoked only when the engine requests evaluation, so dynamic results can depend on the current runtime state and engine lifecycle.

## View commands

Commands belong to a concrete view. `Enter` is not a special data-source action; it is an ordinary command binding:

```toml
[views.main.commands.open]
key = "enter"
label = "Open"
type = "run"

[views.main.commands.open.payload]
handler = '''
gio launch "$LAUNCHER_VALUE"
'''
```

A command has one tagged action: `type = "run"`, `type = "navigate"`, or `type = "complete"`. Every action-specific field belongs to its `payload` table. Navigate payload values are evaluated when the action is read. Completion returns the current selected item (or non-empty input when there is no item) to the invocation host; an empty selection and empty input keep the View open. A complete command without a payload writes the selected value, or the typed input, followed by a newline. With a plugin-relative handler, the evaluated `payload.params` object is written to its stdin after the terminal is restored.

```toml
[views.main.commands.accept]
key = "enter"
label = "Accept"
type = "complete"

[views.main.commands.accept.payload]
handler = "scripts/result.sh"

[views.main.commands.accept.payload.params]
query = "{{ this:query }}"
selected = "{{ runtime:view.active.selected_item }}"
stdin = "{{ input:stdin }}"
```

The handler receives exactly that JSON object on stdin. Its raw stdout and stderr are forwarded, and its exit code becomes the launcher exit code. A View may set `cancel_exit_code` to control the exit code when its root invocation is cancelled.

Navigation reads one automatically evaluated request object. `target` may be a literal or an expression; `query` is the target View's initial input:

```toml
[views.main.commands.inspect]
key = "alt+i"
label = "Inspect"
type = "navigate"

[views.main.commands.inspect.payload]
target = "{{ runtime:view.active.selected_item.metadata.target }}"
query = "{{ runtime:view.active.selected_item.value }}"
```

The target owns its behavior. A capture view evaluates `output` relative to its plugin and requires a string result. An embedded view evaluates `command` to a non-empty argv array, so it can host arbitrary PTY processes:

```toml
[views.shell]
type = "embedded"
command = ["sh", "-lc", "{{ runtime:view.active.input }}"]

[views.btop]
type = "embedded"
command = ["btop"]
title = "System monitor"
```

The action type determines valid fields. A run command with `exit = true` transfers the terminal to its handler and exits the launcher after it finishes:

```toml
[views.main.commands.connect]
key = "enter"
label = "Connect"
type = "run"

[views.main.commands.connect.payload]
handler = '''
exec ssh "$LAUNCHER_VALUE"
'''
exit = true
```

The footer is assembled from the current view commands and, when an item is selected, the commands of the item's source view. Item JSON does not contain command definitions.

View commands use the same named-key, Ctrl, and Alt binding syntax as picker actions. Plain characters edit the session input, which may update the ordered query states and trigger a new items evaluation. Picker actions take priority when a physical key is assigned to both; override or disable that picker action in the View's `bindings` before assigning the key to a View command.

## Picker items

A concrete picker view can define an `items` expression. The expression returns one JSON array, and every item must contain a `label` plus an optional `value` and `metadata`:

```toml
[views.main]
type = "picker"
items = '{{ script("scripts/items.sh", this:query) }}'

[views.main.query]
type = "object"
input_order = ["text"]
text = '''{{ state("string", "") }}'''
```

```json
[
  {"label":"Termius","value":"termius.desktop","metadata":{"kind":"app"}}
]
```

`script(target, input, max_output_bytes)` receives the optional input as JSON on stdin. The expression chooses the input value; the script owns its input shape, and the optional output limit uses bytes. For a structured request:

```toml
items = '{{ script("scripts/items.sh", runtime:view.active.request) }}'
```

The picker runtime exposes stack-top metadata under `runtime:view.active`, including the route-aware `input`, `query`, and `raw_input` strings. Session input is also published under `runtime:session.input`. Typed plugin parameters live under `this:query` for the expression-owning View instance instead. A source script can receive that complete object, selected runtime metadata, stdin artifacts, or any explicitly constructed JSON value. Script output is parsed as one JSON document and must be an array for an `items` expression. The returned array is authoritative: its order is preserved, and the picker does not sort or filter valid items. Search, filtering, and sorting belong to the expression or plugin script.

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

Picker views accept `bindings`, `prompt`, and `show_prefix` engine fields. `prompt` changes the input prefix and `show_prefix = false` hides source prefixes. Input defaults and types belong to query `state()` declarations; acceptance belongs to an ordinary `complete` command. Picker has no activation mode, local filter, initial-input field, or item search field.

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

Available actions are `exit`, `open_commands`, `open_completion`, `back`, `select_previous`, `select_next`, `delete_backward`, `clear_input`, `delete_word`, and `activate`. Bindings accept `enter`, `tab`, `backtab`, `backspace`, `up`, `down`, `escape`, `ctrl+<letter>`, and `alt+<character>`. The arrow, Home/End, and Delete keys edit the input when they are not assigned to a picker action. One physical key cannot be assigned to multiple picker actions.

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

Top status, input, divider, content, and footer chrome are composed and rendered centrally from the active route, the current engine, and global errors. The top status line shows the current `user@hostname` on the left and local `HH:MM:SS` time on the right. The input line keeps the cursor visible and scrolls long input around it without a prompt marker. The root view's divider is a plain horizontal rule; nested views show `plugin:view (alias)` followed by a horizontal rule, with the alias omitted when absent. A second horizontal rule separates content from the footer. Engine title and status remain on the left of the footer, while engine command keys use a background highlight and their descriptions remain plain text. A current error temporarily replaces the complete footer and includes its occurrence time. The latest error replaces the previous one and is cleared after five seconds, a new query, a view change, a successful refresh, or a successful command. Errors and command status records are also appended to the runtime JSONL log at `$XDG_STATE_HOME/tui-launcher/runtime.jsonl` or `$HOME/.local/state/tui-launcher/runtime.jsonl`. `TUI_LAUNCHER_LOG_FILE` overrides the path.

When additional view commands do not fit, `Ctrl-K commands` navigates to the command picker view; `Up` / `Down` select a command, `Enter` runs it, and `Esc` returns to the previous view.

Picker item expressions are debounced globally by 120ms and evaluated by a background worker. Older results are discarded when a newer view/query request exists. Each script has a 10-second timeout, is limited to 64 KiB of JSON input, 1 MiB of stdout, and 64 KiB of stderr. Timed-out or oversized scripts report an error for that source. If `Enter` is pressed while item evaluation is pending, it waits for the matching result before executing the view command.
