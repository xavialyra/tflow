---
title: "Getting Started with tflow"
type: "tutorial"
tags:
  - onboarding
  - quickstart
  - setup
  - producers
description: "Understand workflows, suites, and settings, then configure, validate, and launch your first workflow."
---

# Getting Started with tflow

This tutorial explains the configuration files, creates a minimal workflow and default suite, and launches an interactive list (a Picker).

## Understand the Configuration Files

| Concept | Purpose | File in this tutorial |
| --- | --- | --- |
| **Workflow** | Defines a tool's views and commands. It can run on its own or be included in a suite. | `workflows/hello.toml` |
| **Suite** | Groups workflows, selects the initial view, and optionally defines shorthand names (aliases). | `default.toml` |
| **Settings** | Sets shared preferences such as theme, image protocol, and default keys. Applies to both standalone workflows and suites. | `settings.toml` (optional) |

Running `tflow` loads the default suite. Use `tflow -s <suite.toml>` to choose another suite, or `tflow -w <workflow.toml>` to run a workflow directly without a suite. Settings are optional; built-in defaults apply when the default settings file is absent.

A suite lists its workflows explicitly: placing a file in `workflows/` does not automatically add it. Views and commands belong in workflow files; shared preferences belong in settings.

## 1. Install

If `tflow` is already installed, continue to step 2. Otherwise, install Rust and Cargo, then build the binary. This tutorial also requires Python 3 for the example command.

```bash
git clone https://github.com/xavialyra/tflow.git
cd tflow
cargo build --release
```

The binary is `target/release/tflow`. Add it to your `$PATH` or invoke it by its path.

## 2. Create the Configuration Directory

`tflow` discovers configuration using the XDG Base Directory layout:

```text
$XDG_CONFIG_HOME/tflow/
├── settings.toml  # optional host settings
├── default.toml   # explicit suite manifest
└── workflows/
    ├── hello.toml
    └── git/
        ├── workflow.toml
        └── scripts/
```

If `$XDG_CONFIG_HOME` is unset, the default is `$HOME/.config`. Set a shell variable for the configuration directory and use it throughout this tutorial:

```bash
tflow_config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/tflow"
mkdir -p "$tflow_config_dir/workflows"
```

The `git/` directory above illustrates a directory workflow package; it is not needed for this tutorial.

## 3. Create a Minimal Workflow

Save the following as `$tflow_config_dir/workflows/hello.toml`:

```toml
[workflow]
api = 1
name = "Hello Launcher"
entrypoint = "main"

[views.main.engine]
type = "picker"

[views.main.engine.config]
items = [
  { display = "Echo Hello", value = "hello", metadata = {} },
  { display = "Current Date", value = "date", metadata = {} },
]

[views.main.keymap]
enter = "execute"

[commands.execute]
label = "Run"
type = "run"
producer = "script"

[commands.execute.handler]
script = '''#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
state = request["context"]["engine"]["state"]
item = state.get("item") if isinstance(state, dict) else None
value = item.get("value") if isinstance(item, dict) else None
if not isinstance(value, str):
    raise SystemExit("select an item first")
json.dump({
    "version": 1,
    "operation": {
        "type": "run",
        "mode": "foreground",
        "argv": ["printf", "selected:%s\\n" % value],
        "exit": True,
    },
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
'''
```

The handler reads the selected item from `context.engine.state` and emits one `run` operation. It does not interpolate a value into TOML or print diagnostics to stdout.

Commands live at the workflow root because they are workflow-scoped: `[views.main.keymap]` is what exposes `commands.execute`, and the same command can be bound by any other view in this workflow. See [Workflow Commands](../reference/workflow-toml.md#workflow-commands).

## 4. Create the Default Suite

Save the following as `$tflow_config_dir/default.toml`:

```toml
[suite]
api = 1
name = "My tools"
entrypoint = "hello:main"

[workflows]
hello = { file = "./workflows/hello.toml" }
```

The `[workflows]` table registers `hello` as the name of this workflow in the suite. Its file path is relative to `default.toml`. The entrypoint `hello:main` opens the `main` view in that workflow. You can also run `tflow hello` to open the workflow's own entrypoint.

## 5. Customize Settings (Optional)

You can customize key bindings in `$tflow_config_dir/settings.toml`. For example, the following configuration changes the keys for moving between items. If that file already exists, update the matching entries instead of replacing it:

```toml
[defaults.picker.bindings]
select_previous = ["up", "ctrl+k"]
select_next = ["down", "ctrl+j"]
```

These defaults apply to Pickers in both suite and standalone workflow sessions. You can skip this file to keep the built-in defaults. See the [settings reference](../reference/settings-toml.md) for themes and other preferences, and the [suite reference](../reference/suite-toml.md) for orchestrating multiple workflows and aliases.

## 6. Validate and Launch

Run validation before opening the TUI:

```bash
tflow --check
```

Then launch it:

```bash
tflow
```

The Picker displays the two literal items. Press `Enter` to run the producer operation. The foreground command inherits the caller's working directory and the launcher exits because the operation sets `exit = true`.

## Next Steps

- Use the [settings.toml Specification](../reference/settings-toml.md) to customize shared preferences.
- Use the [Suite Manifest Specification](../reference/suite-toml.md) to organize workflows, entrypoints, and routing aliases.
- Follow [Your First Workflow](first-workflow.md) to build a directory package with separate item and command scripts.
- Read [Picker Views](../how-to/picker-views.md) for request-driven item generation.
- Use [workflow.toml Specification](../reference/workflow-toml.md) for the full operation and protocol reference.
