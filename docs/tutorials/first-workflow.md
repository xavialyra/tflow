---
title: "Building Your First Workflow"
type: "tutorial"
tags:
  - workflow
  - producers
  - scripts
  - views
  - commands
description: "Build a Picker workflow with literal items and a JSON command producer."
---

# Building Your First Workflow

In this tutorial, you will create a workflow called `notes` with a Picker list and a command that opens the selected note. The item value travels to the command script through the producer request.

## What You Will Learn

- How to choose a single-file or directory workflow package.
- How to keep View and item configuration literal.
- How to read `context.engine.state.item` from a command producer request.
- How to return a typed foreground operation from a script.

## Option A: Single-file Workflow

Create `~/.config/tlaunch/workflows/notes.toml`:

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
  { display = "Project Ideas", value = "ideas.txt", metadata = {} },
  { display = "Meeting Notes", value = "meeting.txt", metadata = {} },
  { display = "Shopping List", value = "shopping.txt", metadata = {} },
]

[views.main.commands.open]
key = "enter"
label = "Edit note"
type = "run"
producer = "script"

[views.main.commands.open.handler]
script = '''#!/usr/bin/env python3
import json
import os
import sys

request = json.load(sys.stdin)
state = request["context"]["engine"]["state"]
item = state.get("item") if isinstance(state, dict) else None
path = item.get("value") if isinstance(item, dict) else None
if not isinstance(path, str):
    raise SystemExit("select a note first")
editor = os.environ.get("EDITOR", "vi")
json.dump({
    "version": 1,
    "operation": {
        "type": "run",
        "mode": "foreground",
        "argv": [editor, path],
        "exit": True,
    },
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
'''
```

Run validation and launch it:

```bash
tlaunch --check
tlaunch notes
```

The producer script is materialized by the host and receives JSON on stdin. Its stdout contains only the operation envelope. The editor runs with the caller's current working directory, so relative note paths resolve from the directory where you launch `tlaunch`.

## Option B: Directory Package Workflow

Use a package when the workflow has multiple scripts or assets:

```bash
mkdir -p ~/.config/tlaunch/workflows/notes/scripts
```

Create `~/.config/tlaunch/workflows/notes/scripts/list_notes.py`:

```python
#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
state = request["context"]["engine"]["state"]
needle = state.get("input", "").lower() if isinstance(state, dict) else ""
notes = [
    ("Project Ideas", "ideas.txt"),
    ("Meeting Notes", "meeting.txt"),
    ("Shopping List", "shopping.txt"),
]
items = [
    {"display": label, "value": path, "metadata": {}}
    for label, path in notes
    if needle in label.lower()
]
json.dump({"version": 1, "items": items}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
```

Create `~/.config/tlaunch/workflows/notes/scripts/open_note.py`:

```python
#!/usr/bin/env python3
import json
import os
import sys

request = json.load(sys.stdin)
state = request["context"]["engine"]["state"]
item = state.get("item") if isinstance(state, dict) else None
path = item.get("value") if isinstance(item, dict) else None
if not isinstance(path, str):
    raise SystemExit("select a note first")
editor = os.environ.get("EDITOR", "vi")
json.dump({
    "version": 1,
    "operation": {
        "type": "run",
        "mode": "foreground",
        "argv": [editor, path],
        "exit": True,
    },
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
```

Make both scripts executable:

```bash
chmod +x ~/.config/tlaunch/workflows/notes/scripts/list_notes.py
chmod +x ~/.config/tlaunch/workflows/notes/scripts/open_note.py
```

Create `~/.config/tlaunch/workflows/notes/workflow.toml`:

```toml
[workflow]
api = 1
name = "Notes Manager"

[views.main]
alias = "notes"

[views.main.engine]
type = "picker"

[views.main.engine.config.items]
producer = "script"

[views.main.engine.config.items.handler]
file = "scripts/list_notes.py"

[views.main.commands.open]
key = "enter"
label = "Edit note"
type = "run"
producer = "script"

[views.main.commands.open.handler]
file = "scripts/open_note.py"
```

Validate and run:

```bash
tlaunch --check
tlaunch notes:main
```

## Next Steps

- Read [workflow.toml Specification](../reference/workflow-toml.md) for all six operation types and strict protocol rules.
- Follow [Picker Views](../how-to/picker-views.md) for request filtering, feeds, and previews.
- Read [Commands and Producer Scripts](../how-to/commands-and-producers.md) for script context and typed responses.
- Read [Producer Protocol](../reference/producer-protocol.md) for the exact request and response contract.
