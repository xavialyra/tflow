---
title: "settings.toml Specification"
type: "reference"
tags:
  - config
  - toml
  - specification
  - session
description: "Authoritative reference for tlaunch root configuration, defaults, themes, and session bindings."
---

# settings.toml Specification

The passive host environment lives at `$XDG_CONFIG_HOME/tlaunch/settings.toml`.
Both standalone workflows and suites inherit it. The historical `config.toml`
combination of environment and workflow discovery is no longer supported.

## Schema Overview

```toml
theme = "theme_name"
image_protocol = "kitty"
log_file = "/tmp/tlaunch.log"

[defaults.picker.bindings]
exit = ["ctrl+c", "ctrl+d"]
back = ["escape"]
select_previous = ["up"]
select_next = ["down"]

[styles.git.staged]
foreground = "scheme:accent"
bold = false
underline = true
```

`theme` names `themes/<name>.toml` beside settings. Omission uses the terminal
theme; `--theme` overrides selection. Semantic slot overrides merge individual
fields over workflow and suite values, including explicit `false` modifiers.
Theme-file workflow overrides are applied after these layers.

Settings reject `default_view`, `disabled_workflows`, workflow mounts, aliases,
and suite/workflow headers. Session orchestration belongs in a suite manifest.

## Suite Manifest

Default launch loads `default.toml`; `-s <PATH>` selects another suite.

```toml
[suite]
api = 1
name = "Tools"
entrypoint = "git:branches"

[workflows]
git = { file = "./git.toml" }
docker = { dir = "./docker" }

[aliases]
co = "git:branches"

[styles.git.staged]
foreground = "scheme:accent"
```

Each member mounts one atomic workflow. The member key automatically routes to
its local entrypoint. An explicit alias cannot redirect a member name to a
different view; a repeated alias with the same target is allowed. Suites reject
`theme`, `image_protocol`, `log_file`, and `defaults`; these belong to settings.
Suites cannot mount suites or define views. See [ADR 0005](../adr/0005-manifest-driven-suites-and-self-contained-workflows.md).

## Engine Defaults (`[defaults.<engine>.bindings]`)

Global keybindings for engines can be adjusted at the root level.

### `[defaults.picker.bindings]`
Available configurable actions for the `picker` engine:
- `exit`: Array of key combinations to immediately exit the launcher.
- `back`: Return to the parent view without changing the input.
- `clear_input`: Clear the current input. The default key is `ctrl+u`.
- `select_previous`: Move selection up.
- `select_next`: Move selection down.
- `toggle_preview`: Toggle the initially collapsed preview in any Picker. The default key is `ctrl+p`.
- `preview_scroll_up`, `preview_scroll_down`: Scroll the preview by three rows. No default keys are assigned.

Picker Enter behavior is configured by the View's explicit command bindings. The Picker engine has no implicit primary-selection action.

## Workflow Commands and Host Actions

Settings and suites reject `[commands]`. Business commands belong to atomic
workflows: `[commands.<id>]` supplies workflow-local defaults and
`[views.<name>.commands.<id>]` defines or overrides a view command. See the
[workflow specification](workflow-toml.md#workflow-commands).

The command palette is opened by the host with `Ctrl-K` when command folding is active. It is implemented as a built-in Popup Picker View. The host passes the eligible command descriptors to that View through the Popup navigation request; the View returns the selected command reference to the host for execution.

The built-in workflows are loaded before user workflow packages under reserved IDs prefixed with `__`. They provide the ordinary Picker route `__commands:main` for the command palette and the native Form route `__form:main` for parameter editing. These routes are host-owned and are not user-declared session commands. Any workflow ID starting with `__` is reserved for built-in workflows; user workflows cannot use the `__` prefix.
