# tui-launcher

`tui-launcher` is a small dmenu-style TUI workflow launcher. It evaluates runtime-driven picker items and command scripts, keeps named views in a view stack, and exposes view-owned commands for the current selection.

The launcher has two configuration concepts:

- a plugin namespace, such as `apps` or `ssh`;
- a concrete view, such as `apps:main` or `apps:detail`.

Each view selects one built-in engine in its `[views.<name>.engine]` table. Engine-specific fields live beneath `[views.<name>.engine.config]`; routing, state, and commands remain on the View.

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

The repository includes `mise.toml` for project-local development. It pins Rust 1.97.1 with the default profile, adds the debug and release target directories to `PATH`, and points `TUI_LAUNCHER_CONFIG` at `tests/fixtures/config/config.toml`. This configuration is a development and test fixture; it is not installed and is intentionally separate from any default plugin set distributed with the launcher later:

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
tui-launcher --config ./tests/fixtures/config/config.toml dmenu:main
```

Global options must precede the View. Every argument after the View must use an explicit key declared by that View's fixed `query` table. CLI positional arguments, unknown keys, repeated keys, missing required states, and type mismatches are rejected before the terminal opens.

```toml
[views.main.query]
type = "object"
input_order = ["source", "target", "text"]
source = { type = "string", nullable = true }
target = { type = "string", nullable = true }
text = { type = "string", default = "" }
tags = { type = "array<string>", default = [] }
limit = { type = "integer", default = 10 }
```

Query fields declare their type directly. A field with no `default` and no `nullable = true` is required; nullable fields default to `null`, and fields with a default use that value. `--name=value` is parsed using the declared field type, `--name:=VALUE` supplies typed JSON, `--flag`/`--no-flag` supply booleans, and declared arrays accept one comma-separated token. For example:

```bash
tui-launcher tr --source=en --target=zh --text="hello world" --tags=formal,short --limit:=5
```

The TUI input controller uses `input_order`, so the same query can be edited as `en zh 'hello world'`; it validates the complete line and commits all ordered fields atomically. Query schema metadata such as `type` and `input_order` is not included in `this:query`.

When stdin is not a TTY, the launcher captures it unchanged in a private temporary file and opens `/dev/tty` for interaction. `input:stdin.path`, `input:stdin.length`, and `input:stdin.is_tty` describe that immutable input to every engine and script. A `complete` command may explicitly declare a result handler and a JSON `params` object. After the session ends and the terminal is restored, the launcher evaluates that object against the completion-time View state and runtime snapshot, writes it to the handler's stdin, and uses the handler's raw stdout, stderr, and exit status as the invocation result.

Each stack entry owns an independent committed query instance; push creates defaults, pop restores the parent values, and expression tasks capture the active instance snapshot when submitted. Feeds pickers evaluate each owner view with an ephemeral query scope derived from the page's committed params binding. Scripts receive the typed query only when an expression explicitly passes `this:query`.

## dmenu plugin

The development fixture's `dmenu:main` View is an ordinary picker plus plugin scripts. It is a reference plugin used by integration tests, not a bundled default. Core contains no dmenu CLI branch, parameter schema, record parser, filtering rule, or completion mode. The View declares typed query fields below `views.main.query`; its items script reads `this:query` and `input:stdin.path`, performs source filtering, and returns standard picker items. TTY stdin is an empty candidate source, so direct invocation can accept free text. Its completion command explicitly passes the selected item, typed input, option state, and stdin descriptor to the result script, which maps them back to the original bytes. These scripts require `python3`.

```bash
printf '%s\n' 'Option 1' 'Option 2' 'Option 3' |
  tui-launcher dmenu:main

printf '1\tFirst\n2\tSecond\n' |
  tui-launcher dmenu:main --with-nth=2

ps aux |
  tui-launcher dmenu:main \
    --with-nth=2,11 \
    --nth-delimiter=whitespace

# NUL-delimited records also use NUL-terminated output.
printf 'one\0two\0three\0' |
  tui-launcher dmenu:main --dmenu0

