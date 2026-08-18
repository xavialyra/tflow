# tui-launcher

`tui-launcher` is a small TUI workflow host. It evaluates runtime-driven Views and command scripts, maintains navigation and call stacks, and exposes View-owned commands through a common dispatch model.

## Run

The launcher has two configuration concepts:

- a plugin namespace, such as `apps` or `ssh`;
- a concrete view, such as `apps:main` or `apps:detail`.

Each view selects one built-in engine in its `[views.<name>.engine]` table. Engine-specific fields live beneath `[views.<name>.engine.config]`; routing, state, and commands remain on the View.

The launcher reads its configuration from the XDG configuration directory:

```text
$XDG_CONFIG_HOME/tui-launcher/
├── config.toml
├── themes/
│   └── work.toml
└── plugins/
    └── apps/
        ├── plugin.toml
        └── scripts/
```

When `XDG_CONFIG_HOME` is unset, `$HOME/.config/tui-launcher/` is used. The loader reads `config.toml` and only its sibling `plugins/` directory. Named themes are resolved from the sibling `themes/` directory when selected. Plugin configuration is never embedded in the executable, and no project-local or system directory is scanned unless `TUI_LAUNCHER_CONFIG` or `--config` explicitly points there.

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

## Themes

The built-in `terminal` theme is used when no theme is configured. It uses the terminal's default foreground and background for ordinary content, and ANSI cyan with `bold` for primary emphasis, markers, and selected items. It does not query or modify terminal colors with OSC sequences. The embedded PTY keeps the child process's own ANSI and RGB styles.

A root configuration selects a named theme from the sibling `themes/` directory:

```toml
theme = "work"
```

This loads `themes/work.toml`. Themes are resolved in three layers:

1. `palette` defines theme-owned ANSI or RGB colors.
2. `scheme` assigns Material Design 3-inspired semantic roles to user palette colors or fixed ANSI colors.
3. `bindings` assigns semantic roles to concrete launcher UI elements.

The last layer has defaults, so a theme may only define the colors it needs:

```toml
# themes/work.toml
[palette]
brand = "#BC7588"
paper = "#F2E9E1"
ink = "#696969"

[scheme]
primary = "palette:brand"
primary-container = "palette:paper"
on-primary-container = "palette:ink"
# Omitted surface roles use the terminal's default colors.
on-surface-variant = "palette:ink"
outline = "ansi:magenta"

[bindings.picker-selected]
foreground = "scheme:on-primary-container"
background = "scheme:primary-container"
bold = true
italic = true
underline = true
```

The supported scheme roles are `primary`, `on-primary`, `primary-container`, `on-primary-container`, `surface`, `surface-container`, `on-surface`, `on-surface-variant`, `outline`, `error`, and `on-error`. A scheme value must use `palette:NAME` for a theme-owned color or `ansi:COLOR` for a fixed basic ANSI color; a binding value must use `scheme:ROLE`. The `on-*` roles are intended to be used with their matching background role so foreground/background contrast remains understandable.

Bindings are concrete and independently overridable. The built-in bindings cover `text`, `muted-text`, chrome divider/input prefix/footer/error/footer key, picker text/muted/selected/selected-muted/marker/scrollbar, preview text/error/border, and capture text. Route completion reuses the picker text, muted, selected, and marker bindings. Each binding supports `foreground`, `background`, and boolean text effects: `bold`, `italic`, `underline`, and `strikethrough`. A missing binding inherits its built-in semantic defaults; a missing field inherits only that binding's field default. Bindings do not implicitly inherit from one another.

