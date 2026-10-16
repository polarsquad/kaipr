#!/usr/bin/env bash
# token-budget-demo.sh -- burn the gateway's LLM token budget, watch the
# 429, and show the budget recover when the window rolls.
#
# The budget itself is GitOps state (app/policies/token-budget.yaml): at
# most 100000 tokens per minute on the vllm-sim-llm route, counted on both
# input and output. This script only generates load against an already
# reachable gateway:
#
#   1. sends chat completions until one is rejected. Each request uses
#      max_tokens 512, and the sim pads its canned output to the cap, so
#      each request burns ~510 tokens and the 100000-token budget exhausts
#      after ~200 requests;
#   2. reports the first rejection with the cumulative token count
#      (HTTP 429 when the token budget is what tripped);
#   3. waits for the per-minute window to roll, sends one request, and
#      reports whether the budget recovered (one more 60s wait if the
#      limiter is a sliding window rather than a fixed one).
#
# Usage:
#   scripts/token-budget-demo.sh --upstream http://127.0.0.1:18080
#
# --upstream must already be reachable; the mise task (token-budget-demo)
# sets up the gateway port-forward the same way the inference-demo task
# does.
#
# Env:
#   MODEL      model name to request (default Qwen/Qwen3-32B)
#   MAX_TOKENS completion cap per request (default 512)

set -euo pipefail

usage() {
  echo "Usage: $0 --upstream http://127.0.0.1:18080" >&2
  exit 2
}

UPSTREAM=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --upstream) UPSTREAM="${2:?--upstream needs a URL}"; shift 2 ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done
[[ -n "$UPSTREAM" ]] || usage
UPSTREAM="${UPSTREAM%/}"

MODEL="${MODEL:-Qwen/Qwen3-32B}"
MAX_TOKENS="${MAX_TOKENS:-512}"

send() {
  # Prints "<http_code> <total_tokens>" for one chat completion.
  curl -sS -m 30 -o /tmp/token-budget-demo.body \
    -w '%{http_code} ' \
    "$UPSTREAM/v1/chat/completions" \
    -H 'content-type: application/json' \
    -d "{\"model\":\"$MODEL\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}],\"max_tokens\":$MAX_TOKENS}"
  sed -n 's/.*"total_tokens":\([0-9][0-9]*\).*/\1/p' /tmp/token-budget-demo.body | head -n 1
}

echo "burning the token budget on ${UPSTREAM} (model ${MODEL}, max_tokens ${MAX_TOKENS})"

ok=0
tokens=0
while true; do
  out=$(send)
  code=${out%% *}
  used=${out#* }
  if [[ "$code" == "200" ]]; then
    ok=$((ok + 1))
    tokens=$((tokens + ${used:-0}))
    if (( ok % 25 == 0 )); then
      echo "  ${ok} accepted, ~${tokens} tokens"
    fi
  else
    echo "  ${ok} accepted, ~${tokens} tokens"
    echo "  request #${ok}+1 rejected: HTTP ${code}"
    if [[ "$code" != "429" ]]; then
      echo "  note: expected HTTP 429 from the token budget, got ${code}"
    fi
    break
  fi
done

# Fixed-window semantics: the next minute boundary is enough. Sliding
# window: need one full minute after the last counted request.
now_epoch=$(date +%s)
wait_s=$(( (now_epoch / 60 + 1) * 60 - now_epoch + 2 ))
echo "waiting ${wait_s}s for the per-minute window to roll..."
sleep "$wait_s"

out=$(send)
code=${out%% *}
if [[ "$code" != "200" ]]; then
  echo "still rejected (HTTP ${code}); the limiter may be a sliding window. Waiting a full 60s..."
  sleep 60
  out=$(send)
  code=${out%% *}
fi

if [[ "$code" == "200" ]]; then
  echo "recovered: HTTP 200 after the window rolled"
else
  echo "not recovered: HTTP ${code}; check the policy (app/policies/token-budget.yaml)"
fi
rm -f /tmp/token-budget-demo.body
