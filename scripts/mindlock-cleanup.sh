#!/usr/bin/env bash
# Deletes rejected mindlock artifacts older than 30 days.
# Intended to run daily at hour 23 via systemd timer.
set -euo pipefail

MINDLOCK_DIR="${MINDLOCK_DIR:-/home/zarclaw/mindlock}"
REJECTED_DIR="$MINDLOCK_DIR/rejected"
RETENTION_DAYS=30

if [ ! -d "$REJECTED_DIR" ]; then
    exit 0
fi

# Delete artifact files and their meta sidecars older than retention period
find "$REJECTED_DIR" -type f -mtime "+$RETENTION_DAYS" -delete
