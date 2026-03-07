#!/usr/bin/env bash
# testbus.sh — Local E2E testing harness for VeriCore completions bridge
# Usage: testbus.sh <mode> [--content <text>] [--timeout <secs>] [--route <pref>]
#   mode: "bridge" | "vericore"
#
# Bridge mode:  sends directly to completions bridge socket (model resolution + LLM)
# VeriCore mode: sends stimulus to VeriCore daemon socket (full driver loop + bridge)
#
# Identity: channel=internal, actor=claude-code — no spoofing of telegram_dm or user IDs.
# Routing: --route driver (default) exercises the full GatewayLlmClient → bridge path.
#
# v1: main-agent-only. Uses resolveOpenClawAgentDir() which resolves to the
#     primary agent directory. Multi-agent derivation from sessionKey is a v2 concern.
#
# Audit log: ~/shared/testbus/audit.jsonl

set -euo pipefail

BRIDGE_SOCKET="/run/user/$(id -u)/openclaw-completions.sock"
VERICORE_SOCKET="/run/user/$(id -u)/vericore-core.sock"
AUDIT_LOG="$HOME/shared/testbus/audit.jsonl"
NODE_BIN="$HOME/.nvm/versions/node/v22.22.0/bin/node"

# Defaults
MODE=""
CONTENT="testbus ping"
TIMEOUT=120
REQUEST_ID="tb-$(date +%s)-$$"
ACTOR="claude-code"
CHANNEL="internal"
SESSION_KEY="testbus:test"
ROUTE_PREF="driver"

usage() {
  echo "Usage: $0 <bridge|vericore> [options]"
  echo ""
  echo "Options:"
  echo "  --content <text>      Message content (default: 'testbus ping')"
  echo "  --timeout <secs>      Timeout in seconds (default: 120)"
  echo "  --id <request-id>     Custom request ID (default: auto-generated)"
  echo "  --actor <name>        Actor identity (default: claude-code)"
  echo "  --channel <ch>        Channel for vericore mode (default: internal)"
  echo "  --session <key>       Session key (default: testbus:test)"
  echo "  --route <pref>        Route preference: driver|fallback|gate|off (default: driver)"
  exit 1
}

# Parse args
[[ $# -lt 1 ]] && usage
MODE="$1"; shift

while [[ $# -gt 0 ]]; do
  case "$1" in
    --content)  CONTENT="$2"; shift 2 ;;
    --timeout)  TIMEOUT="$2"; shift 2 ;;
    --id)       REQUEST_ID="$2"; shift 2 ;;
    --actor)    ACTOR="$2"; shift 2 ;;
    --channel)  CHANNEL="$2"; shift 2 ;;
    --session)  SESSION_KEY="$2"; shift 2 ;;
    --route)    ROUTE_PREF="$2"; shift 2 ;;
    *)          echo "Unknown arg: $1"; usage ;;
  esac
done

[[ "$MODE" != "bridge" && "$MODE" != "vericore" ]] && { echo "Mode must be 'bridge' or 'vericore'"; usage; }

# Ensure audit directory exists
mkdir -p "$(dirname "$AUDIT_LOG")"

TS_START=$(date -u +%Y-%m-%dT%H:%M:%SZ)
START_MS=$(($(date +%s%N) / 1000000))

json_escape_content() {
  printf '%s' "$1" | "$NODE_BIN" -e 'let d="";process.stdin.on("data",c=>d+=c);process.stdin.on("end",()=>process.stdout.write(JSON.stringify(d)))'
}