Palette values may be a basic ANSI color or `#RRGGBB`. The palette namespace contains only names declared by the theme; it has no built-in entries. The separate `ansi:` namespace contains `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `gray`, and `white` and cannot be shadowed by palette names. For example, `palette:cyan` and `ansi:cyan` are independent. Omitting a scheme role keeps its built-in default, including the terminal's default foreground or background where appropriate. 256-color indexes, light/bright names, and aliases are not accepted.

The CLI `--theme` option selects a built-in or named theme:

```bash
tui-launcher --theme work apps:main
```

Themes are resolved relative to the configuration directory. Global options must precede the View selector. The previous `[tokens]` format is not supported.

## Direct View invocation

Pass a canonical reference or unique alias to start any configured View directly, regardless of its engine:

```bash
tui-launcher apps:main
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

When stdin is not a TTY, the launcher captures it unchanged in a private temporary file and opens `/dev/tty` for interaction. `input:stdin.path`, `input:stdin.length`, and `input:stdin.is_tty` describe that immutable input to every engine and script. A root `return` command may declare a result handler and a JSON `params` object. After the session ends and the terminal is restored, the launcher evaluates that object against the returning View state and runtime snapshot, writes it to the handler's stdin, and uses the handler's raw stdout, stderr, and exit status as the invocation result.

Each stack entry owns an independent committed query instance; push creates defaults, pop restores the parent values, and expression tasks capture the active instance snapshot when submitted. Feeds pickers evaluate each owner view with an ephemeral query scope derived from the page's committed params binding. Scripts receive the typed query only when an expression explicitly passes `this:query`.

## dmenu plugin

The development fixture's `dmenu:main` View is an ordinary picker plus plugin scripts. It is a reference plugin used by integration tests, not a bundled default. Core contains no dmenu CLI branch, parameter schema, record parser, filtering rule, or result mode. The View declares typed query fields below `views.main.query`; its items script reads `this:query` and `input:stdin.path`, performs source filtering, and returns standard picker items. TTY stdin is an empty candidate source, so direct invocation can accept free text. Its `return` command explicitly passes the selected item, typed input, option state, and stdin descriptor to the result script, which maps them back to the original bytes. These scripts require `python3`.

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

find . -name '*.rs' | tui-launcher dmenu:main
```

The generic invocation syntax replaces the former core `--dmenu` switch and `argv[0] == dmenu` handling. Invoke `dmenu:main` explicitly (or its `dmenu` View alias), and use `=`/`:=` forms rather than spaced option values. The plugin accepts `--initial=TEXT`, `--dmenu0`, `--index`, `--with-nth=N|FMT`, `--accept-nth=N|FMT`, `--match-nth=N|FMT`, and `--nth-delimiter=CHARACTER`. Use `--nth-delimiter=whitespace` for runs of spaces or tabs. Field ranges use `{N..M}` and `{N..}`; a field format of `0` disables that projection. Rofi metadata following a NUL separator in newline records is exposed as item metadata but is not written with the selected record.

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
show_prefix = false

[views.main.query]
type = "object"
input_order = ["initial"]
initial = { type = "string", default = "" }
dmenu0 = { type = "boolean", default = false }
index = { type = "boolean", default = false }
with-nth = { type = "string", nullable = true }
accept-nth = { type = "string", nullable = true }
match-nth = { type = "string", nullable = true }
nth-delimiter = { type = "string", nullable = true }

[views.main.keymap]
"escape" = "exit"

[views.main.commands.accept]
key = "enter"
label = "Accept"
type = "return"

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

The built-in engines are `picker`, `capture`, and `embedded`. Every configured path is a view: picker renders searchable items, capture renders a string result, and embedded hosts a PTY process. Picker image preview paths may be absolute, use `~` or `~/` for the current user's home directory, or be relative to the selected item's source plugin directory. Image previews use the root `image_protocol` setting and otherwise render Unicode half-blocks. Navigation supplies a view path and may provide an input string; when it does, the target engine decides what that input means. For example, `shell:main ls` enters `shell:main` with `ls` as its input.

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

Supported preview block types are `image`, `text`, and `separator`. A separator renders a horizontal line, needs no `source`, and occupies one row by default; use `size` to reserve more rows. An image path is resolved relative to the selected item's source plugin. Image decoding uses a process-wide pool of two workers and keeps at most one pending selection batch; a newer selection replaces the pending batch, and cancellation is checked between image blocks. Image sources must be regular files, are opened nonblocking without following the final symlink, and are limited to 64 MiB of encoded bytes before decoder construction. One image is limited to 8192x8192 dimensions and approximately 64 MiB of decoded allocation, and one preview may contain at most four image blocks. When the terminal is too small to satisfy both pane minimums, the preview hides and the picker list uses the full content area.

Set the root `image_protocol` setting to `kitty`, `sixel`, `iterm2`, or `halfblocks` for the desired image protocol. For example:

```toml
image_protocol = "kitty"
```

It defaults to `halfblocks`; invalid values are configuration errors. The launcher performs no terminal capability query or protocol environment guessing, and never mutates `TERM` or tmux options. For an explicit native protocol, passthrough wrapping follows the caller's `TMUX`, `TERM`, and `TERM_PROGRAM` environment; native protocols in tmux still require correct `TERM` and `allow-passthrough` configuration.

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

Unsupported API versions are rejected. `name` must not be empty. An alias cannot be empty or contain `:` or whitespace. Duplicate plugin names are accepted, but each View alias must be globally unique; duplicate aliases are rejected when the configuration is loaded. Package directory names cannot contain `:` or whitespace because they form canonical view references. Script paths must remain below the package directory. Inline scripts are supported for small commands. Git source, release version, and lock data are not part of the runtime manifest yet.

The root config may select an explicit default View and can define shared engine binding defaults:

```toml
# Optional when every invocation names a View explicitly.
default_view = "custom:home"

