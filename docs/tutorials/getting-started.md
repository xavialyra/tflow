---
title: "Getting Started with tlaunch"
type: "tutorial"
tags:
  - onboarding
  - quickstart
  - setup
  - producers
description: "Build, configure, validate, and launch a minimal workflow using literal items and a JSON command producer."
---

# Getting Started with tlaunch

This tutorial builds the binary, creates a minimal single-file workflow, and launches a Picker whose command is backed by a version-1 producer script.

## 1. Install

Ensure Rust and Cargo are installed, then build the binary:

```bash
git clone https://github.com/example/tlaunch.git
cd tlaunch
cargo build --release
```

The binary is `target/release/tlaunch`. Add it to your `$PATH` or invoke it by its path.

## 2. Create the Configuration Directory

`tlaunch` discovers configuration using the XDG Base Directory layout:

```text
$XDG_CONFIG_HOME/tlaunch/
├── settings.toml  # optional host settings
├── default.toml   # explicit suite manifest
└── workflows/
    ├── hello.toml
    └── git/
        ├── workflow.toml
        └── scripts/
```

If `$XDG_CONFIG_HOME` is unset, the default is `$HOME/.config`. Create the workflow directory:

```bash
mkdir -p ~/.config/tlaunch/workflows
```

## 3. Create a Minimal Workflow

Create `~/.config/tlaunch/workflows/hello.toml`:

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

Create `~/.config/tlaunch/default.toml`:

```toml
[suite]
api = 1
name = "My tools"
entrypoint = "hello:main"

[workflows]
hello = { file = "./workflows/hello.toml" }
```

## 5. Validate and Launch

Run validation before opening the TUI:

```bash
tlaunch --check
```

Then launch it:

```bash
tlaunch
```

The Picker displays the two literal items. Press `Enter` to run the producer operation. The foreground command inherits the caller's working directory and the launcher exits because the operation sets `exit = true`.

## Next Steps

- Follow [Your First Workflow](first-workflow.md) to build a directory package with separate item and command scripts.
- Read [Picker Views](../how-to/picker-views.md) for request-driven item generation.
- Add [Route Completion to a Launcher](../how-to/route-completion.md) once several workflows share one suite entrypoint.
- Use [workflow.toml Specification](../reference/workflow-toml.md) for the full operation and protocol reference.
