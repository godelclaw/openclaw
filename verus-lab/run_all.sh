#!/usr/bin/env bash
set -euo pipefail

LAB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERUS_BIN="${VERUS_BIN:-/home/zarclaw/repos/verus/source/target-verus/release/verus}"

if [[ ! -x "$VERUS_BIN" ]]; then
  echo "ERROR: verus binary not found at: $VERUS_BIN" >&2
  echo "Build Verus first in /home/zarclaw/repos/verus/source:" >&2
  echo "  source ../tools/activate && vargo build --release" >&2
  exit 1
fi

status=0
for f in "$LAB_DIR"/lesson*.rs; do
  echo "==> verifying $(basename "$f")"
  if ! "$VERUS_BIN" "$f"; then
    status=1
  fi
  echo
done

if [[ "$status" -ne 0 ]]; then
  echo "One or more Verus lessons failed."
  exit "$status"
fi

echo "All Verus lessons verified successfully."
