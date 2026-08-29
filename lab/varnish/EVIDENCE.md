# Evidence

Captured from a clean `docker compose up` on macOS (Apple Silicon, OrbStack).
Varnish 9.0.3, vmod-reqwest built from varnish-rs/vmod-reqwest#40 (copy_headers_to_resp branch).

## docker compose ps

```
NAME                    IMAGE            COMMAND                  SERVICE       CREATED         STATUS          PORTS
varnish-edge-1          varnish:latest   "/usr/local/bin/dock…"   edge          3 minutes ago   Up 12 seconds   8443/tcp, 0.0.0.0:8080->80/tcp, [::]:8080->80/tcp
varnish-entitlement-1   python:3-slim    "python3 /app/entitl…"   entitlement   3 minutes ago   Up 3 minutes    8000/tcp
varnish-origin-1        varnish-origin   "bash /gen-hls.sh"       origin        3 minutes ago   Up 3 minutes    80/tcp
```

## Full smoke test output

```
=== Test 1: Valid token -> two distinct Set-Cookie lines ===
Set-Cookie: playback_session=113fcfed-b0b2-42a3-9ef9-a0eed18bc886; Path=/; HttpOnly
Set-Cookie: edge_pin=fabd69c89cb68f5f; Expires=Sun, 30 Aug 2026 00:24:28 GMT; Path=/
  PASS: two Set-Cookie headers
  PASS: playback_session cookie
  PASS: edge_pin cookie
  PASS: X-Edge-Auth: sideband
  PASS: CMSD-Dynamic present

=== Test 2: Cookies present -> sideband skipped ===
X-Edge-Auth: cached
Access-Control-Expose-Headers: X-Cache, X-Edge-Auth, CMSD-Dynamic
  PASS: sideband skipped (cached)

=== Test 3: Segment cache — first MISS, then HIT ===
X-Cache: MISS
Access-Control-Expose-Headers: X-Cache, X-Edge-Auth, CMSD-Dynamic
  PASS: first segment: MISS
X-Cache: HIT
Access-Control-Expose-Headers: X-Cache, X-Edge-Auth, CMSD-Dynamic
  PASS: second segment: HIT

=== Test 4: Bad token -> 403 ===
HTTP status: 403
  PASS: bad token -> 403

=== Results: 9 passed, 0 failed ===
```

## Raw first-request headers (unabbreviated)

```
HTTP/1.1 200 OK
Server: SimpleHTTP/0.6 Python/3.14.7
Date: Sat, 29 Aug 2026 20:24:28 GMT
Content-type: application/vnd.apple.mpegurl
Content-Length: 120
Last-Modified: Sat, 29 Aug 2026 20:21:27 GMT
X-Varnish: 32774 3
Age: 0
Via: 1.1 966d703d75a8 (Varnish/9.0)
Accept-Ranges: bytes
X-Cache: HIT
Set-Cookie: playback_session=bfbec2cb-cb01-4dd4-bf94-f8bb9c747c56; Path=/; HttpOnly
Set-Cookie: edge_pin=a21d2cdf7a54f392; Expires=Sun, 30 Aug 2026 00:24:28 GMT; Path=/
X-Edge-Auth: sideband
CMSD-Dynamic: "edge-kickoff"; etp=50000; rtt=5
Access-Control-Allow-Origin: *
Access-Control-Expose-Headers: X-Cache, X-Edge-Auth, CMSD-Dynamic
Connection: keep-alive

```

## Cached-session request headers

```
HTTP/1.1 200 OK
Server: SimpleHTTP/0.6 Python/3.14.7
Date: Sat, 29 Aug 2026 20:24:28 GMT
Content-type: application/vnd.apple.mpegurl
Content-Length: 120
Last-Modified: Sat, 29 Aug 2026 20:21:27 GMT
X-Varnish: 10 3
Age: 0
Via: 1.1 966d703d75a8 (Varnish/9.0)
Accept-Ranges: bytes
X-Cache: HIT
X-Edge-Auth: cached
CMSD-Dynamic: "edge-kickoff"; etp=50000; rtt=5
Access-Control-Allow-Origin: *
Access-Control-Expose-Headers: X-Cache, X-Edge-Auth, CMSD-Dynamic
Connection: keep-alive

```

## Bad token headers

```
HTTP/1.1 403 Forbidden
Date: Sat, 29 Aug 2026 20:24:28 GMT
Server: Varnish
X-Varnish: 32776
Content-Type: text/plain
Access-Control-Allow-Origin: *
Content-Length: 9
Connection: keep-alive

```