[defaults.capture.bindings]
copy = ["enter"]
back = ["escape"]

[defaults.picker.bindings]
exit = ["ctrl+c", "ctrl+d"]
back = ["escape"]
select_previous = ["up"]
select_next = ["down"]
activate = ["enter"]
```

When `default_view` is omitted, the launcher requires a View on the command line, for example `tui-launcher apps:main` or its configured alias. A configured `default_view` is always an ordinary View and may aggregate plugin Views through explicit feeds or a wildcard feed pattern.

A feed entry such as `view = "*:main"` is expanded while configuration is loaded to all configured picker Views whose local name is `main`; `view = "*:default"` follows the same rule for `default`. The supported form is `*:<view-name>`, not a hard-coded `main` name. Non-picker Views are ignored, matching order is deterministic, and no match is valid. This lets a plugin become part of a configured home View by providing a conventional View name without requiring edits to the user's root configuration.

Explicit feeds remain available for configured pickers that need curated owners or ordering independent of the convention. A feeds picker merges the `items` of its configured owner views. It cannot define its own `items`. A feed is a stateless item provider: every refresh parses the page's committed params string through each owner's query schema, evaluates that owner once, and keeps one response-level state/binding snapshot for all items returned by that feed. Empty committed input stays empty as `this:raw_input` while the owner's typed `this:query` retains schema defaults. A non-empty binding requires at least one field in an object owner's `input_order`; otherwise that owner reports an error without evaluating its item provider. The page query may use any supported schema; the product contract is that its committed params string is independently parsed by every feed owner. Feed state is never installed as a persistent View instance, and its `state_revision` is only evaluation metadata for that refresh, not a persistent revision across refreshes. Any picker may declare `feeds`, not only the home view.

Configured plugins are trusted code: their scripts run with the launcher's user permissions and are not sandboxed from one another. When a called picker delegates item evaluation to feeds, each feed owner receives the same `request:args` as the page. Wildcard feeds therefore opt all matching providers into that request contract; do not load an untrusted plugin or pass secrets under the assumption that request namespaces are a security boundary.

Commands can open another concrete view. The command belongs in the owning plugin manifest:

```toml
[views.main.commands.apps]
key = "alt+a"
label = "Apps"
type = "navigate"

