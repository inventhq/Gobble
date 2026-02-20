#!/bin/bash
# End-to-end SSE pipeline test
# Tests that events sent via /ingest and /batch reach the SSE gateway
set -e

TRACK_URL="https://track.juicyapi.com"
SSE_URL="https://sse.juicyapi.com"
TOKEN="pt_6vct_78a88n1we4gnl0nouzo619bi9hvmwemc"

echo "=== 1. Health check ==="
echo "tracker-core:"
curl -m 5 -s "$TRACK_URL/health"
echo ""
echo "sse-gateway:"
curl -m 5 -s "$SSE_URL/health"
echo ""

echo ""
echo "=== 2. Open SSE stream (background, 20s timeout) ==="
SSE_OUT=$(mktemp)
curl -m 20 -sN "$SSE_URL/sse/events" > "$SSE_OUT" 2>&1 &
SSE_PID=$!
sleep 2

echo ""
echo "=== 3. Send /ingest event ==="
curl -m 5 -s -X POST "$TRACK_URL/ingest" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"event_type":"sse_e2e_ingest","params":{"src":"test_script","path":"ingest"}}'
echo ""

echo ""
echo "=== 4. Send /batch event ==="
curl -m 5 -s -X POST "$TRACK_URL/batch" \
  -H "Content-Type: application/json" \
  -d "[{\"event_id\":\"019c7c99-e2e0-7000-8000-$(date +%s)\",\"event_type\":\"sse_e2e_batch\",\"timestamp\":$(date +%s)000,\"ip\":\"1.2.3.4\",\"user_agent\":\"test-script\",\"referer\":null,\"accept_language\":\"en\",\"request_path\":\"/batch\",\"request_host\":\"track.juicyapi.com\",\"params\":{\"key_prefix\":\"6vct\",\"src\":\"test_script\",\"path\":\"batch\"}}]"
echo ""

echo ""
echo "=== 5. Wait 10s for events to flow ==="
sleep 10

echo ""
echo "=== 6. Check SSE gateway health (counters) ==="
curl -m 5 -s "$SSE_URL/health" | python3 -m json.tool
echo ""

echo ""
echo "=== 7. SSE stream output ==="
kill $SSE_PID 2>/dev/null || true
wait $SSE_PID 2>/dev/null || true
echo "--- BEGIN SSE OUTPUT ---"
cat "$SSE_OUT"
echo "--- END SSE OUTPUT ---"
rm -f "$SSE_OUT"

echo ""
echo "=== Done ==="
