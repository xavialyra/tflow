# tui-launcher

`tui-launcher` is a small dmenu-style TUI workflow launcher. It merges newline-delimited output from ordinary discovery commands, routes input by configured prefixes, and runs the selected item with an ordinary CLI command.

The MVP intentionally has no tabs, sessions, panes, or daemon. One action runs at a time and returns to the launcher unless it uses `takeover` mode.

## Run the example

```bash
cargo run -- --config config.example.toml
```

The example includes:

- a default merged rule for actions and bookmarks
- `ssh ` prefix discovery with a query argument
- `:` prefix for system actions
- `capture` output for scripts
- `takeover` for SSH

The built-in configuration automatically includes every provider with a `discover` or `default_discover` command in the initial result list. It includes an `app` provider when `fzf` is installed; that provider reads desktop entries from the XDG application directories, caches the index under `$XDG_CACHE_HOME/tui-launcher` for five minutes, uses fzf for fuzzy discovery, and launches the selected entry with `gio launch`. It also provides a read-only `tr` provider with only a `query_discover` command, so `tr :zh hello` runs Translate Shell only after the prefix is entered and displays the result as a launcher item. The launcher treats a displayed provider prefix followed by a space as a global provider selector, so `sys date`, `shell `, `app term`, and `tr :zh hello` only query the matching provider.

The binary also contains a small built-in configuration, so it can run without any user file:

```bash
cargo run
```

The default user configuration path is `~/.config/tui-launcher/config.toml`, or `$XDG_CONFIG_HOME/tui-launcher/config.toml` when `XDG_CONFIG_HOME` is set. If that file does not exist, the built-in configuration is used unchanged. When it does exist, it is recursively merged on top of the built-in configuration:

- tables such as `rules` and `providers` are merged by key
- arrays such as `providers = [...]` and command arguments are replaced
- scalar values are replaced

This lets a user add a rule or override one provider without copying the whole default configuration.

Keys:

- `Enter`: run the selected item
- `Up` / `Down`: move through results
- `Esc`: clear the query, then quit when the query is empty
- `Ctrl-C`: quit
- `Ctrl-U`: clear the query
- `Ctrl-W`: delete the previous word

The input is updated immediately while discovery refresh is debounced globally by 120ms and executed by a background worker. Older results are discarded when a newer query exists. `Enter` flushes a pending refresh and waits for the latest result before running the selected item.

## Configuration

```toml
# Providers with `discover` or `default_discover` participate in the default list.


[providers.apps]
display_prefix = "app"
discover = '''
my-app-list --json
'''
query_discover = '''
my-app-query --json "$LAUNCHER_QUERY"
'''
run = '''
exec my-app-run "$LAUNCHER_VALUE"
'''
mode = "capture"

[providers.hosts]
display_prefix = "ssh"
discover_shell = "bash"
query_discover = '''
my-host-list --json "$LAUNCHER_QUERY"
'''
run = '''
exec ssh "$LAUNCHER_VALUE"
'''
mode = "takeover"
```

`discover` is the common shell script and writes one JSON object per line to stdout. Each result must contain a `label` and may contain a stable `value` plus arbitrary `metadata`, for example `{"label":"Termius","value":"termius.desktop","metadata":{"desktop_file":"/.../termius.desktop"}}`. The query is available to every discovery script as `LAUNCHER_QUERY`. If default and query discovery differ, use `default_discover` for the default list and `query_discover` for a matched provider prefix. A provider with only `query_discover` is route-only and is not called for the initial default list. A provider with `filter = false` is responsible for applying its own query filtering, as the built-in fzf and trans providers do. `discover_shell` selects the interpreter and defaults to `sh`.

`display_prefix` is shown in the first column of the result list and also acts as a global provider selector when followed by a space in the default search. The discovery label is shown in the second column. If `display_prefix` is omitted, the provider ID is used. Run scripts receive the label through `LAUNCHER_ITEM`, the stable value through `LAUNCHER_VALUE`, and the original metadata JSON through `LAUNCHER_METADATA`.

The selected item is available to action scripts as:

- `LAUNCHER_ITEM`
- `LAUNCHER_VALUE`
- `LAUNCHER_METADATA`
- `LAUNCHER_PROVIDER`
- `LAUNCHER_RULE`
- `LAUNCHER_QUERY`

Discovery and run commands are shell scripts. `discover_shell` and `run_shell` select their interpreters and default to `sh`; use `bash` when the script needs Bash syntax. Dynamic item data is passed through environment variables rather than interpolated into the script.

Action modes:

- `oneshot`: restore the terminal, run the command, then return to the launcher
- `capture`: capture stdout/stderr and show the result in a temporary view
- `embedded`: run the command in a managed PTY and relay its terminal output; a bare `Esc` stops it and returns to the launcher
- `takeover`: restore the terminal, run the command directly, and exit the launcher after it finishes

The current embedded mode uses a small VT screen for the child content and keeps the launcher title, input area, status line, and hint line fixed around it. It intentionally supports only common text-terminal control sequences.

Validate a configuration without opening the TUI:

```bash
cargo run -- --config config.example.toml --check
```

## Current boundary

Embedded actions run in their own PTY. The launcher parses common text-terminal sequences into a child screen, draws that screen between its fixed top and bottom areas, updates the PTY size, and returns to the launcher when the child exits or when a bare `Esc` is pressed. Input escape sequences for arrows, function keys, and Alt combinations are forwarded to the child.

The embedded screen does not yet implement the full terminal protocol: rich styles, mouse reporting, terminal graphics, and every private mode are outside this MVP. Applications that need an unrestricted real terminal should use `takeover`; `embedded` reserves only a bare `Esc` for returning to the launcher.
