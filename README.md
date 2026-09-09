# tui-launcher

`tui-launcher` is an extensible terminal workflow host. It coordinates workflow-owned **Views** powered by `picker`, `capture`, or `embedded` (PTY) engines, enabling keyboard-driven navigation, command execution, and interactive terminal workflows.

## Quick Start

### Build from Source

```bash
cargo build --release
# The binary is available at ./target/release/tui-launcher
```

### Usage

```bash
# Validate your configuration without opening the TUI
tui-launcher --check

# Start the launcher using the configured default view
tui-launcher

# Open a specific view directly
tui-launcher apps:main

# Invoke a view with structured query arguments
tui-launcher dmenu:main --index=true --prompt="Select:"
```

---

## Documentation

Full documentation is organized using the **Diátaxis** framework and structured as an **Open Knowledge Format (OKF v0.2)** bundle under [`docs/`](docs/index.md):

- **[Tutorials](docs/tutorials/index.md)**
  - [Getting Started](docs/tutorials/getting-started.md)
  - [Building Your First Workflow](docs/tutorials/first-workflow.md)
- **[How-To Guides](docs/how-to/index.md)**
  - [Picker Views](docs/how-to/picker-views.md)
  - [Capture Views](docs/how-to/capture-views.md)
  - [Embedded Views](docs/how-to/embedded-views.md)
  - [Commands and Producer Scripts](docs/how-to/commands-and-producers.md)
  - [View Navigation & Popups](docs/how-to/view-navigation-and-popups.md)
  - [Custom Themes](docs/how-to/custom-themes.md)
- **[Technical Reference](docs/reference/index.md)**
  - [CLI Reference](docs/reference/cli.md)
  - [config.toml Specification](docs/reference/config-toml.md)
  - [workflow.toml Specification](docs/reference/workflow-toml.md)
  - [Producer Protocol](docs/reference/producer-protocol.md)
- **[Architecture & Concepts (Explanation)](docs/explanation/index.md)**
  - [Architecture Overview & Dependency Rules](docs/explanation/architecture-overview.md)
  - [Input & Navigation Model](docs/explanation/input-and-navigation-model.md)
  - [Runtime Guarantees & Safety](docs/explanation/runtime-guarantees.md)

---

## License

This project is licensed under the MIT License.
