---
title: "How to Build Dynamic Picker Feeds and Previews"
type: "guide"
tags:
  - picker
  - preview
  - scripts
  - feed
description: "How to configure script-backed item feeds and dynamic preview panes in the picker engine."
---

# How to Build Dynamic Picker Feeds and Previews

The `picker` engine supports both static item lists and dynamically generated feeds backed by shell scripts.

## Problem

You want to display items that are computed on the fly (e.g., git branches, running containers, or file trees) and show detailed information for the currently selected item in a side preview pane.

## Solution

### 1. Configure the Script-backed Feed

Under `[views.<name>.engine.config]`, assign `items` to an object with `source = "script"`:

```toml
[views.branches.engine]
type = "picker"

[views.branches.engine.config]
items = { source = "script", file = "scripts/get_branches.sh" }
```

The script must write valid JSON array elements to stdout:

```bash
#!/bin/sh
# scripts/get_branches.sh
git for-each-ref --format='{"label": "%(refname:short)", "value": "%(refname:short)", "description": "%(subject)"}' refs/heads/
```

### 2. Configure a Live Preview Pane

To display contextual details when navigating the list, configure `preview`:

```toml
[views.branches.engine.config.preview]
source = "script"
file = "scripts/preview_branch.sh"
args = ["{{ selection.value }}"]
```

The preview script receives the selected branch name as an argument and outputs text to be rendered in the preview area:

```bash
#!/bin/sh
# scripts/preview_branch.sh
BRANCH="$1"
git log -n 5 --color=always "$BRANCH"
```

### 3. Toggle the Preview Pane

You can allow users to toggle the preview pane visibility using engine keymap actions:

```toml
[views.branches.keymap]
"ctrl+p" = "toggle_preview"
```

## Security & Resource Limits

Script execution within feeds is protected by the launcher runtime:
- Standard script execution timeout is **10 seconds**.
- Default `stdout` is capped at **1 MiB** (can be configured up to **64 MiB** for large feeds).
- Paths are strictly confined within the owning plugin's root directory.
