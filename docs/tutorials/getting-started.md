---
title: "Getting Started with tui-launcher"
type: "tutorial"
tags:
  - onboarding
  - quickstart
  - setup
description: "A step-by-step introduction to building, configuring, and running tui-launcher for the first time."
---

# Getting Started with tui-launcher

This tutorial walks you through setting up `tui-launcher`, creating a minimal configuration file, and launching your first view.

## 1. Prerequisites & Installation

Ensure you have Rust and Cargo installed (edition 2021 or later). Clone the repository and build the binary:

```bash
git clone https://github.com/example/tui-launcher.git
cd tui-launcher
cargo build --release
```

The compiled binary will be located at `target/release/tui-launcher`. You can add it to your `$PATH` or create an alias.

## 2. Directory Layout

`tui-launcher` discovers configurations according to the XDG Base Directory specification:

```text
$XDG_CONFIG_HOME/tui-launcher/
├── config.toml         # Main launcher settings
├── themes/             # Named themes
└── plugins/            # Installed plugins
    └── <plugin-id>/
        ├── plugin.toml # Plugin manifest
        └── scripts/    # Local shell scripts
```

If `$XDG_CONFIG_HOME` is unset, it defaults to `$HOME/.config/tui-launcher/`.

Create this directory structure now:

```bash
mkdir -p ~/.config/tui-launcher/plugins/hello
```

## 3. Create a Minimal Plugin

Inside `~/.config/tui-launcher/plugins/hello/plugin.toml`, declare a simple plugin with a `picker` view:

```toml
[plugin]
api = 1
name = "Hello Launcher"

[views.main]
alias = "hello"

[views.main.engine]
type = "picker"

[views.main.engine.config]
items = [
  { label = "Echo Hello", value = "hello" },
  { label = "Current Date", value = "date" },
]

[views.main.commands.execute]
key = "enter"
label = "Run"
type = "run"

[views.main.commands.execute.payload]
handler = { source = "inline", command = "echo Selected: {{ selection.value }}" }
exit = true
```

## 4. Create the Main Configuration

Create `~/.config/tui-launcher/config.toml` to set this view as your default:

```toml
default_view = "hello:main"
```

## 5. Validate and Launch

Before opening the TUI, you can validate the configuration:

```bash
tui-launcher --check
```

If the validation passes with zero errors, start the launcher:

```bash
tui-launcher
```

You will see an interactive picker containing your items. Pressing `Enter` runs the command and closes the launcher.

## Next Steps

Now that you have a working launcher:
- Follow [Your First Plugin](first-plugin.md) to explore parameters and script execution.
- Learn how to build [Dynamic Picker Feeds](../how-to/dynamic-picker-feeds.md) using external shell scripts.
