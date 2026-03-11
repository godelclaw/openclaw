#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR:-$ROOT/.BGIseed-state}"

CANONICAL_HEARTBEAT="${HEARTBEAT_CANONICAL:-$HOME/HEARTBEAT.md}"
STATE_WORKSPACE_DIR="$OPENCLAW_STATE_DIR/workspace"
MIRROR_HEARTBEAT="$STATE_WORKSPACE_DIR/HEARTBEAT.md"
CONFIG_PATH="${OPENCLAW_CONFIG_PATH:-$OPENCLAW_STATE_DIR/openclaw.json}"
PROMPT_EXPECTED='Heartbeat. Read HEARTBEAT.md and follow it calmly. If there is no clear call to action, reply HEARTBEAT_OK.'

if [[ ! -f "$CANONICAL_HEARTBEAT" ]]; then
  echo "heartbeat-sync: canonical HEARTBEAT.md missing at $CANONICAL_HEARTBEAT" >&2
  exit 1
fi

mkdir -p "$STATE_WORKSPACE_DIR"
install -m 0644 "$CANONICAL_HEARTBEAT" "$MIRROR_HEARTBEAT"

if [[ -f "$CONFIG_PATH" ]]; then
  python3 - "$CONFIG_PATH" "$PROMPT_EXPECTED" <<'PY'
import json
import sys
from pathlib import Path

config_path = Path(sys.argv[1])
prompt_expected = sys.argv[2]
cfg = json.loads(config_path.read_text())

agents = cfg.setdefault("agents", {})
defaults = agents.setdefault("defaults", {})
heartbeat = defaults.setdefault("heartbeat", {})
current_prompt = str(heartbeat.get("prompt", "") or "")
effective_prompt = current_prompt.strip()

uses_file_reference = "heartbeat.md" in effective_prompt.lower()
mentions_legacy_gas = "gas.md" in effective_prompt.lower()

if not uses_file_reference or mentions_legacy_gas:
    heartbeat["prompt"] = prompt_expected
    config_path.write_text(json.dumps(cfg, indent=2, ensure_ascii=False) + "\n")
PY
fi

echo "heartbeat-sync: canonical mirrored to $MIRROR_HEARTBEAT"