[views.main.commands.apps.payload]
target = "apps:main"
```

The runtime keeps a View stack. With `default_view = "core:default"`, opening `apps:main` produces:

```text
[core:default, apps:main]
```

Inside the session input bar, a canonical reference or unique alias enters a configured View through the normal view stack. For example, typing `apps:main terminal` and `app terminal` targets the same View when `apps:main` owns `alias = "app"`. After routing away from the root, chrome displays the active View's alias (or its canonical reference when no alias exists) as a non-editable prefix, while the routed View edits only `terminal`. The root stack entry has no prefix, regardless of whether that View is the configured default or an explicitly invoked View. CLI invocation remains keyed and does not use these positional route strings. A bare plugin ID is ordinary query text and is not expanded to a `default` View. `Esc`, or Backspace on an empty routed query, returns to the parent when the active engine assigns that behavior. Capture and embedded Views are normal children in the same stack.

A temporary selector is also just a View, opened by a `call` command. The first callee entry owns a call boundary; `push` can add descendants and `replace` transfers the boundary. `return` removes the complete callee branch, restores the caller's input, state, selection, and runtime context, then evaluates the optional continuation. Back from the first callee cancels the call without running the continuation. A `return` with no caller completes the external invocation.

## Expressions

Expressions use `{{ ... }}` and are evaluated by the engine that consumes them:

```toml
items = "{{ runtime:view.current.items }}"
commands = "{{ runtime:view.current.command }}"
label = "query: {{ runtime:view.current.query }}"
```

References use six namespaces: `config:` for static merged configuration, `this:` for the View instance that owns the expression, `runtime:` for mutable session/engine metadata, `input:` for the immutable stdin descriptor, `request:` for the current call request, and `return:` for a continuation's temporary result. `$` or an empty path refers to a complete namespace root. `this:query` is the current View instance's committed query value: an editable string when no query schema is declared (or when `query.type = "string"`), and a typed object for `query.type = "object"`; `config:` never receives a query overlay.

A callee reads caller-supplied data below `request:args`; `request:` is unavailable at roots and in ordinary navigation entries. A continuation sees `return:source` and `return:output`; picker output has `type = "selected"`, `item`, and `input`, while explicit and embedded values have `type = "value"` and `value`. During continuation evaluation, `this:` and `runtime:` already refer to the restored caller. `return:` is unavailable outside that evaluation, and neither temporary namespace is written into persistent runtime state.

```toml
items = '{{ path(runtime:view.current, "$.items") }}'
request = '{{ script("scripts/query.sh", {query = this:query, input = input:$}) }}'
selected = '{{ path(script("scripts/query.sh"), "$.items") }}'
handler = "{{ config:commands.script }} --query {{ runtime:view.current.query }}"
```

The built-in expression methods are `path` and `script`. `path(value, jsonpath)` applies a JSONPath expression to any JSON value; no match returns `null`, one match keeps its value type, and multiple matches return an array. `script(target, input, max_output_bytes)` runs a plugin-relative shell script, writes the optional JSON input to stdin, and parses the output as JSON. The script owns the input shape; the expression only chooses which JSON value to pass. Script output defaults to a 1 MiB limit; trusted data-source plugins may use the optional third argument to raise it as high as 64 MiB. Script input/output remain bounded and execution has a timeout. A complete placeholder keeps the returned JSON type. A mixed template must produce a string; arrays and objects cannot be implicitly interpolated into it. Methods are invoked only when the engine requests evaluation, so dynamic results can depend on the current runtime state and engine lifecycle.

## View commands

Commands belong to a concrete picker or capture View. Embedded Views currently reject View commands so PTY input and process completion remain independent from the generic command dispatcher. Each command declares `key`, `label`, `scope`, `requires`, and one tagged action. `scope` is `selection` by default; commands contributed by a feed owner are visible only with that scope. `scope = "view"` is appropriate for page-level tools. `requires` is `items` by default, which makes a picker wait for the matching item result before dispatch; `requires = "input"` dispatches as soon as the committed input is ready.

The action types are `run`, `navigate`, `call`, `return`, `edit-input`, and `invoke`. Action-specific fields belong to `payload`, and unknown or mismatched fields are rejected.

A `run` action executes in its owning plugin context. With `exit = true`, it exits the launcher after the handler finishes:

```toml
[views.main.commands.open]
key = "enter"
label = "Open"
type = "run"