find . -name '*.rs' | tui-launcher dmenu:main --prompt="> "
```

The generic invocation syntax replaces the former core `--dmenu` switch and `argv[0] == dmenu` handling. Invoke `dmenu:main` explicitly (or its `dmenu` View alias), and use `=`/`:=` forms rather than spaced option values. The plugin accepts `--prompt=TEXT`, `--initial=TEXT`, `--dmenu0`, `--index`, `--with-nth=N|FMT`, `--accept-nth=N|FMT`, `--match-nth=N|FMT`, and `--nth-delimiter=CHARACTER`. Use `--nth-delimiter=whitespace` for runs of spaces or tabs. Field ranges use `{N..M}` and `{N..}`; a field format of `0` disables that projection. Rofi metadata following a NUL separator in newline records is exposed as item metadata but is not written with the selected record.

The View itself uses only generic configuration:

```toml
[plugin]
api = 1
name = "dmenu"

[views.main]
alias = "dmenu"
cancel_exit_code = 1

[views.main.engine]
type = "picker"

[views.main.engine.config]
items = '''{{ script("scripts/items.sh", {
  input = input:$,
  query = this:query
}, 67108864) }}'''
prompt = '''{{ this:query.prompt }}'''
show_prefix = false

[views.main.engine.config.bindings]

[views.main.query]
type = "object"
input_order = ["initial"]
prompt = { type = "string", nullable = true }
initial = { type = "string", default = "" }
dmenu0 = { type = "boolean", default = false }
index = { type = "boolean", default = false }
with-nth = { type = "string", nullable = true }
accept-nth = { type = "string", nullable = true }
match-nth = { type = "string", nullable = true }
nth-delimiter = { type = "string", nullable = true }

open_commands = []
open_completion = []
back = []
exit = ["escape", "ctrl+c", "ctrl+d"]

[views.main.commands.accept]
key = "enter"
label = "Accept"
type = "complete"

[views.main.commands.accept.payload]
handler = "scripts/result.sh"

[views.main.commands.accept.payload.params]
options = "{{ this:query }}"
stdin = "{{ input:stdin }}"
selected = "{{ runtime:view.current.selected_item }}"
typed = "{{ runtime:view.current.input }}"
```

## Views and plugins

A plugin contains one or more views. The plugin directory name is its unique ID, so every view has a canonical `plugin:view` reference:

```text
core:default
apps:main
apps:detail
```

A view may also define a short `alias` for picker input. Canonical references contain `:` and are always exact; tokens without `:` are resolved only as aliases.

A view's `engine.type` selects its engine, and its `engine.config` table is validated by that engine:

```toml
[views.search.engine]
type = "picker"

[views.search.engine.config]
items = '{{ script("scripts/items.sh", this:query) }}'

[views.result.engine]
type = "capture"

[views.result.engine.config]
output = "{{ runtime:view.current.input }}"
title = "Result"

[views.shell.engine]
type = "embedded"

[views.shell.engine.config]
command = ["sh", "-lc", "{{ runtime:view.current.input }}"]
title = "Shell"
```

The built-in engines are `picker`, `capture`, and `embedded`. Every configured path is a view: picker renders searchable items, capture renders a string result, and embedded hosts a PTY process. Picker image preview paths may be absolute, use `~` or `~/` for the current user's home directory, or be relative to the selected item's source plugin directory. Image previews use Kitty, Sixel, or iTerm2 when the active terminal reports support and otherwise render a Unicode half-block fallback. Navigation supplies a view path and may provide an input string; when it does, the target engine decides what that input means. For example, `shell:main ls` enters `shell:main` with `ls` as its input.

A picker can reserve a `preview` pane beside its item list. The layout is a two-pane horizontal or vertical split with one `items` slot and one `preview` slot. Preview blocks read JSON Pointer values from the selected item, including its `metadata`:

```toml
[views.files.engine]
type = "picker"

[views.files.engine.config]
items = '{{ script("scripts/items.sh") }}'

[views.files.engine.config.layout]
direction = "horizontal"
gap = 1

[[views.files.engine.config.layout.panes]]
slot = "items"
grow = 1
min = 28

[[views.files.engine.config.layout.panes]]
slot = "preview"
size = 36
min = 24

[views.files.engine.config.preview]

[[views.files.engine.config.preview.blocks]]
type = "image"
source = "/metadata/thumbnail"
grow = 1

[[views.files.engine.config.preview.blocks]]
type = "separator"

