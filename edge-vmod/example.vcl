vcl 4.1;

# Kickoff's edge policy, driving a real Varnish instead of the in-app simulation.
# Point the backend at Kickoff's origin server (origin.rs), then:
#   varnishd -f example.vcl -a :6081 -p vmod_path=./target/release
import kickoff_edge;

backend origin {
    .host = "127.0.0.1";
    .port = "8080";   # Kickoff origin (README: "150ms default latency")
}

sub vcl_hash {
    # Share one cache slot across viewers by dropping the per-viewer ?token=
    # stub — the "Include Token in Cache Key = OFF" path from the lab. Turning
    # fragmentation back on is as simple as hashing req.url directly instead.
    hash_data(kickoff_edge.normalize_key(req.url));
    return (lookup);
}

sub vcl_backend_response {
    # Per-class TTL, exactly the lab's ttl_for: manifests 2s, segments 5m.
    set beresp.ttl = kickoff_edge.ttl(bereq.url);

    # Stale-while-revalidate: serve stale for 10s while a background fetch
    # refreshes. This is the lab's "[SWR] background refresh" line, for real.
    set beresp.grace = kickoff_edge.swr_grace();

    # Debug aid: see the class Varnish picked, like the lab's stats panel.
    set beresp.http.X-Kickoff-Class = kickoff_edge.class(bereq.url);
}

# Request collapsing (the lab's headline toggle) needs no VMOD call: Varnish
# coalesces concurrent misses for the same hash onto one backend fetch via its
# waiting list by default. That is the "collapse ratio" the flash crowd shows.
