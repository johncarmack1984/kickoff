#!/usr/bin/env python3
"""Minimal entitlement service for the edge-auth-sideband lab.

GET /entitle?token=<valid>  -> 200 + two Set-Cookie headers
GET /entitle?token=<invalid> -> 403
"""

from http.server import HTTPServer, BaseHTTPRequestHandler
from urllib.parse import urlparse, parse_qs
from datetime import datetime, timedelta, timezone
import uuid
import hashlib

VALID_TOKENS = {"kickoff-test", "demo", "valid"}


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        parsed = urlparse(self.path)
        if parsed.path != "/entitle":
            self.send_response(404)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            self.wfile.write(b"not found")
            return

        params = parse_qs(parsed.query)
        token = params.get("token", [None])[0]

        if token not in VALID_TOKENS:
            self.send_response(403)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            self.wfile.write(b"forbidden")
            return

        session_id = str(uuid.uuid4())
        pin = hashlib.sha256(session_id.encode()).hexdigest()[:16]
        expires = datetime.now(timezone.utc) + timedelta(hours=4)
        expires_str = expires.strftime("%a, %d %b %Y %H:%M:%S GMT")

        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header(
            "Set-Cookie",
            f"playback_session={session_id}; Path=/; HttpOnly",
        )
        self.send_header(
            "Set-Cookie",
            f"edge_pin={pin}; Expires={expires_str}; Path=/",
        )
        self.end_headers()
        self.wfile.write(b"ok")

    def log_message(self, fmt, *args):
        print(f"[entitlement] {fmt % args}")


if __name__ == "__main__":
    server = HTTPServer(("0.0.0.0", 8000), Handler)
    print("[entitlement] listening on :8000")
    server.serve_forever()
