# tui-launcher

`tui-launcher` is a small dmenu-style TUI workflow launcher. It runs ordinary discovery and command scripts, keeps named views in a view stack, and exposes view-owned commands for the current selection.

The launcher separates three concepts:

- a plugin namespace, such as `apps` or `ssh`;
- a concrete view reference, such as `apps:main` or `apps:detail`;
- a view type, such as `launcher`, `capture`, or `embedded`.

Several concrete views can use the same view type. For example, `default` and `app` can both be launcher views while keeping separate discovery data and commands.

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

The dmenu UI reads keyboard input and draws through `/dev/tty`. It loads the configured dedicated dmenu view (default: `core:dmenu`) for display settings, but does not run that view's discovery, sources, or commands. Standard input is consumed as a newline-delimited snapshot before the selector opens; `--dmenu0` uses NUL-delimited records and NUL-terminated output. On acceptance, standard output contains the complete selected input line followed by a newline. `Esc`, `Ctrl-C`, and `Ctrl-D` cancel with a non-zero exit status. If the query does not match an entry, the query text itself is returned. Rofi metadata after a NUL separator is parsed into generic candidate metadata; the current text renderer ignores it.

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

Field ranges use the Fuzzel-style `{N..M}` and `{N..}` forms. A symlink whose basename is `dmenu` also starts the program in dmenu mode. Set `dmenu_view = "core:dmenu"` in the root config to choose the dedicated display view; no dmenu view-selection CLI flag is provided yet. Dmenu mode can use `--config`, but cannot be combined with `--check`. The dedicated view is a launcher view with no discovery or commands:

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

A view has a `type` which determines its renderer and input model:

- `launcher`: query input, discovery results, selection, and view commands;
- `capture`: captured command output and return controls;
- `embedded`: a managed PTY with input forwarded to the child.

`exit` is not a view. A command with `exit = true` returns an exit event after its script finishes.

A plugin is packaged as a directory so its configuration and scripts stay together:

```text
plugins/apps/
├── plugin.toml
├── scripts/
│   ├── discover.sh
│   └── open.sh
└── lib/
    └── common.sh
```

The plugin ID is derived from the package directory name. The optional `[plugin]` metadata table currently only carries the launcher protocol version; `api` defaults to `1` when it is omitted. Its view definitions are relative to that directory namespace:

```toml
[views.main]
type = "launcher"
discover_shell = "bash"
discover = { file = "scripts/discover.sh" }

[views.main.commands.open]
key = "enter"
label = "Open"
run = { file = "scripts/open.sh" }
```

Set `[plugin].api = 1` explicitly when desired. Unsupported API versions are rejected. Plugin directory names must not contain `:` or whitespace because they form the first part of a view reference. Script paths must remain below the plugin directory. Inline scripts are still supported for small commands. Git source, release version, and lock data are not part of the runtime manifest yet; a plugin directory can still be maintained as a Git checkout.

The root config file contains the default view and global rules:

```toml
default_view = "core:default"
```

The `core` plugin can aggregate launcher views from several plugin packages:

```toml
[views.default]
type = "launcher"
sources = ["sys:default", "apps:default"]
```

This table belongs in `plugins/core/plugin.toml`, not in the root `config.toml`.

A view with `sources` displays the results of those launcher views. Aggregate views cannot define commands. A source view keeps its own discovery configuration and remains the owner of the resulting item commands.

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

`Esc` returns to the parent view when the query is empty. A capture or embedded view is treated as a temporary child of the view that launched it. `Ctrl-K` opens the configured command launcher view (default: `core:command`) for the selected item's source view; selecting a command returns to the parent frame before executing it.

## View commands

Commands belong to a concrete view. `Enter` is not a special provider action; it is an ordinary command binding:

```toml
[views.main.commands.open]
key = "enter"
label = "Open"
run = '''
gio launch "$LAUNCHER_VALUE"
'''
```

A command can run a script and optionally route its output to a built-in view:

```toml
[views.main.commands.inspect]
key = "alt+i"
label = "Inspect"
view = "core:capture"
run = '''
printf 'desktop file: %s\n' "$LAUNCHER_VALUE"
'''
```

An interactive command can target the embedded view:

```toml
[views.main.commands.shell]
key = "alt+s"
label = "Shell"
view = "core:embedded"
run = '''
exec sh
'''
```

A command that has a `view` but no `run` is a view navigation command. A command with `exit = true` transfers the terminal to its script and exits the launcher after it finishes:

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