[[views.files.engine.config.preview.blocks]]
type = "text"
source = "/metadata/summary"
size = 5
```

Supported preview block types are `image`, `text`, and `separator`. A separator renders a horizontal line, needs no `source`, and occupies one row by default; use `size` to reserve more rows. An image path is resolved relative to the selected item's source plugin; image decoding runs in a cancellable background task. When the terminal is too small to satisfy both pane minimums, the preview hides and the picker list uses the full content area.

Configuration fields accept native TOML values directly, including arrays and tables. Use `{{ ... }}` only when a value must be computed from a reference or method call; consumers receive the evaluated JSON value without distinguishing static configuration from expressions. For example, a static picker can declare `items = [{ label = "Open shell" }]`, while a dynamic picker can declare `items = '{{ script("scripts/items.sh") }}'`.

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

[views.main]
alias = "app"

[views.main.engine]
type = "picker"

[views.main.engine.config]
items = '{{ script("scripts/items.sh", this:query) }}'

[views.main.commands.open]
key = "enter"
label = "Open"
type = "run"

[views.main.commands.open.payload]
handler = { file = "scripts/open.sh" }
```

Unsupported API versions are rejected. `name` must not be empty. An alias cannot be empty or contain `:` or whitespace. Duplicate names and aliases are accepted; a duplicate alias becomes an error only when it is used, at which point the picker displays the canonical conflicting views and stays on the current view. Package directory names cannot contain `:` or whitespace because they form canonical view references. Script paths must remain below the package directory. Inline scripts are supported for small commands. Git source, release version, and lock data are not part of the runtime manifest yet.

The root config file selects the default views and can define shared picker binding defaults:

```toml
default_view = "core:default"

[defaults.picker.bindings]
open_commands = ["ctrl+k"]
```

The `core` plugin can fan in picker views from several plugin packages as feeds:

```toml
[views.default.engine]
type = "picker"

[[views.default.engine.config.feeds]]
view = "sys:main"

[[views.default.engine.config.feeds]]
view = "apps:main"
```

This table belongs in `plugins/core/plugin.toml`, not in the root `config.toml`.

A feeds picker merges the `items` of those owner views. It cannot define its own `items`. A feed is a stateless item provider: every refresh parses the page's committed params string through each owner's query schema, evaluates that owner once, and keeps one response-level state/binding snapshot for all items returned by that feed. Empty committed input stays empty as `this:raw_input` while the owner's typed `this:query` retains schema defaults. A non-empty binding requires at least one field in an object owner's `input_order`; otherwise that owner reports an error without evaluating its item provider. The page query may use any supported schema; the product contract is that its committed params string is independently parsed by every feed owner. Feed state is never installed as a persistent View instance, and its `state_revision` is only evaluation metadata for that refresh, not a persistent revision across refreshes. Any picker may declare `feeds`, not only the home view.

Commands can open another concrete view. The command belongs in the owning plugin manifest:

```toml
[views.main.commands.apps]
key = "alt+a"
label = "Apps"
type = "navigate"

[views.main.commands.apps.payload]
target = "apps:main"
```

The runtime keeps a view stack. Opening `apps:main` from `core:default` produces:

```text
[core:default, apps:main]
```

Inside the session input bar, a canonical reference or unique alias enters a configured View through the normal view stack. For example, typing `apps:main terminal` and `app terminal` targets the same View when `apps:main` owns `alias = "app"`. CLI invocation remains keyed and does not use these positional route strings. A bare plugin ID is ordinary query text and is not expanded to a `default` view. `Esc` returns to the parent view when the active engine assigns it that behavior. Capture and embedded views are normal children in the same view stack. `Ctrl-K` opens the configured command picker view (default: `core:command`) with the union of the selected feed owner's commands and the parent page commands; owner commands win key conflicts. With no selected item, page commands remain available. The temporary picker retains typed page/owner snapshots and the original parent item, and navigation replaces that picker with its target.

## Expressions

Expressions use `{{ ... }}` and are evaluated by the engine that consumes them:

```toml
items = "{{ runtime:view.current.items }}"
commands = "{{ runtime:view.current.command }}"
label = "query: {{ runtime:view.current.query }}"
```

References use four namespaces: `config:` for static merged configuration, `this:` for the View instance that owns the expression, `runtime:` for mutable session/engine metadata, and `input:` for the immutable stdin descriptor. `$` or an empty path refers to a complete namespace root. `this:query` is the current View instance's committed query value: an editable string when no query schema is declared (or when `query.type = "string"`), and a typed object for `query.type = "object"`; `config:` never receives a query overlay.

```toml
items = '{{ path(runtime:view.current, "$.items") }}'
request = '{{ script("scripts/query.sh", {query = this:query, input = input:$}) }}'
selected = '{{ path(script("scripts/query.sh"), "$.items") }}'
handler = "{{ config:commands.script }} --query {{ runtime:view.current.query }}"
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
selected = "{{ runtime:view.current.selected_item }}"
stdin = "{{ input:stdin }}"
```