[views.main.commands.open.payload]
handler = '''gio launch "$LAUNCHER_VALUE"'''
exit = true
```

A `navigate` action pushes a View without creating a return boundary. `target` may be a canonical View reference, a unique View alias, or an expression returning either form. `query` may be a string, which the target schema parses, or a directly validated object. Omitting `query` keeps the target defaults; an explicit empty string clears editable input.

```toml
[views.main.commands.inspect]
key = "alt+i"
label = "Inspect"
type = "navigate"

[views.main.commands.inspect.payload]
target = "{{ runtime:view.current.selected_item.metadata.target }}"
query = "{{ runtime:view.current.selected_item.value }}"
```

A `call` action opens any engine as a callee by canonical reference or unique alias and may provide `query`, arbitrary JSON `args`, and one recursive `then` action. Selectors therefore need no kernel support. The global Ctrl-K footer binding calls an ordinary command selector with the current command catalog:

```toml
[chrome.footer.bindings.commands]
key = "ctrl+k"
label = "Commands"
visibility = "overflow"
type = "call"

[chrome.footer.bindings.commands.payload]
target = "selectors:commands"
args = { commands = "{{ runtime:view.current.command }}" }
```

A `return` action unwinds the nearest call, or completes the invocation at the root. `payload.value` returns any evaluated JSON value. Without it, the action uses output prepared by the picker or capture engine. A direct root return can add a plugin-relative `handler` and evaluated `params`; `params` require a handler, and a Return nested under `call.payload.then` cannot define either field because continuations do not install root result adapters. After terminal restoration, the handler receives that JSON on stdin and controls raw stdout, stderr, and exit status. Handler stdout is limited to 16 MiB, stderr to 64 KiB, and runtime to 10 seconds. `cancel_exit_code` controls cancellation of a root invocation.

An `invoke` action evaluates an opaque `{"view":"...","id":"..."}` reference, then revalidates that command against the restored page and selected owner before dispatching it. A command selector can consume `runtime:view.current.command`, return an entry's `ref`, and use this continuation:

```toml
[chrome.footer.bindings.commands.payload.then]
type = "invoke"

[chrome.footer.bindings.commands.payload.then.payload]
command = "{{ return:output.value }}"
```

`edit-input` replaces the restored View's complete UTF-8 buffer. Its optional byte `cursor` must be on a character boundary; omitting it places the cursor at the end. The footer is assembled from current page commands and, when an item is selected, selection-scoped commands from the item's source View. Item JSON does not contain command definitions.

View commands and View keymap patches use the same named-key, printable ASCII, Ctrl, and Alt syntax. A picker semantic action wins when the same physical key is assigned to both, so a View keymap patch can replace or disable that key before using it as a View command. An explicit View command wins over the built-in default-View Tab behavior and a same-key chrome footer binding. Capture gives its View commands and global footer bindings priority over capture semantic bindings.

### Embedded results

An embedded View normally connects child stdin, stdout, and stderr to its PTY. Adding `engine.config.result` changes stdout into a dedicated bounded result pipe while stdin and stderr remain on the PTY. The CLI must render its terminal UI to stderr in this mode. When the child exits successfully, the parsed result automatically returns from the View; no Return key or launcher-specific live PTY protocol is exposed.

```toml
[views.form.engine]
type = "embedded"

