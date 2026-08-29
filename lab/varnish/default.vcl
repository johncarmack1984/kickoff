vcl 4.1;

import reqwest from "/vmods/";

backend origin {
    .host = "origin";
    .port = "80";
}

sub vcl_init {
    new auth_client = reqwest.client(timeout = 2s);
}

sub vcl_recv {
    set req.backend_hint = origin;
    return (hash);
}

sub vcl_hash {
    # Strip the per-viewer token so all viewers share one cache slot.
    hash_data(regsub(req.url, "[?&]token=[^&]*", ""));
    hash_data(req.http.host);
    # Range is deliberately excluded: ranged and full requests share a
    # cache slot (RFC 9111 Section 4.1).
    return (lookup);
}

sub vcl_backend_response {
    if (bereq.url ~ "\.(m3u8)(\?|$)") {
        set beresp.ttl = 2s;
        set beresp.grace = 10s;
    } else if (bereq.url ~ "\.(m4s|mp4|ts)(\?|$)") {
        set beresp.ttl = 300s;
        set beresp.grace = 60s;
    } else {
        set beresp.ttl = 30s;
        set beresp.grace = 10s;
    }
}

sub vcl_deliver {
    if (obj.hits > 0) {
        set resp.http.X-Cache = "HIT";
    } else {
        set resp.http.X-Cache = "MISS";
    }

    # --- Auth sideband ---
    # First request of a session (no playback_session cookie) triggers a
    # sideband call to the entitlement service.  The origin never sees auth.
    if (req.http.Cookie !~ "playback_session=") {
        if (req.url ~ "[?&]token=") {
            auth_client.init("auth", "http://entitlement:8000/entitle?token=" +
                regsub(req.url, ".*[?&]token=([^&]+).*", "\1"));

            if (auth_client.status("auth") == 200) {
                # Each Set-Cookie line is appended individually -- no
                # comma-joining, so the Expires date's comma survives.
                auth_client.copy_headers_to_resp("auth", "Set-Cookie");
                set resp.http.X-Edge-Auth = "sideband";
            } else {
                return (synth(403, "Forbidden"));
            }
        } else {
            return (synth(403, "Missing token"));
        }
    } else {
        set resp.http.X-Edge-Auth = "cached";
    }

    # --- CMSD-Dynamic (CTA-5006 Section 3.2, Table 2) ---
    # sf-item (RFC 8941 Section 3.3) carrying server tag + per-response
    # metrics.  etp = estimated throughput (kbps), rtt = round-trip
    # estimate (ms).  Multi-value safe: the structured-field list
    # format survives intermediary header folding because its grammar
    # never collides with the Set-Cookie comma problem.
    set resp.http.CMSD-Dynamic = {""edge-kickoff"; etp=50000; rtt=5"};

    set resp.http.Access-Control-Allow-Origin = "*";
    set resp.http.Access-Control-Expose-Headers = "X-Cache, X-Edge-Auth, CMSD-Dynamic";
}

sub vcl_synth {
    set resp.http.Content-Type = "text/plain";
    set resp.http.Access-Control-Allow-Origin = "*";
    synthetic(resp.reason);
    return (deliver);
}