Only `Enter` and `Alt+<character>` are available for plugin commands. Plain characters remain search input. `Esc`, `Ctrl-C`, `Ctrl-D`, arrows, and input editing controls are reserved by the launcher or the active child view.

## Discovery

A launcher view with `discover`, `default_discover`, or `query_discover` writes one JSON object per line. Each item must contain a `label` and may contain a stable `value` plus arbitrary `metadata`. Discovery fields can contain inline shell text or a plugin-relative file reference such as `discover = { file = "scripts/discover.sh" }`:

```json
{"label":"Termius","value":"termius.desktop","metadata":{"kind":"app"}}
```

View discovery scripts receive:

- `LAUNCHER_PLUGIN`;
- `LAUNCHER_PLUGIN_DIR` when the view belongs to a file-backed plugin;
- `LAUNCHER_VIEW`;
- `LAUNCHER_VIEW_REF`;
- `LAUNCHER_RULE`;
- `LAUNCHER_QUERY`;
- `LAUNCHER_LOG_FILE`, the append-only runtime log path.

A root launcher view runs discovery for each view in `sources`. `display_prefix` is shown in the result list and acts as a source selector when followed by a space:

```text
app terminal
ssh prod
```

When a view with an explicit `display_prefix` is not a source of the current view, the same prefix enters that launcher view through the normal view stack. For example, a standalone view with `display_prefix = "log"` is entered with `log timeout`; `timeout` becomes its query. Source selection stays in the current frame, while view routing pushes a new frame.

If `display_prefix` is omitted, the view name is used for source display and selection. `default_discover` is used for an unqualified source query and `query_discover` is used after a matching source prefix. A standalone prefixed view should provide `discover`, because its prefix is removed before its first discovery request.

`discover_shell` selects the discovery interpreter and defaults to `sh`. Set it to `bash` when the script uses Bash syntax.

The core plugin provides a normal log launcher without adding it to `core:default`:

```toml
[views.messages]
type = "launcher"
display_prefix = "log"
discover = { file = "scripts/messages.sh" }
```

The script can read the runtime log directly:

```sh
cat "$LAUNCHER_LOG_FILE"
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
- `LAUNCHER_RULE`;
- `LAUNCHER_QUERY`;
- `LAUNCHER_LOG_FILE`.

`LAUNCHER_PROVIDER` remains available as an alias for the source plugin name for compatibility with older scripts.

Command `shell` selects the command interpreter. When omitted, the source view's `run_shell` is used, then `sh`. File-backed plugin commands run with the plugin directory as their working directory, so relative paths and `LAUNCHER_PLUGIN_DIR` are stable.

## Keys

- `Enter`: execute the current view's Enter command;
- `Alt+<character>`: execute a view command;
- `Up` / `Down`: move through results;
- `Esc`: clear the query, then return to the parent view or quit at the root;
- `Ctrl-C`: quit;
- `Ctrl-U`: clear the query;
- `Ctrl-W`: delete the previous word;
- `Ctrl-K`: open the command launcher view for the selected item's source view.

The launcher footer occupies one fixed row. It keeps the current view on the left and view commands on the right. A current error temporarily replaces the left side and includes its occurrence time; the latest error replaces the previous one and is cleared after five seconds, a new query, a view change, a successful refresh, or a successful command. Errors and command status records are also appended to the runtime JSONL log at `$XDG_STATE_HOME/tui-launcher/runtime.jsonl` or `$HOME/.local/state/tui-launcher/runtime.jsonl`. `TUI_LAUNCHER_LOG_FILE` overrides the path.

When additional view commands do not fit, `Ctrl-K commands` navigates to the command launcher view; `Up` / `Down` select a command, `Enter` runs it, and `Esc` returns to the previous view.

Discovery is debounced globally by 120ms and executed by a background worker. Older results are discarded when a newer view/query request exists. Each discovery command receives a null stdin, has a 10-second timeout, and is limited to 1 MiB of stdout and 64 KiB of stderr. Timed-out or oversized commands report an error for that source. If `Enter` is pressed while discovery is pending, it waits for the matching result before executing the view command.

## Legacy configuration

The loader still accepts the old `[providers.<name>]` format inside the configuration file. Each provider is converted to `<name>:default`, and providers with discovery scripts are added to `core:default` when that aggregate view exists. The old `run` field becomes an `Enter` command.

Legacy modes are mapped as follows:

```text
oneshot  -> command without a target view
capture  -> view = "core:capture"
embedded -> view = "core:embedded"
takeover -> exit = true
```

New configuration should use the `config.toml` plus `plugins/<id>/plugin.toml` layout and namespaced plugin views directly.