The handler receives exactly that JSON object on stdin. Its raw stdout and stderr are forwarded, and its exit code becomes the launcher exit code. Handler stdout is limited to 16 MiB and stderr to 64 KiB, with a 10-second timeout. A View may set `cancel_exit_code` to control the exit code when its root invocation is cancelled.

Navigation reads one automatically evaluated request object. `target` may be a literal or an expression. `query` may be a string, which is parsed by the target View's query schema, or an object, which is validated directly and completed with defaults. Omitting `query`, or evaluating it to `null`, keeps the target View's query defaults, while an explicit empty string clears its editable query:

```toml
[views.main.commands.inspect]
key = "alt+i"
label = "Inspect"
type = "navigate"

[views.main.commands.inspect.payload]
target = "{{ runtime:view.current.selected_item.metadata.target }}"
query = "{{ runtime:view.current.selected_item.value }}"
```

The target owns its behavior. A capture view evaluates `output` relative to its plugin and requires a string result. An embedded view evaluates `command` to a non-empty argv array, so it can host arbitrary PTY processes:

```toml
[views.shell.engine]
type = "embedded"

[views.shell.engine.config]
command = ["sh", "-lc", "{{ runtime:view.current.input }}"]

[views.btop.engine]
type = "embedded"

[views.btop.engine.config]
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

A concrete picker view can define an `items` expression in its engine config. The expression returns one JSON array, and every item must contain a `label` plus an optional `value` and `metadata`:

```toml
[views.main.engine]
type = "picker"

[views.main.engine.config]
items = '{{ script("scripts/items.sh", this:query) }}'