[views.form.engine.config]
command = ["./form-cli"]
escape-cancels = false
result = { format = "json", required = true, max_bytes = 1048576 }
```

Embedded input is byte-preserving PTY input, including UTF-8, unknown escape sequences, and bracketed paste. `escape-cancels` defaults to `true`: a timed-out bare `Esc` terminates the child and cancels the View, while Alt and complete terminal escape sequences pass through. Set `escape-cancels = false` for programs such as nvim that own `Esc`; in that mode every input byte is written to the PTY immediately and the launcher exposes no cancel key. `Ctrl-C` is also ordinary child input rather than a launcher command. Chrome footer bindings and the command selector are disabled while the PTY is active. A bare embedded command is resolved using the configured working directory and PATH to one executable candidate; the candidate must be directly executable or a script with a shebang. The launcher does not promise the complete `execvp` EACCES search or ENOEXEC shell fallback. With `result` configured, a zero exit parses stdout as `text` or `json` and produces a Return. Text removes one trailing newline and its optional carriage return; JSON preserves its native type. An empty result is an error unless `required = false`, in which case the value is `null`. The default result limit is 1 MiB and the maximum is 16 MiB. Without `result`, any child exit returns to the previous View without running a call continuation; a nonzero exit also cancels a result-producing call.

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

The session publishes stack-top metadata under `runtime:view.current` and input under `runtime:session.input`. `runtime:session.views` is the public View catalog; `runtime:view.current.command` is the current command catalog, with each entry carrying an opaque `ref`, owner, normalized key, and label. The catalog is empty for embedded Views. Picker adds `selected_item`, `items`, and the selected feed owner's visible commands. Public picker items contain `prefix`, `text`, `value`, `metadata`, and one provenance field, `owner_view`; internal feed IDs, state, binding, and query snapshots are never exposed. `this` is the expression-owning definition context (`ref`, `query`, `input`, `raw_input`, `state_revision`) and does not include selection. Feed owner scripts typically take `this:query` or `this:$`. Selection belongs in command payloads as `runtime:view.current.selected_item`. Script output is parsed as one JSON document and must be an array for an `items` expression. The returned array is authoritative: its order is preserved, and the picker does not sort or filter valid items. Search, filtering, and sorting belong to the expression or plugin script.

A feeds picker evaluates the `items` expression of each owner in `engine.config.feeds`. Each result shows its owner view's alias, or its canonical reference when no alias is configured, in a right-aligned trailing column:

```text
Terminal       app
System monitor sys
Package details apps:detail
```

Typing `app terminal` enters the view owning alias `app` with `terminal` as its query, while `apps:main timeout` uses an exact canonical reference. Both aliases and canonical `plugin:view` references become routes only after a whitespace separator, so `app` and `apps:main` alone remain ordinary query text. The input bar belongs to the session chrome: a recognized route selector is transient routing state, while the target engine receives only the query. `Esc` from a routed child, or Backspace on its empty query, returns to the parent with an empty input buffer so a stale query cannot filter the parent view. Feed owner commands remain owned by the owner view.

## Command environment

Command scripts receive:

- `LAUNCHER_ITEM`, `LAUNCHER_VALUE`, and `LAUNCHER_METADATA` from the selected item;
- `LAUNCHER_ITEM_VIEW_REF` and `LAUNCHER_ITEM_PLUGIN` from the selected item's feed owner (empty when no item is selected);
- `LAUNCHER_PLUGIN`, `LAUNCHER_PLUGIN_DIR`, and `LAUNCHER_VIEW_REF` from the command owner;
- `LAUNCHER_VIEW` and `LAUNCHER_QUERY` from the parent page and its committed binding;
- `LAUNCHER_COMMAND` and optional `LAUNCHER_STDIN_FILE` when invocation stdin was captured to a file; the launcher runtime-log path is never exported.

Command `shell` selects the command interpreter. When omitted, the command owner's `run_shell` is used, then `sh`. File-backed commands run in the command owner's plugin directory. Owner commands evaluate against the selected feed context, while page commands evaluate against page state. Direct bindings and selector-returned `CommandRef` values are dispatched from the same snapshots and validation path. Navigate, call, return, edit-input, and invoke expressions see the command owner's committed binding through `this:raw_input`.

An embedded View starts its argv in the target plugin directory and receives `LAUNCHER_VIEW_REF`, `LAUNCHER_INPUT`, `LAUNCHER_PLUGIN`, and optional `LAUNCHER_PLUGIN_DIR`; it never receives the launcher runtime-log path. Navigation does not implicitly carry source item metadata; evaluate the target `query` or call `args` explicitly when it is needed.

Picker engine configs accept `show_prefix`; View keymap patches are declared at `[views.<name>.keymap]` and apply to the effective bindings of the selected engine. Owner prefixes are hidden by default; feeds pickers may set `engine.config.show_prefix = true` to identify each owner. Input defaults and types belong to the View's `query` schema; acceptance belongs to an ordinary `return` command. Picker has no activation mode, local filter, initial-input field, or item search field.

## Keys

Picker shortcuts are semantic engine bindings. Root defaults apply to every picker View:

```toml
[defaults.picker.bindings]
exit = ["ctrl+c", "ctrl+d"]
back = ["escape"]
select_previous = ["up"]
select_next = ["down"]
delete_backward = ["backspace"]
clear_input = ["ctrl+u"]
delete_word = ["ctrl+w"]
activate = ["enter"]
toggle_preview = ["ctrl+p"]
```

A View overrides the effective physical keys without knowing which engine supplied them. A value of `false` disables an inherited key (a tombstone that survives recursive plugin/user configuration merging), while a string assigns or replaces the action on that key:

```toml
[views.main.keymap]
enter = false
"ctrl+p" = false
"space" = "activate"
```

Unmentioned keys remain unchanged. A key cannot be both disabled and rebound in one patch. Key aliases are canonicalized before plugin and user tables are merged, and conflicting aliases in one layer are rejected. Action names are checked against the selected engine; a key patch does not need to identify the layer or engine that originally supplied the key. Bindings accept `space`, printable ASCII characters, `enter`, `tab`, `backtab`, `backspace`, `delete`, `left`, `right`, `home`, `end`, `up`, `down`, `escape`, `ctrl+<letter>`, and `alt+<character>`.

Available picker actions are `exit`, `back`, `select_previous`, `select_next`, `delete_backward`, `clear_input`, `delete_word`, `activate`, and `toggle_preview`. Picker defaults are configured under `[defaults.picker.bindings]`; a View patch can replace or disable any effective picker key.

Capture's engine defaults copy the complete, unsanitized capture output with Enter and return with Escape:

```toml
[defaults.capture.bindings]
copy = ["enter"]
back = ["escape"]

