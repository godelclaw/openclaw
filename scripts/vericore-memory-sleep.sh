#!/usr/bin/env bash
set -euo pipefail
socket=/run/user/1001/vericore-core.sock
send() {
  local payload=$1
  local response
  response=$(printf '%s' "$payload" | socat - UNIX-CONNECT:"$socket")
  python3 - "$payload" "$response" <<'PY2'
import json, sys
payload = sys.argv[1]
response = sys.argv[2]
try:
    data = json.loads(response)
except Exception as err:
    print(f"vericore-memory-sleep: invalid response for {payload}: {err}: {response}", file=sys.stderr)
    raise SystemExit(1)
if not data.get("ok"):
    print(f"vericore-memory-sleep: daemon error for {payload}: {data.get('error')}", file=sys.stderr)
    raise SystemExit(1)
PY2
}
send '{"id":"1","method":"memory_refine","embed":true}'
send '{"id":"2","method":"memory_extract_history"}'
send '{"id":"3","method":"memory_refresh_midterm"}'
