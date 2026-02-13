#!/usr/bin/env bash
# Check OpenRouter credit usage
if [ -f "$HOME/.openclaw/.env" ]; then
  set -a; . "$HOME/.openclaw/.env"; set +a
fi
data=$(curl -s https://openrouter.ai/api/v1/auth/key -H "Authorization: Bearer $OPENROUTER_API_KEY")
echo "$data" | python3 -c "
import sys, json
d = json.load(sys.stdin)['data']
print(f\"OpenRouter Usage Report\")
print(f\"─────────────────────\")
print(f\"  Today:  \${d['usage_daily']:.4f}\")
print(f\"  Week:   \${d['usage_weekly']:.4f}\")
print(f\"  Month:  \${d['usage_monthly']:.4f}\")
print(f\"  Total:  \${d['usage']:.4f}\")
limit = d.get('limit')
if limit:
    remaining = d.get('limit_remaining', 0)
    print(f\"  Limit:  \${limit:.2f}  (remaining: \${remaining:.4f})\")
else:
    print(f\"  Limit:  none set\")
"
