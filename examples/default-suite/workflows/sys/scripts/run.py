#!/usr/bin/env python3
"""Execution runner for system session actions.

Executes the requested action using systemctl / loginctl or screenlockers,
and produces human-readable diagnostic output for the capture view.
"""
import json
import os
import shutil
import subprocess
import sys


def run_lock_command() -> tuple[int, str]:
    """Attempt session locking via loginctl or known screenlockers."""
    # Preferred: systemd logind session lock
    if shutil.which("loginctl"):
        res = subprocess.run(["loginctl", "lock-session"], capture_output=True, text=True)
        if res.returncode == 0:
            return 0, "Session locked via loginctl."

    # Fallbacks for specific environments
    lockers = [
        ["hyprlock"],
        ["swaylock"],
        ["waylock"],
        ["xdg-screensaver", "lock"],
        ["i3lock"],
    ]
    for cmd in lockers:
        if shutil.which(cmd[0]):
            res = subprocess.run(cmd, capture_output=True, text=True)
            if res.returncode == 0:
                return 0, f"Session locked via {cmd[0]}."

    return 1, "No compatible screen locker found (tried loginctl, hyprlock, swaylock, xdg-screensaver, i3lock)."


def execute_action(action: str) -> tuple[int, str]:
    if action == "lock":
        return run_lock_command()

    user = os.environ.get("USER", "")
    commands = {
        "suspend": ["systemctl", "suspend"],
        "logout": ["loginctl", "terminate-user", user],
        "reboot": ["systemctl", "reboot"],
        "poweroff": ["systemctl", "poweroff"],
    }

    cmd = commands.get(action)
    if not cmd or (action == "logout" and not user):
        return 1, f"Unknown or invalid system action: {action}"

    if not shutil.which(cmd[0]):
        return 1, f"Required command '{cmd[0]}' not found in PATH."

    res = subprocess.run(cmd, capture_output=True, text=True)
    msg = (res.stdout + res.stderr).strip()
    if res.returncode == 0:
        return 0, f"Action '{action}' executed successfully." + (f" ({msg})" if msg else "")
    return res.returncode, f"Failed to execute '{action}': {msg or f'exit status {res.returncode}'}"


def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        raise SystemExit("failed to parse JSON request")

    context = request.get("context", {})
    parameters = context.get("parameters", {})

    if isinstance(parameters, str):
        action = parameters
    elif isinstance(parameters, dict):
        action = parameters.get("action") or parameters.get("query")
    else:
        action = None

    if not action:
        output = "No action specified."
    else:
        code, msg = execute_action(str(action))
        output = f"[{'SUCCESS' if code == 0 else 'ERROR'}] {msg}"

    json.dump({"version": 1, "output": output + "\n"}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