[views.main.query]
type = "object"
input_order = ["text"]
text = { type = "string", default = "" }
```

```json
[
  {"label":"Termius","value":"termius.desktop","metadata":{"kind":"app"}}
]
```

`script(target, input, max_output_bytes)` receives the optional input as JSON on stdin. The expression chooses the input value; the script owns its input shape, and the optional output limit uses bytes. For a structured request:

```toml
items = '{{ script("scripts/items.sh", this:$) }}'
```

The picker runtime exposes stack-top UI metadata under `runtime:view.current`, including `input`, `query`, `raw_input`, `selected_item`, and `items`. Session input is also published under `runtime:session.input`. Public picker items contain `prefix`, `text`, `value`, `metadata`, and one provenance field, `owner_view`; internal feed IDs, state, binding, and query snapshots are never exposed. `this` is the expression-owning definition context (`ref`, `query`, `input`, `raw_input`, `state_revision`) and does not include selection. Feed owner scripts typically take `this:query` or `this:$`. Selection belongs in complete/navigate params as `runtime:view.current.selected_item`. Script output is parsed as one JSON document and must be an array for an `items` expression. The returned array is authoritative: its order is preserved, and the picker does not sort or filter valid items. Search, filtering, and sorting belong to the expression or plugin script.

A feeds picker evaluates the `items` expression of each owner in `engine.config.feeds`. Each result shows its owner view's alias, or its canonical reference when no alias is configured, in a right-aligned trailing column:

```text
Terminal       app
System monitor sys
Package details apps:detail
```

Typing `app terminal` enters the view owning alias `app` with `terminal` as its query, while `core:messages timeout` uses an exact canonical reference. Both aliases and canonical `plugin:view` references become routes only after a whitespace separator, so `app` and `core:messages` alone remain ordinary query text. The input bar belongs to the session chrome: a recognized route selector is transient routing state, while the target engine receives only the query. `Esc` from a routed child, or Backspace at the end of its selector tag, returns to the parent with an empty input buffer so a stale query cannot filter the parent view. Feed owner commands remain owned by the owner view.

## Command environment

Command scripts receive:

- `LAUNCHER_ITEM`, `LAUNCHER_VALUE`, and `LAUNCHER_METADATA` from the selected item;
- `LAUNCHER_ITEM_VIEW_REF` and `LAUNCHER_ITEM_PLUGIN` from the selected item's feed owner (empty when no item is selected);
- `LAUNCHER_PLUGIN`, `LAUNCHER_PLUGIN_DIR`, and `LAUNCHER_VIEW_REF` from the command owner;
- `LAUNCHER_VIEW` and `LAUNCHER_QUERY` from the parent page and its committed binding;
- `LAUNCHER_COMMAND` and `LAUNCHER_LOG_FILE`.

Command `shell` selects the command interpreter. When omitted, the command owner's `run_shell` is used, then `sh`. File-backed commands run in the command owner's plugin directory. Direct shortcuts and Ctrl-K use the same snapshots: owner commands evaluate against the selected feed context, while page commands evaluate against page state. Navigate and complete payloads see the same committed binding through `this:raw_input`; Ctrl-K complete returns the original parent item, not the command-list row.

An embedded View starts its argv in the target plugin directory and receives `LAUNCHER_VIEW_REF`, `LAUNCHER_INPUT`, `LAUNCHER_PLUGIN`, and optional `LAUNCHER_PLUGIN_DIR` and `LAUNCHER_LOG_FILE`. Navigation does not implicitly carry source item metadata; use the command's `input` expression to pass the target parameter explicitly.

Picker engine configs accept `bindings`, `prompt`, and `show_prefix`. `prompt` changes the input prefix. Owner prefixes are hidden by default; feeds pickers may set `engine.config.show_prefix = true` to identify each owner. Input defaults and types belong to the View's `query` schema; acceptance belongs to an ordinary `complete` command. Picker has no activation mode, local filter, initial-input field, or item search field.

## Keys

Picker shortcuts are semantic engine bindings. Root defaults apply to every picker View:

```toml
[defaults.picker.bindings]
open_commands = ["ctrl+k"]
toggle_preview = ["ctrl+p"]
clear_input = ["ctrl+u"]
exit = ["ctrl+c", "ctrl+d"]
```

A View can override only the actions it needs; omitted actions inherit the root or built-in defaults, while an empty array disables an action:

```toml
[views.main.engine.config.bindings]
open_commands = ["ctrl+k"]
delete_word = []
```

Available actions are `exit`, `open_commands`, `open_completion`, `back`, `select_previous`, `select_next`, `delete_backward`, `clear_input`, `delete_word`, `activate`, and `toggle_preview`. `toggle_preview` only affects picker Views that configure a preview; other picker Views ignore it. Bindings accept `enter`, `tab`, `backtab`, `backspace`, `up`, `down`, `escape`, `ctrl+<letter>`, and `alt+<character>`. The arrow, Home/End, and Delete keys edit the input when they are not assigned to a picker action. One physical key cannot be assigned to multiple picker actions.

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
- `Ctrl-K`: open the owner-priority union of selected feed owner and parent page commands.
- `Ctrl-P`: show or hide the preview in a picker View that configures one.

View completion searches every configured View by alias, canonical reference, and plugin name. `Enter` accepts the highlighted candidate and navigates to its canonical reference. The completion list uses the same fixed marker column, `▌` selection marker, bold selected text, and scrollbar gutter as picker items.

Top status, input, divider, content, and footer chrome are composed and rendered centrally from the active route, the current engine, and global errors. The top status line is reserved as blank space. The input line keeps the cursor visible and scrolls long input around it without a prompt marker; non-focus views display their canonical view prefix and muted query text. The divider is a plain horizontal rule. The footer follows the content directly and uses a Rose Pine Dawn surface background inside the viewport padding. Engine title and status remain on the left of the footer, while engine command keys use a background highlight and their descriptions remain plain text. A current error temporarily replaces the complete footer and includes its occurrence time. The latest error replaces the previous one and is cleared after five seconds, a new query, a view change, a successful refresh, or a successful command. Errors and command status records are also appended to the runtime JSONL log at `$XDG_STATE_HOME/tui-launcher/runtime.jsonl` or `$HOME/.local/state/tui-launcher/runtime.jsonl`. `TUI_LAUNCHER_LOG_FILE` overrides the path.

When additional commands do not fit, `Ctrl-K` navigates to the command picker view; `Up` / `Down` select a command, `Enter` runs it, and `Esc` returns to the previous view. Commands with the same normalized key are displayed and executed using the same owner-first, page-fallback rule.

Launcher input is committed to View state and runtime immediately; picker item refreshes are then scheduled by the shared input controller with a 120ms debounce and evaluated by a background worker. A request generation plus complete view/raw binding/state identity prevents results from an older snapshot from being accepted, even when the visible input text is unchanged. Each script has a 10-second timeout, is limited to 64 KiB of JSON input, 1 MiB of stdout, and 64 KiB of stderr. Timed-out or oversized scripts report an error for that source. If `Enter` is pressed while item evaluation is pending, it waits for the matching result before executing the view command.
