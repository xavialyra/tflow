# tflow

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform: Linux](https://img.shields.io/badge/Platform-Linux-lightgrey.svg)](#)
[![Status: Alpha](https://img.shields.io/badge/Status-Alpha-orange.svg)](#)

`tflow` is a programmable framework for building interactive terminal workflows. It provides the building blocks for defining terminal user interfaces, commands, data pipelines, and user interaction—a desktop application launcher is just one of many tools that can be created with it.

> [!WARNING]
> `tflow` is currently in alpha and targets Linux. Protocols and manifest schemas may evolve before stability. Suggestions, bug reports, and feedback are welcome in the [GitHub issue tracker](https://github.com/xavialyra/tflow/issues).

---

## Core Concepts

The architecture separates concerns into three distinct layers:

| Component | Flag / File | Role |
| :--- | :--- | :--- |
| **Workflow** | `-w` / `workflow.toml` | **Self-contained tool unit.** Declares interactive views, keybindings, commands, and producer scripts. |
| **Suite** | `-s` / `<suite>.toml` | **Orchestration bus.** Mounts multiple member workflows, assigns public shorthand aliases, and defines the session entrypoint. |
| **Settings** | `settings.toml` | **Passive host preferences.** Configures ambient terminal behavior: themes, image preview protocols, and global keybindings. |

---

## Features

- **Pipeline Friendly:** Drop workflows directly into shell pipelines as interactive filters between standard commands.
- **Composable Engines:** Tailor every view with one of four built-in interaction engines:
  - **`picker`**: Query-driven selectable lists with live previews and script-handled filtering.
  - **`capture`**: Read-only rich text and command output displays.
  - **`form`**: Interactive multi-field forms for parameter collection.
  - **`embedded`**: PTY terminal emulation hosting interactive programs (e.g. `btop`, subshells).
- **Terminal Graphics:** Render image previews using Kitty, Sixel, iTerm2, or Halfblocks protocols.
- **Pure Self-Containment:** Workflows operate independently without hardcoded cross-dependencies, making them reusable across different suites.
- **Granular Customization:** Fine-grained semantic style slots, themes, and declared or script-driven handlers.

---

## Quick Start

### 1. Download Pre-built Binary

For Linux x86_64, install via the published release script:

```bash
curl -fL https://github.com/xavialyra/tflow/releases/latest/download/install.sh | sh
```

Run the interactive setup wizard (or follow [Getting Started](docs/tutorials/getting-started.md)):

```bash
# Launch the built-in setup wizard directly from standard input
curl -fL https://github.com/xavialyra/tflow/releases/latest/download/init.toml | tflow -w -
```

### 2. Build From Source

Requires a standard Rust toolchain (Cargo) and Git:

```bash
git clone https://github.com/xavialyra/tflow.git
cd tflow
cargo build --release
install -Dm755 target/release/tflow ~/.local/bin/tflow
```

Install the example suite (or follow [Getting Started](docs/tutorials/getting-started.md)):

```bash
# Install the default suite (includes calculator, clipboard, app finder, and sys tools)
mkdir -p ~/.config/tflow
cp -r examples/default-suite/. ~/.config/tflow/
```

---

## Usage Scenarios

### 1. Launching Suites & Workflows

```bash
# Launch the default suite ($XDG_CONFIG_HOME/tflow/default.toml)
tflow

# Open a specific workflow entrypoint by member ID
tflow apps

# Launch via a suite-defined alias
tflow calc

# Pass structured arguments to a view parameter
tflow clip --content_type="image"
```

### 2. Interactive Pipeline Integration

`tflow` can read candidate items or entire workflow manifests directly from standard input:

```bash
# Pipe text items into a dmenu-style picker
printf '%s\n' "Alpha" "Beta" "Gamma" | tflow dmenu

# Feed a dynamic workflow definition directly from stdin
cat ./temporary-tool.toml | tflow -w -
```

### 3. Standalone Tools & Shebang Executables

Run any atomic workflow file directly with `-w`:

```bash
tflow -w ./my-tool/workflow.toml
```

You can turn any workflow file into a standalone, directly executable terminal utility by adding a shebang:

```toml
#!/usr/bin/env -S tflow -w
[workflow]
api = 1
name = "Disk Usage"
entrypoint = "main"

[views.main.engine]
type = "capture"

[views.main.engine.config.output]
producer = "script"
[views.main.engine.config.output.handler]
script = '''#!/usr/bin/env bash
printf '{"version": 1, "output": "%s"}\n' "$(df -h)"
'''
```

```bash
chmod +x ./disk-tool.toml
./disk-tool.toml
```

---

## Documentation

For complete guides, configuration references, and architecture overviews, see [`docs/`](docs/index.md):

- **[Tutorials](docs/tutorials/index.md)**
  - [Getting Started](docs/tutorials/getting-started.md)
  - [Building Your First Workflow](docs/tutorials/first-workflow.md)
- **[How-To Guides](docs/how-to/index.md)**
  - [Picker Views](docs/how-to/picker-views.md)
  - [Capture Views](docs/how-to/capture-views.md)
  - [Form Views](docs/how-to/form-views.md)
  - [Embedded Views](docs/how-to/embedded-views.md)
  - [Commands and Producer Scripts](docs/how-to/commands-and-producers.md)
  - [View Navigation and Popups](docs/how-to/view-navigation-and-popups.md)
  - [Custom Themes](docs/how-to/custom-themes.md)
- **[Reference Specifications](docs/reference/index.md)**
  - [CLI Reference](docs/reference/cli.md)
  - [Settings Specification (`settings.toml`)](docs/reference/settings-toml.md)
  - [Suite Manifest Specification (`<suite>.toml`)](docs/reference/suite-toml.md)
  - [Workflow Specification (`workflow.toml`)](docs/reference/workflow-toml.md)
  - [Picker Preview Documents & Protocols](docs/reference/picker-preview.md)
  - [Form Content and State](docs/reference/form.md)
  - [Producer Protocol](docs/reference/producer-protocol.md)
  - [Theme Specification](docs/reference/theme-toml.md)
- **[Architecture & Concepts](docs/explanation/index.md)**
  - [Architecture Overview](docs/explanation/architecture-overview.md)
  - [Input and Navigation Model](docs/explanation/input-and-navigation-model.md)
  - [Runtime Guarantees](docs/explanation/runtime-guarantees.md)

---

## License

This project is licensed under the [MIT License](LICENSE).
