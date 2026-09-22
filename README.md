# tflow

## Overview

`tflow` is a programmable framework for building interactive terminal workflows. It provides the structure for defining interfaces, commands, data flow, and user interaction; a launcher is only one of the applications that can be built with it.

> [!WARNING]
> `tflow` is currently in alpha. The workflow and producer protocols are not stable yet and may change without backwards compatibility. Suggestions, use cases, and bug reports are welcome in the [GitHub issue tracker](https://github.com/xavialyra/tflow/issues).
>
> `tflow` currently supports Linux only.

## Features

- **Pipeline friendly:** use workflows as interactive steps between ordinary commands.
- **Highly customizable:** define the interface, parameters, commands, scripts, navigation, themes, and output behavior.
- **Multiple workflow engines:** choose the interaction model that fits each part of a workflow, from selectable lists and forms to captured output and embedded terminal programs.
- **Terminal image protocols:** render image previews through Halfblocks, Kitty, Sixel, or iTerm2 protocols.
- **Composable:** combine views and actions into reusable or one-off workflows.
- **Easy to embed:** adapt the same framework to many command line tools and workflows without writing a separate application each time.

## Quick Start

### Download a GitHub build

For Linux x86_64, run the installer published with the latest GitHub Release:

```bash
curl -fL https://github.com/xavialyra/tflow/releases/latest/download/install.sh | sh
```

Optional example setup:

```bash
curl -fL https://github.com/xavialyra/tflow/releases/latest/download/init.toml | tflow -w -
```

### Build from source

You need Git and a Rust toolchain with Cargo.

```bash
git clone https://github.com/xavialyra/tflow.git
cd tflow
cargo build --release
install -Dm755 target/release/tflow ~/.local/bin/tflow
```

Optional example setup:

```bash
mkdir -p ~/.config/tflow
cp -r examples/default-suite/. ~/.config/tflow/
```

## Usage examples

### Run a workflow

```bash
# Start the configured default workflow
tflow

# Open a specific workflow view
tflow app

# Pass a value to a view parameter
tflow clip --content_type="image"

# Use tflow as a dmenu-style launcher
printf '%s\n' "first" "second" | tflow dmenu
```

For a standalone workflow, use `-w`:

```bash
tflow -w ./my-workflow.toml
```

A workflow can also be read from standard input with `-w -`, which makes it possible to create a one-off interactive step directly in a pipeline:

```bash
cat ./my-workflow.toml | tflow -w -
```

You can make a workflow file executable with a shebang and run it directly:

```toml
#!/usr/bin/env -S tflow -w
[workflow]
api = 1
name = "one-off-workflow"
entrypoint = "main"
```

```bash
chmod +x ./one-off-workflow.toml
./one-off-workflow.toml
```

This allows a workflow to be used as a temporary, self-contained terminal tool without installing it into a suite.

## Documentation

The documentation is organized using the Diátaxis framework and structured as an Open Knowledge Format (OKF v0.2) bundle under [`docs/`](docs/index.md). See the [full project documentation](https://github.com/xavialyra/tflow/tree/main/docs) for the complete guide set.

- **[Tutorials](docs/tutorials/index.md)**
  - [Getting Started](docs/tutorials/getting-started.md)
  - [Building Your First Workflow](docs/tutorials/first-workflow.md)
- **[How-To Guides](docs/how-to/index.md)**
  - [Picker Views](docs/how-to/picker-views.md)
  - [Capture Views](docs/how-to/capture-views.md)
  - [Embedded Views](docs/how-to/embedded-views.md)
  - [Commands and Producer Scripts](docs/how-to/commands-and-producers.md)
  - [View Navigation and Popups](docs/how-to/view-navigation-and-popups.md)
  - [Custom Themes](docs/how-to/custom-themes.md)
- **[Reference](docs/reference/index.md)**
  - [CLI Reference](docs/reference/cli.md)
  - [Settings and Suite Specification](docs/reference/settings-toml.md)
  - [Workflow Specification](docs/reference/workflow-toml.md)
  - [Producer Protocol](docs/reference/producer-protocol.md)
- **[Architecture and Concepts](docs/explanation/index.md)**
  - [Architecture Overview](docs/explanation/architecture-overview.md)
  - [Input and Navigation Model](docs/explanation/input-and-navigation-model.md)
  - [Runtime Guarantees](docs/explanation/runtime-guarantees.md)

## License

This project is licensed under the MIT License.
