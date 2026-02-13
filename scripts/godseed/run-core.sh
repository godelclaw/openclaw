#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
export OPENCLAW_STATE_DIR="$ROOT/.godseed-state"
export OPENCLAW_SKIP_GMAIL_WATCHER=1

if [ -s "$HOME/.nvm/nvm.sh" ]; then
  export NVM_DIR="$HOME/.nvm"
  . "$NVM_DIR/nvm.sh"
  nvm use 22 >/dev/null
fi

# Load local secrets (OPENROUTER_API_KEY, TELEGRAM_BOT_TOKEN, etc.) if present.
if [ -f "$HOME/.openclaw/.env" ]; then
  set -a
  # shellcheck disable=SC1091
  . "$HOME/.openclaw/.env"
  set +a
fi

cd "$ROOT"
exec pnpm openclaw gateway run --bind loopback --port 18789 --force "$@"
