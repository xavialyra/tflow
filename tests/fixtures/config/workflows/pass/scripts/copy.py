#!/usr/bin/env python3
import json
import os
import shutil
import subprocess
import sys


def workflow_owner():
    """Member id this package is mounted as; the suite decides it, not us."""
    root = os.environ.get("TFLOW_WORKFLOW_DIR") or os.path.dirname(
        os.path.dirname(os.path.realpath(__file__))
    )
    root = os.path.realpath(root)
    try:
        import tomllib

        with open(os.environ["TFLOW_SUITE"], "rb") as handle:
            workflows = tomllib.load(handle).get("workflows") or {}
        base = os.path.dirname(os.path.abspath(os.environ["TFLOW_SUITE"]))
        for member, entry in workflows.items():
            target = (entry or {}).get("dir") or (entry or {}).get("file")
            if isinstance(target, str) and os.path.realpath(
                os.path.join(base, target)
            ) == root:
                return member
    except Exception:
        pass
    name = os.path.basename(root)
    if not name:
        raise SystemExit("cannot resolve the workflow owner; set TFLOW_WORKFLOW_DIR")
    return name


def selected_entry(request):
    state = request.get("context", {}).get("engine", {}).get("state", {})
    item = state.get("item") if isinstance(state, dict) else None
    entry = item.get("value") if isinstance(item, dict) else None
    if not isinstance(entry, str) or not entry:
        raise ValueError("select a password first")
    return entry


def find_passfile(entry):
    prefix = os.environ.get("PASSWORD_STORE_DIR", os.path.expanduser("~/.password-store"))
    candidates = [
        os.path.join(prefix, f"{entry}.gpg"),
        os.path.expanduser(f"~/.local/share/password-store/{entry}.gpg"),
    ]
    for c in candidates:
        if os.path.exists(c):
            return c
    return os.path.join(prefix, f"{entry}.gpg")


def copy_to_clipboard(text):
    clip_time = int(os.environ.get("PASSWORD_STORE_CLIP_TIME", "45"))
    if os.environ.get("WAYLAND_DISPLAY") and shutil.which("wl-copy"):
        subprocess.run(["wl-copy"], input=text.encode(), check=True)
        subprocess.Popen(
            ["bash", "-c", f"sleep {clip_time} && wl-copy --clear"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
    elif os.environ.get("DISPLAY") and shutil.which("xclip"):
        subprocess.run(["xclip", "-selection", "clipboard"], input=text.encode(), check=True)
        subprocess.Popen(
            ["bash", "-c", f"sleep {clip_time} && (pkill -f 'xclip -selection clipboard' 2>/dev/null || true)"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )


def main():
    entry = selected_entry(json.load(sys.stdin))
    passfile = find_passfile(entry)

    # Probe gpg-agent memory cache.
    # With --pinentry-mode error, gpg decrypts immediately if cached in gpg-agent,
    # or fails instantly without any GUI or terminal prompts if not cached.
    if os.path.exists(passfile):
        probe = subprocess.run(
            [
                "gpg",
                "--batch",
                "--quiet",
                "--yes",
                "--pinentry-mode",
                "error",
                "-d",
                passfile,
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        if probe.returncode == 0:
            lines = probe.stdout.splitlines()
            secret = lines[0] if lines else ""
            copy_to_clipboard(secret)
            json.dump(
                {
                    "version": 1,
                    "operation": {
                        "type": "run",
                        "mode": "foreground",
                        "argv": ["true"],
                        "exit": True,
                        "success_message": f"Password for {entry} copied",
                    },
                },
                sys.stdout,
                separators=(",", ":"),
            )
            sys.stdout.write("\n")
            return

    # Not cached: invoke the pass:unlock form popup
    json.dump(
        {
            "version": 1,
            "operation": {
                "type": "call",
                "target": f"{workflow_owner()}:unlock",
                "query": {"entry": entry},
                "presentation": {
                    "mode": "popup",
                    "width": 60,
                    "height": 7,
                },
            },
        },
        sys.stdout,
        separators=(",", ":"),
    )
    sys.stdout.write("\n")


if __name__ == "__main__":
    try:
        main()
    except (TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"pass: {error}", file=sys.stderr)
        raise SystemExit(2)
