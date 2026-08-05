#!/bin/sh

if [ -n "${LAUNCHER_LOG_FILE:-}" ] && [ -r "$LAUNCHER_LOG_FILE" ]; then
    cat "$LAUNCHER_LOG_FILE"
fi
