#!/bin/sh
command -v python3 >/dev/null 2>&1 || {
  printf '%s\n' 'dmenu plugin requires python3' >&2
  exit 127
}
exec python3 scripts/dmenu.py items "$@"
