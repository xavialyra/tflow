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


def get_system_info() -> str:
    """Generate a rich ANSI-colored system diagnostic report."""
    import platform

    def color(code: str, text: str) -> str:
        return f"\033[{code}m{text}\033[0m"

    lines = []
    lines.append(color("1;36", "  __  __ _               "))
    lines.append(color("1;36", " | |_/ _| |_____ __ __   ") + color("1;32", "● SYSTEM DIAGNOSTICS"))
    lines.append(color("1;36", " |  _|  _| / _ \\ V  V /  ") + color("90", f"Host: {platform.node()}"))
    lines.append(color("1;36", "  \\__|_| |_\\___/\\_/\\_/   ") + color("90", f"Kernel: {platform.release()} ({platform.machine()})"))
    lines.append("")
    lines.append(color("1;33", "━" * 50))
    lines.append(color("1;37", " ❖ SYSTEM OVERVIEW"))
    lines.append(color("1;33", "━" * 50))

    user = os.environ.get("USER", "unknown")
    shell = os.environ.get("SHELL", "unknown")
    desktop = os.environ.get("XDG_CURRENT_DESKTOP", os.environ.get("DESKTOP_SESSION", "terminal/cli"))

    lines.append(f"  {color('1;34', 'User:')}        {user}")
    lines.append(f"  {color('1;34', 'Desktop:')}     {desktop}")
    lines.append(f"  {color('1;34', 'Shell:')}       {shell}")
    lines.append(f"  {color('1;34', 'Python:')}      {platform.python_version()}")

    # Uptime
    uptime = "unknown"
    try:
        with open("/proc/uptime", "r", encoding="ascii") as f:
            sec = float(f.readline().split()[0])
            days = int(sec // 86400)
            hours = int((sec % 86400) // 3600)
            mins = int((sec % 3600) // 60)
            uptime = f"{days}d {hours}h {mins}m"
    except Exception:
        pass
    lines.append(f"  {color('1;34', 'Uptime:')}      {uptime}")

    lines.append("")
    lines.append(color("1;33", "━" * 50))
    lines.append(color("1;37", " ❖ MEMORY UTILIZATION"))
    lines.append(color("1;33", "━" * 50))

    try:
        meminfo = {}
        with open("/proc/meminfo", "r", encoding="ascii") as f:
            for line in f:
                parts = line.split(":")
                if len(parts) == 2:
                    meminfo[parts[0].strip()] = int(parts[1].split()[0])
        total = meminfo.get("MemTotal", 0) / 1024
        avail = meminfo.get("MemAvailable", 0) / 1024
        used = total - avail
        pct = (used / total * 100) if total > 0 else 0

        bar_len = 24
        filled = int(bar_len * (pct / 100))
        bar_color = "1;32" if pct < 70 else ("1;33" if pct < 85 else "1;31")
        bar = color(bar_color, "■" * filled) + color("90", "□" * (bar_len - filled))

        lines.append(f"  RAM Usage:   [{bar}] {pct:.1f}%")
        lines.append(f"  Total:       {total:.1f} MB")
        lines.append(f"  Used:        {used:.1f} MB")
        lines.append(f"  Available:   {avail:.1f} MB")
    except Exception as e:
        lines.append(f"  {color('90', f'Unable to read memory metrics: {e}')}")

    lines.append("")
    lines.append(color("1;33", "━" * 50))
    lines.append(color("1;37", " ❖ STORAGE OVERVIEW"))
    lines.append(color("1;33", "━" * 50))

    try:
        st = shutil.disk_usage("/")
        d_total = st.total / (1024 ** 3)
        d_used = st.used / (1024 ** 3)
        d_free = st.free / (1024 ** 3)
        d_pct = (st.used / st.total * 100) if st.total > 0 else 0

        bar_len = 24
        filled = int(bar_len * (d_pct / 100))
        bar_color = "1;32" if d_pct < 75 else ("1;33" if d_pct < 90 else "1;31")
        bar = color(bar_color, "■" * filled) + color("90", "□" * (bar_len - filled))

        lines.append(f"  Root (/):    [{bar}] {d_pct:.1f}%")
        lines.append(f"  Total:       {d_total:.1f} GB")
        lines.append(f"  Used:        {d_used:.1f} GB")
        lines.append(f"  Free:        {d_free:.1f} GB")
    except Exception as e:
        lines.append(f"  {color('90', f'Unable to read storage metrics: {e}')}")

    lines.append("")

    return "\n".join(lines)


def execute_action(action: str) -> tuple[int, str]:
    if action == "info":
        return 0, get_system_info()
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
        output = "\033[1;33m[NOTICE]\033[0m No action specified."
    elif str(action) == "info":
        code, output = execute_action("info")
    else:
        code, msg = execute_action(str(action))
        badge = "\033[1;32m[SUCCESS]\033[0m" if code == 0 else "\033[1;31m[ERROR]\033[0m"
        output = f"{badge} {msg}"

    json.dump({"version": 1, "output": output + "\n"}, sys.stdout, separators=(",", ":"))
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
