# Companion Combination Examples

Each combination is an independent single-file workflow. Run a specific `.toml` file, not this directory (there is no shared `workflow.toml`).

## Requirements

- `tflow`, or `cargo run --` from the repository root.
- Python 3 for JSON producers.
- A POSIX `sh` and PTY support for Embedded examples.

## Examples

| File | Combination | What to try |
| --- | --- | --- |
| [picker_capture.toml](picker_capture.toml) | Picker + Capture | Change the service with Up/Down; details show its port and owner live. |
| [picker_picker.toml](picker_picker.toml) | Picker + Picker | Choose a category, enter its service list, choose a variant, and confirm. |
| [picker_form.toml](picker_form.toml) | Picker + Form | Select a job, press Alt-E to attach a snapshot editor, or Enter to edit in the foreground. |
| [picker_embedded.toml](picker_embedded.toml) | Picker + Embedded | Select an item, press Ctrl-P to start its tool. |
| [form_capture.toml](form_capture.toml) | Form + Capture | Edit the name, enum, and boolean; the right pane shows live draft values and validation state. |
| [capture_picker.toml](capture_picker.toml) | Capture + Picker | Keep a report visible, enter the action list with Enter, and choose an action. |
| [capture_embedded.toml](capture_embedded.toml) | Capture + Embedded | Read a report alongside an interactive follow-up shell. |
| [embedded_capture.toml](embedded_capture.toml) | Embedded + Capture | Type in the shell while a read-only reference remains attached. |

Run a workflow:

```sh
tflow --workflow tests/fixtures/companion-matrix/form_capture.toml
tflow --workflow tests/fixtures/companion-matrix/picker_picker.toml

# Without an installed tflow binary:
cargo run -- --workflow tests/fixtures/companion-matrix/picker_capture.toml
```

Validate all examples without launching them:

```sh
for file in tests/fixtures/companion-matrix/*.toml; do
  tflow --check --workflow "$file" || exit 1
done
```

## Cascading Picker Walkthrough

1. Run `picker_picker.toml`. The left list contains **Databases** and **Queues**.
2. Use Up/Down to select a category. The right list follows it (PostgreSQL/SQLite or RabbitMQ/Redis Streams).
3. Press Enter to push the corresponding service Picker.
4. Select a service and press Enter to push its variant Picker.
5. Select a variant and press Enter to display confirmation.
6. Press Escape to go back one level. Escape from the service list reveals the original category Picker and its companion.

The service companion hides its input row because its input contains binding JSON, not a search query. Only the root has an attached companion; deeper levels use ordinary navigation, so this example does not require nested companion attachments.

## Form Summary

`form_capture.toml` declares `companion = "summary"`. For a Form, the primary view's published state object containing `values`, `drafts`, `valid`, and `dirty` automatically synchronizes to the Capture companion. Its Capture producer renders that state on each update.

Static field declarations also require explicit `value` properties; query defaults alone do not initialize declared field editors. The example sets those values explicitly.

## Navigation and Snapshot Semantics

- Ctrl-P is explicitly bound in each root View to an ordinary workflow companion command; configuring `companion` alone does not bind it. It toggles that target and can be remapped or removed with `unbind.keys`. Picker + Form shares its Alt-E snapshot command with Ctrl-P.
- Escape uses the active engine's ordinary back/cancel behavior.
- Picker + Form uses an explicit Alt-E companion command. Changing the job does not overwrite drafts; toggle the editor off with Alt-E and reopen it to snapshot another job. In the foreground Form, Escape cancels back to the Picker; Enter submits the draft as JSON and ends this standalone workflow.
- An Embedded process starts with a snapshot of `TFLOW_INPUT`. Changing selection does not restart a mounted PTY. Toggle it off/on or enter a fresh foreground tool to use the latest input.
- The shell examples execute real commands in the caller's directory. They do not modify files automatically; use only shell commands you trust.

## Current Layout Boundary

The Host computes primary and companion rectangles for both rendering and Resize/PTY dimensions, and owns the Picker Omnibar. Form fields and Embedded input remain engine-local. These examples exercise common layouts; they do not cover every terminal size or image protocol.