[views.output.keymap]
enter = false
"ctrl+y" = "copy"
```

Available capture actions are `copy` and `back`. Disabled or otherwise unbound keys are ignored. Copy uses OSC 52 and wraps the sequence for tmux; clipboard acceptance still depends on the terminal or multiplexer configuration.

Tab opens the Router's built-in visual route completion only in the normal launcher's root View while its input is focused. The overlay excludes the current default View and searches the remaining configured aliases, canonical View references, and plugin names; Tab/Down and Shift-Tab/Up move through candidates, Enter inserts the selected canonical View reference into the input, and Esc closes the overlay. It is route grammar UI rather than a View command: it creates no Call boundary, is not rendered as an input hint, is unavailable to directly invoked Views and child Views, and is never published in `runtime:view.current.command`. Engine semantic bindings and explicit View commands retain priority over the built-in Tab behavior. Session chrome separately defines global footer bindings under `chrome.footer.bindings`; the fixture uses Ctrl-K to call the ordinary `selectors:commands` View with `runtime:view.current.command`, then invokes the returned strict `CommandRef`. Footer bindings are unavailable while an embedded PTY is active.

For picker Views, semantic engine bindings take priority over View commands on the same key. An unavailable `toggle_preview` binding still consumes its key without editing input; an `activate` binding delegates to the same-key View command when one exists. Embedded Views reserve only timed-out bare `Esc` for cancellation and otherwise forward input to the PTY. Capture Views resolve View commands and global footer bindings first, then their `copy` and `back` semantic bindings.

Top status, input, divider, content, and footer chrome are composed and rendered centrally from the active route, the current engine, and global errors. The top status line is reserved as blank space. The root input line has no route prefix. Child Views display the router-provided alias, falling back to the canonical View reference, and keep the editable query and cursor separate from that prefix. Long input scrolls around the cursor; non-focus Views mute only the query text. The divider is a plain horizontal rule. The footer follows the content directly and uses the active theme's `chrome-footer` binding inside the viewport padding. Engine title and status remain on the left of the footer, while engine command keys use the `chrome-footer-key` binding and their descriptions remain plain text. Footer hints are omitted when the active engine, View command, or built-in router owns the same key. An overflow footer binding is shown only when all current View commands do not fit; its key remains active when the hint is hidden. View keymap patches are behavioral and are not rendered in the input line. Picker footers hide View commands whose keys are consumed by semantic actions, except `activate` keys that dispatch the command; capture footers additionally render effective `copy` and `back` actions, omitting `copy` for failed captures. A current error temporarily replaces the complete footer and includes its occurrence time. The latest error replaces the previous one and is cleared after five seconds, a new query, a view change, a successful refresh, or a successful command.

Errors and command status records are written exclusively by the launcher. By default the JSONL file is `$XDG_STATE_HOME/tui-launcher/runtime.jsonl`, falling back to `$HOME/.local/state/tui-launcher/runtime.jsonl`; set the root `log_file` configuration to override it, for example `log_file = "state/runtime.jsonl"`. Relative paths are resolved from the main configuration file's directory. Plugin commands and embedded processes do not receive the log path or write this file, and there is no log picker/plugin. The launcher opens one append-only regular file for its lifetime, using `O_NOFOLLOW`, rejecting special files, limiting individual messages, and assembling each JSONL record before one `write_all`. An initial open or later write failure disables logging and reports the degradation once through the footer when possible or on stderr for an immediate exit. Concurrent launcher instances sharing one path are unsupported; there is no file snapshot, lock, rotation, or cross-process completeness guarantee.

The first external SIGTERM, SIGHUP, SIGQUIT, or SIGINT requests cooperative cancellation and best-effort cleanup. A second signal exits immediately without further cleanup guarantees. After cleanup enters final-output, even the first signal exits immediately. stdout and stderr are not transactional: output is valid only when the invocation exits with status 0, and an interrupted invocation may leave a prefix. Terminal restoration is best-effort and is not guaranteed after terminal damage or disconnect. Task and child-process cleanup runs outside the signal handler. Signal exits use status `128 + signal`; no bounded-exit guarantee is made for arbitrary kernel I/O blocking.

Launcher input is committed to View state and runtime immediately; picker item refreshes are then scheduled by the shared input controller with a 120ms debounce and evaluated by the single Session items worker. A monotonically increasing request generation prevents results from an older snapshot from being accepted. Each script has a 10-second timeout, is limited to 64 KiB of JSON input, defaults to 1 MiB of stdout, and may explicitly raise stdout up to 64 MiB; stderr remains limited to 64 KiB. Timed-out or oversized scripts report an error for that source. A command with `requires = "items"` waits for the matching result; `requires = "input"` does not. A Session accepts at most 100,000 aggregated picker items; excess feed items report an error, and item parsing observes cancellation.