if [[ "$MODE" == "bridge" ]]; then
  # ── Bridge mode: length-prefixed JSON to completions socket ──
  [[ ! -S "$BRIDGE_SOCKET" ]] && { echo "ERROR: bridge socket not found at $BRIDGE_SOCKET"; exit 1; }

  ESCAPED_CONTENT=$(json_escape_content "$CONTENT")

  PAYLOAD="{\"request_id\":\"$REQUEST_ID\",\"session_key\":\"$SESSION_KEY\",\"call_kind\":\"driver_turn\",\"prompt_mode\":\"driver\",\"messages\":[{\"role\":\"user\",\"content\":$ESCAPED_CONTENT}],\"max_tokens\":1024,\"temperature\":0.7}"

  # Send via Node.js (handles 4-byte length prefix + clears timeout on response)
  RESPONSE=$("$NODE_BIN" -e '
    const net = require("net");
    const payload = Buffer.from(process.argv[1]);
    const header = Buffer.alloc(4);
    header.writeUInt32BE(payload.length, 0);
    const timeoutMs = parseInt(process.argv[3]) * 1000;
    const timer = setTimeout(() => {
      process.stdout.write("{\"ok\":false,\"error\":\"timeout\"}");
      process.exit(1);
    }, timeoutMs);
    const sock = net.createConnection(process.argv[2], () => {
      sock.write(Buffer.concat([header, payload]));
      sock.end();
    });
    let resp = Buffer.alloc(0);
    sock.on("data", c => resp = Buffer.concat([resp, c]));
    sock.on("end", () => {
      clearTimeout(timer);
      if (resp.length >= 4) {
        const len = resp.readUInt32BE(0);
        process.stdout.write(resp.subarray(4, 4 + len).toString("utf-8"));
      } else {
        process.stdout.write("{\"ok\":false,\"error\":\"empty response\"}");
      }
    });
    sock.on("error", e => {
      clearTimeout(timer);
      process.stdout.write(JSON.stringify({ok: false, error: e.message}));
    });
  ' "$PAYLOAD" "$BRIDGE_SOCKET" "$TIMEOUT" 2>/dev/null)

elif [[ "$MODE" == "vericore" ]]; then
  # ── VeriCore mode: plain JSON to daemon socket ──
  # Uses route_preference to control whether VeriCore's driver loop (and thus the
  # completions bridge) is exercised. Default: "driver" for full path testing.
  [[ ! -S "$VERICORE_SOCKET" ]] && { echo "ERROR: vericore socket not found at $VERICORE_SOCKET"; exit 1; }

  EPOCH=$(date +%s)
  ESCAPED_CONTENT=$(json_escape_content "$CONTENT")

  PAYLOAD="{\"method\":\"run\",\"stimulus\":{\"channel\":\"$CHANNEL\",\"actor\":\"$ACTOR\",\"content\":$ESCAPED_CONTENT,\"timestamp\":$EPOCH,\"session_key\":\"$SESSION_KEY\",\"route_preference\":\"$ROUTE_PREF\"}}"

  # Send via socat with timeout
  RESPONSE=$(echo "$PAYLOAD" | timeout "$TIMEOUT" socat -t "$TIMEOUT" - UNIX-CONNECT:"$VERICORE_SOCKET" 2>/dev/null || echo '{"ok":false,"error":"socat failed or timeout"}')
fi

END_MS=$(($(date +%s%N) / 1000000))
DURATION_MS=$((END_MS - START_MS))

# ── Parse and display results ──
echo ""
echo "═══ Testbus Result ═══"
echo "  Mode:     $MODE"
echo "  ID:       $REQUEST_ID"
echo "  Duration: ${DURATION_MS}ms"
echo ""

parse_field() {
  "$NODE_BIN" -e "try{const r=JSON.parse(process.argv[1]);const v=$2;console.log(v??'?')}catch{console.log('parse_error')}" "$RESPONSE" 2>/dev/null
}

if [[ "$MODE" == "bridge" ]]; then
  OK=$(parse_field "$RESPONSE" "r.ok")
  MODEL=$(parse_field "$RESPONSE" "r.resolved_model")
  PROVIDER=$(parse_field "$RESPONSE" "r.resolved_provider")
  PROFILE=$(parse_field "$RESPONSE" "r.resolved_profile")
  FALLBACK=$(parse_field "$RESPONSE" "r.fallback_used")
  CONTENT_PREVIEW=$(parse_field "$RESPONSE" "r.choices?.[0]?.message?.content?.substring(0,200)")
  ERROR=$(parse_field "$RESPONSE" "r.error||''")

  echo "  ok:                $OK"
  [[ -n "$ERROR" && "$ERROR" != "" ]] && echo "  error:             $ERROR"
  echo "  resolved_model:    $MODEL"
  echo "  resolved_provider: $PROVIDER"
  echo "  resolved_profile:  $PROFILE"
  echo "  fallback_used:     $FALLBACK"
  echo ""
  echo "  Response preview:"
  echo "  $CONTENT_PREVIEW"

  STATUS="$OK"
elif [[ "$MODE" == "vericore" ]]; then
  OK=$(parse_field "$RESPONSE" "r.ok")
  ALLOW=$(parse_field "$RESPONSE" "r.result?.decision?.allow")
  ROUTE=$(parse_field "$RESPONSE" "r.result?.route?.route||'n/a'")
  RESP_PREVIEW=$(parse_field "$RESPONSE" "r.result?.outcome?.response?.substring(0,200)")
  PROMPT_TOKENS=$(parse_field "$RESPONSE" "r.result?.outcome?.prompt_tokens||0")
  COMPLETION_TOKENS=$(parse_field "$RESPONSE" "r.result?.outcome?.completion_tokens||0")
  ERROR=$(parse_field "$RESPONSE" "r.error||''")

  echo "  ok:              $OK"
  [[ -n "$ERROR" && "$ERROR" != "" ]] && echo "  error:           $ERROR"
  echo "  decision.allow:  $ALLOW"
  echo "  route:           $ROUTE"
  echo "  tokens:          ${PROMPT_TOKENS}in / ${COMPLETION_TOKENS}out"
  echo ""
  echo "  Response preview:"
  echo "  $RESP_PREVIEW"

  STATUS="$OK"
fi

echo ""

# ── Audit log ──
AUDIT_ENTRY=$("$NODE_BIN" -e "
  const entry = {
    id: process.argv[1],
    ts: process.argv[2],
    mode: process.argv[3],
    from: process.argv[4],
    duration_ms: parseInt(process.argv[5]),
    status: process.argv[6],
  };
  try {
    const r = JSON.parse(process.argv[7]);
    if (process.argv[3] === 'bridge') {
      entry.resolved_model = r.resolved_model;
      entry.resolved_provider = r.resolved_provider;
      entry.resolved_profile = r.resolved_profile;
      entry.fallback_used = r.fallback_used;
    } else {
      entry.decision_allow = r.result?.decision?.allow;
      entry.route = r.result?.route?.route;
      entry.prompt_tokens = r.result?.outcome?.prompt_tokens;
      entry.completion_tokens = r.result?.outcome?.completion_tokens;
    }
  } catch {}
  console.log(JSON.stringify(entry));
" "$REQUEST_ID" "$TS_START" "$MODE" "$ACTOR" "$DURATION_MS" "$STATUS" "$RESPONSE" 2>/dev/null)

echo "$AUDIT_ENTRY" >> "$AUDIT_LOG"
echo "  Audit logged to: $AUDIT_LOG"
