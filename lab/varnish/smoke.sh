#!/bin/bash
set -euo pipefail

BASE="http://localhost:8080"
PASS=0
FAIL=0

pass() { echo "  PASS: $1"; ((PASS++)) || true; }
fail() { echo "  FAIL: $1"; ((FAIL++)) || true; }

echo "=== Test 1: Valid token -> two distinct Set-Cookie lines ==="
RESP=$(curl -sD - "$BASE/master.m3u8?token=kickoff-test" -o /dev/null)
echo "$RESP" | grep -i "^set-cookie:" || true

COOKIE_COUNT=$(echo "$RESP" | grep -ci "^set-cookie:" || true)
[ "$COOKIE_COUNT" -eq 2 ] && pass "two Set-Cookie headers" || fail "expected 2 Set-Cookie headers, got $COOKIE_COUNT"

echo "$RESP" | grep -qi "playback_session=" && pass "playback_session cookie" || fail "playback_session missing"
echo "$RESP" | grep -qi "edge_pin=" && pass "edge_pin cookie" || fail "edge_pin missing"
echo "$RESP" | grep -qi "x-edge-auth: sideband" && pass "X-Edge-Auth: sideband" || fail "X-Edge-Auth not sideband"
echo "$RESP" | grep -qi "cmsd-dynamic" && pass "CMSD-Dynamic present" || fail "CMSD-Dynamic missing"

PS_COOKIE=$(echo "$RESP" | grep -i "^set-cookie:.*playback_session" | sed 's/.*playback_session=/playback_session=/' | cut -d';' -f1 | tr -d '\r')
EP_COOKIE=$(echo "$RESP" | grep -i "^set-cookie:.*edge_pin" | sed 's/.*edge_pin=/edge_pin=/' | cut -d';' -f1 | tr -d '\r')

echo ""
echo "=== Test 2: Cookies present -> sideband skipped ==="
RESP2=$(curl -sD - -H "Cookie: $PS_COOKIE; $EP_COOKIE" "$BASE/master.m3u8" -o /dev/null)
echo "$RESP2" | grep -i "x-edge-auth" || true
echo "$RESP2" | grep -qi "x-edge-auth: cached" && pass "sideband skipped (cached)" || fail "expected cached auth"

echo ""
echo "=== Test 3: Segment cache — first MISS, then HIT ==="
RESP3=$(curl -sD - -H "Cookie: $PS_COOKIE; $EP_COOKIE" "$BASE/seg_000.m4s" -o /dev/null)
echo "$RESP3" | grep -i "x-cache" || true
echo "$RESP3" | grep -qi "x-cache: miss" && pass "first segment: MISS" || fail "expected MISS"

RESP4=$(curl -sD - -H "Cookie: $PS_COOKIE; $EP_COOKIE" "$BASE/seg_000.m4s" -o /dev/null)
echo "$RESP4" | grep -i "x-cache" || true
echo "$RESP4" | grep -qi "x-cache: hit" && pass "second segment: HIT" || fail "expected HIT"

echo ""
echo "=== Test 4: Bad token -> 403 ==="
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" "$BASE/master.m3u8?token=invalid")
echo "HTTP status: $HTTP_CODE"
[ "$HTTP_CODE" = "403" ] && pass "bad token -> 403" || fail "expected 403, got $HTTP_CODE"

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="
[ "$FAIL" -eq 0 ] && exit 0 || exit 1
