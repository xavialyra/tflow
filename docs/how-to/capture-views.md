---
title: "How to Configure Capture Views"
type: "guide"
tags:
  - capture
  - views
  - producers
  - scripts
description: "Configure Capture Views for static text or one-shot script-produced output."
---

# How to Configure Capture Views

Use the `capture` Engine for text that should be rendered by the launcher after a View is mounted. Capture is one-shot output, not an interactive terminal; use [Embedded Views](embedded-views.md) for a live PTY program.

## Problem

You want to show fixed text or generate output with a script while keeping the launcher in control of the View lifecycle and error display.

## Solution

### 1. Show Static Output

Declare a Capture View with a literal `output` value:

```toml
[views.output.engine]
type = "capture"

[views.output.engine.config]
output = "System information is ready."
```

The value is literal configuration and is validated when the workflow is loaded.

### 2. Produce Output with a Script

Use the producer envelope when output depends on launch-time data:

```toml
[views.output.engine]
type = "capture"

[views.output.engine.config.output]
producer = "script"

[views.output.engine.config.output.handler]
file = "scripts/output.py"
```

A Capture output producer receives `entrypoint = "capture-output"` and the shared context:

```python
#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
parameters = request["context"]["parameters"]
json.dump({
    "version": 1,
    "output": "parameters=" + json.dumps(parameters),
}, sys.stdout, separators=(",", ":"))
sys.stdout.write("\n")
```

For Capture, `context.parameters` contains the View's bound parameters, `context.input` contains the explicit launch input descriptor, and `context.engine.state` is `null`. The response must contain one complete `output` string:

```json
{"version":1,"output":"System date output\n"}
```

Producer scripts write the protocol response to stdout and diagnostics to stderr. They do not write arbitrary terminal output.

### 3. Understand Lifecycle and Failure Handling

The output producer starts after the Capture View has mounted. While it runs, the View remains mounted and can show its loading state. A valid response replaces the displayed content. If the producer fails, the Capture View remains mounted and displays a diagnostic instead of changing the Router stack.

A Capture producer cannot change the target route, query schema, Engine type, keymap, or View configuration.

## Troubleshooting

- `output` is required for a Capture View.
- Return exactly one version-1 JSON object; extra stdout text invalidates the response.
- Use `tflow --check` to validate the producer handler before launch.
- Use [Producer Protocol](../reference/producer-protocol.md) for response validation, timeouts, cancellation, and output limits.
