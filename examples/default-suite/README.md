# Default Example Suite

This directory contains the default suite configuration and member workflows for `tflow`.

---

## Workflows Overview

| Workflow | Entrypoint | Shorthand Alias | Description |
| :--- | :--- | :--- | :--- |
| **`core`** | `core:main` | *(suite entry)* | Unified aggregate hub querying apps, calculator, and system tools with live route completion. |
| **`apps`** | `apps:main` | `app` | XDG desktop application launcher with icon preview, keyword search, launch frequency ranking, and weight configuration form. |
| **`calculator`** | `calculator:main` | `calc` | Real-time mathematical expression evaluator with multi-representation preview (decimal, hex, binary, octal) and clipboard copy. |
| **`sys`** | `sys:main` | `sys` | System session controls (lock, sleep, logout, reboot, poweroff) with safety confirmation prompts for destructive actions. |
| **`clipboard`** | `clipboard:default`| `clip` | Searchable clipboard history powered by `cliphist` with image/text previews and cross-platform restoration. |
| **`shell`** | `shell:main` | `shell` | Embedded terminal running user's default `$SHELL` inside a PTY view. |
| **`dmenu`** | `dmenu:main` | `dmenu` | Standard dmenu replacement for shell pipelines with index and projection support. |

---

## Keyboard Controls & Bindings

### Global & View Controls
- `↑` / `↓`: Navigate list items.
- `Escape`: Go back to the previous view or close popup.
- `Ctrl+P`: Toggle the preview pane (where available).
- `Ctrl+K`: Open the command palette.
- `Ctrl+C` / `Ctrl+D`: Exit `tflow`.

### Workflow-Specific Shortcuts
- **`core`**:
  - `Tab`: Complete route alias or open completion popup.
  - `Space`: Route separator (e.g. typing `calc ` jumps to calculator).
- **`apps`**:
  - `Enter`: Launch application.
  - `Ctrl+O`: Launch detached (does not wait for activation bus).
  - `Ctrl+W`: Open weight & pin configuration form (popup).
  - `Ctrl+Y`: Copy `.desktop` file path to clipboard.
- **`calculator`**:
  - `Enter`: Copy evaluation result to clipboard.
  - `Ctrl+P`: Toggle detailed base conversions (Hex, Bin, Oct).
- **`sys`**:
  - `Enter`: Execute action (prompts confirmation for Reboot and Shut down).

---

## Dependencies & Environment Support

- **Core & Apps**: Python 3.10+ (standard on modern Linux).
- **Clipboard copying**: Automatically detects and uses `wl-copy` (Wayland), `xclip` / `xsel` (X11), or terminal OSC 52 sequences.
- **Clipboard history**: Requires `cliphist` and `wl-paste` (Wayland) or compatible provider.
- **Desktop Launching**: `setsid` and `gio` (standard XDG tools).
- **System Controls**: `systemctl` and `loginctl` (systemd), with fallback screen lockers (`swaylock`, `hyprlock`, `waylock`, `xdg-screensaver`, `i3lock`).
