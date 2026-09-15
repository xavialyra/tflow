#!/usr/bin/env python3
import json
import os
import subprocess
import sys


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
    if os.environ.get("WAYLAND_DISPLAY"):
        subprocess.run(["wl-copy"], input=text.encode(), check=True)
        subprocess.Popen(
            ["bash", "-c", f"sleep {clip_time} && wl-copy --clear"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
    elif os.environ.get("DISPLAY"):
        subprocess.run(["xclip", "-selection", "clipboard"], input=text.encode(), check=True)
        subprocess.Popen(
            ["bash", "-c", f"sleep {clip_time} && (pkill -f 'xclip -selection clipboard' 2>/dev/null || true)"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )


def main():
    req = json.load(sys.stdin)
    context = req.get("context", {})
    parameters = context.get("parameters", {})
    if not isinstance(parameters, dict):
        parameters = {}
    query = context.get("query", {})
    if not isinstance(query, dict):
        query = {}
    entry = parameters.get("entry") or query.get("entry", "")

    state = context.get("engine", {}).get("state", {})
    values = state.get("values", {}) if isinstance(state, dict) else {}
    passphrase = values.get("passphrase", "")

    passfile = find_passfile(entry)
    if not os.path.exists(passfile):
        print(f"pass: file not found: {passfile}", file=sys.stderr)
        raise SystemExit(1)

    res = subprocess.run(
        [
            "gpg",
            "--batch",
            "--yes",
            "--quiet",
            "--pinentry-mode",
            "loopback",
            "--passphrase",
            passphrase,
            "-d",
            passfile,
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )

    if res.returncode != 0:
        err = res.stderr.strip() or "Bad passphrase"
        if "Bad passphrase" in err or "decryption failed" in err:
            print("Bad passphrase. Please try again.", file=sys.stderr)
        else:
            print(f"Decryption failed: {err}", file=sys.stderr)
        raise SystemExit(1)

    lines = res.stdout.splitlines()
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


if __name__ == "__main__":
    main()
