---
title: "Building Your First Workflow"
type: "tutorial"
tags:
  - workflow
  - scripts
  - views
  - commands
description: "Learn how to build a complete workflow with script execution, dynamic templates, and query parameters."
---

# Building Your First Workflow

In this tutorial, you will create a workflow called `notes` that displays note files from a directory and lets you open them in your editor.

## What You Will Learn

- How to structure single-file and directory workflows.
- How to write multi-line inline scripts.
- How to define commands with dynamic `{{ selection.value }}` arguments while preserving caller `$PWD`.

## Option A: Standalone Single-File Workflow

Single-file workflows live directly in `$XDG_CONFIG_HOME/tui-launcher/workflows/<id>.toml`:

Create `~/.config/tui-launcher/workflows/notes.toml`:

```toml
[workflow]
api = 1
name = "Notes Manager"

[views.main]
alias = "notes"

[views.main.engine]
type = "picker"

[views.main.engine.config]
items = [
  { display = "Project Ideas", value = "ideas.txt", description = "Future app thoughts" },
  { display = "Meeting Notes", value = "meeting.txt", description = "Weekly sync" },
  { display = "Shopping List", value = "shopping.txt", description = "Groceries" }
]

[views.main.commands.open]
key = "enter"
label = "Edit Note"
type = "run"
args = ["{{ selection.value }}"]
exit = true
script = """
#!/usr/bin/env -S sh -eu
target="$1"
${EDITOR:-vim} "$target"
"""
```

Test it immediately:

```bash
tui-launcher notes
```

Because caller working directory (`$PWD`) is preserved, your editor opens the target note relative to wherever you executed the command.

---

## Option B: Directory Package Workflow

For multi-script or complex extensions:

### 1. Directory Structure

Create a dedicated workflow folder:

```bash
mkdir -p ~/.config/tui-launcher/workflows/notes/scripts
```

### 2. Write the List Script

Create `~/.config/tui-launcher/workflows/notes/scripts/list_notes.sh`:

```bash
#!/bin/sh
cat << 'ITEMS_EOF'
[
  {"display": "Project Ideas", "value": "ideas.txt", "description": "Future app thoughts"},
  {"display": "Meeting Notes", "value": "meeting.txt", "description": "Weekly sync"},
  {"display": "Shopping List", "value": "shopping.txt", "description": "Groceries"}
]
ITEMS_EOF
```

Make it executable:

```bash
chmod +x ~/.config/tui-launcher/workflows/notes/scripts/list_notes.sh
```

### 3. Define the Workflow Manifest

Create `~/.config/tui-launcher/workflows/notes/workflow.toml`:

```toml
[workflow]
api = 1
name = "Notes Manager"

[views.main]
alias = "notes"

[views.main.engine]
type = "picker"

[views.main.engine.config.items]
source = "script"
file = "scripts/list_notes.sh"

[views.main.commands.open]
key = "enter"
label = "Open Note"
type = "run"

[views.main.commands.open.payload]
handler = { source = "script", file = "scripts/open_note.sh" }
args = ["{{ selection.value }}"]
exit = true
```

### 4. Create the Open Script

Create `~/.config/tui-launcher/workflows/notes/scripts/open_note.sh`:

```bash
#!/bin/sh
note_path="$1"
${EDITOR:-vim} "$note_path"
```

Make it executable:

```bash
chmod +x ~/.config/tui-launcher/workflows/notes/scripts/open_note.sh
```

### 5. Run the Workflow

Launch directly by alias or view identifier:

```bash
tui-launcher notes:main
# Or via alias:
tui-launcher notes
```
