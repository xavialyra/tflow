#!/bin/sh
set -eu

exec sh "$TFLOW_WORKFLOW_DIR/scripts/btop.sh" "${TFLOW_INPUT:-cpu}"
