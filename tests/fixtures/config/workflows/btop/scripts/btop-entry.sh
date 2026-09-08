#!/bin/sh
set -eu

exec sh "$WORKFLOW_DIR/scripts/btop.sh" "${LAUNCHER_INPUT:-cpu}"
