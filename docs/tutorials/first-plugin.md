---
title: "Building Your First Plugin"
type: "tutorial"
tags:
  - plugins
  - scripts
  - views
  - commands
description: "Learn how to build a complete plugin with script execution, dynamic templates, and query parameters."
---

# Building Your First Plugin

In this tutorial, you will create a plugin called `notes` that displays note files from a directory and lets you open them in your editor.

## What You Will Learn

- How to structure a plugin directory.
- How to connect a shell script to generate picker items.
- How to define commands with dynamic `{{ selection.value }}` arguments.

## 1. Directory Structure

Create a dedicated plugin folder:

```bash
mkdir -p ~/.config/tui-launcher/plugins/notes/scripts
```

## 2. Write the List Script

Create `~/.config/tui-launcher/plugins/notes/scripts/list_notes.sh`:

```bash
#!/bin/sh
cat <<EOF
[
  {"label": "Project Ideas", "value": "ideas.txt", "description": "Future app thoughts"},
  {"label": "Meeting Notes", "value": "meeting.txt", "description": "Weekly sync"},
  {"label": "Shopping List", "value": "shopping.txt", "description": "Groceries"}
]
EOF
```

Make it executable:

```bash
chmod +x ~/.config/tui-launcher/plugins/notes/scripts/list_notes.sh
```

## 3. Define the Plugin Manifest

Create `~/.config/tui-launcher/plugins/notes/plugin.toml`:

```toml
[plugin]
api = 1
name = "Notes Manager"

[views.main]
alias = "notes"

[views.main.engine]
type = "picker"

[views.main.engine.config]
items = { source = "script", file = "scripts/list_notes.sh" }

[views.main.commands.open]
key = "enter"
label = "Edit Note"
type = "run"

[views.main.commands.open.payload]
handler = { source = "script", file = "scripts/open_note.sh" }
args = ["--file={{ selection.value }}"]
exit = true
```

## 4. Write the Handler Script

Create `~/.config/tui-launcher/plugins/notes/scripts/open_note.sh`:

```bash
#!/bin/sh
FILE="$1"
echo "Opening $FILE..."
# In practice: ${EDITOR:-vim} "$HOME/notes/$FILE"
```

Make it executable:

```bash
chmod +x ~/.config/tui-launcher/plugins/notes/scripts/open_note.sh
```

## 5. Test Directly from the Command Line

You can invoke any view directly using its `plugin:view` syntax or its `alias`:

```bash
tui-launcher notes:main
# or via alias:
tui-launcher notes
```

Press `Enter` on any note to test the script execution!

## Next Steps

- Check out [Dynamic Picker Feeds](../how-to/dynamic-picker-feeds.md) to see how to pass live query input to scripts.
- Check out [View Navigation & Popups](../how-to/view-navigation-and-popups.md) to open detail views in popup overlays.
