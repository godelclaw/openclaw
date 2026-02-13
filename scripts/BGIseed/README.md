# Godseed Bring-Up

State/config is isolated under `.BGIseed-state/` in the repo root.

## One-time toolchain

```bash
# Install nvm + node 22 + pnpm
export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] || curl -fsSL https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash
. "$NVM_DIR/nvm.sh"
nvm install 22
nvm alias default 22
corepack enable
corepack prepare pnpm@10.23.0 --activate
```

## Install deps

```bash
pnpm install
```

## Run

1. Core startup check (no channels):
   `scripts/BGIseed/run-core-skip-channels.sh`
2. Full startup with configured channels:
   `scripts/BGIseed/run-core.sh`

## Notes

- Token pressure is reduced via small bootstrap files, reduced history limits, no bundled skills, disabled control UI/browser/canvas.
- Secrets are loaded from `~/.openclaw/.env` (OPENROUTER_API_KEY, TELEGRAM_BOT_TOKEN, etc.)
- Gateway auth token is configured in `.BGIseed-state/openclaw.json`.
