# Kickoff

A desktop HLS lab that makes CDN-edge behavior visible. Transcodes video into an ABR ladder, serves it through a simulated origin + edge cache, and exposes cache behavior on screen: hits/misses, TTLs, stale-while-revalidate, purging, and request collapsing under a simulated flash crowd.

## Architecture

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│   hls.js     │────▶│  Edge Cache  │────▶│   Origin     │
│   Player     │     │ (kickoff://) │     │  (disk + lag)│
│              │◀────│              │◀────│              │
└──────────────┘     └──────────────┘     └──────────────┘
       │                    │                    │
   Playback            Per-class TTLs       150ms default
   ABR switching       LRU (512 entries)    latency
   Rendition display   SWR background       Per-object
                       refresh              fetch counter
                       Request collapsing
                       Token-aware keys
```

**Origin** serves generated HLS manifests and fMP4 segments from disk. Configurable artificial latency (default 150ms) simulates network round-trip. Tracks per-object fetch counts.

**Edge Cache** sits in front of origin, exposed as a Tauri custom URI scheme (`kickoff://`). Features: per-class TTLs (manifests 2s, segments 5min), LRU eviction (512 entries), stale-while-revalidate (serve stale + background refresh during a 10s grace window), purge-all and purge-by-prefix, request collapsing (concurrent misses for the same object share one origin fetch), and configurable cache-key discipline (token stripping).

**Player** uses hls.js against the master playlist through the edge. Displays current rendition and ABR switch events.

## Stack

- **Tauri v2** — desktop shell, custom protocol handler
- **Rust** — origin, edge cache, simulator, ingest orchestration
- **TypeScript** — player UI, controls, stats display
- **hls.js** — adaptive bitrate streaming player
- **ffmpeg** — transcoding (required at runtime, not bundled)

## Prerequisites

- Rust toolchain (stable)
- Node.js 18+
- ffmpeg (install via `brew install ffmpeg`)
- `cargo-tauri` CLI (`cargo install tauri-cli --version "^2"`)

## Running

```sh
npm install
cargo tauri dev
```

Click **Use Test Source** to generate a 30-second test pattern, or **Pick Video File** to transcode a local video. Transcoding produces a 3-rung ABR ladder (240p/480p/720p) with fMP4/CMAF segments at ~4s target duration.

## What each toggle demonstrates

| Toggle | Default | What it shows |
|--------|---------|---------------|
| **Request Collapsing** | ON | When ON, concurrent cache misses for the same object share a single origin fetch. Run the flash crowd with it ON, note origin fetches. Turn it OFF, purge, run again — origin fetches jump because each miss triggers its own fetch. |
| **Include Token in Cache Key** | OFF | Each simulated viewer sends a unique `?token=` stub. With the toggle OFF, the edge strips the token from the cache key (validates but doesn't fragment). Turn it ON and watch the hit ratio collapse — every viewer gets its own cache slot. |
| **Origin Latency** | 150ms | Drag higher to widen the window where concurrent requests overlap. Higher latency = more requests in flight = more collapsing (when enabled). |
| **Purge All / Purge Prefix** | — | Purge forces the edge to re-fetch from origin. Useful for demonstrating that SWR doesn't permanently serve stale content. |

## Flash crowd

The simulator spawns N viewers (default 200) that start the same stream within a 10-second window. Each viewer requests the master playlist, variant playlists, and all segments through the edge cache. Live counters show hits, misses, origin fetches, collapse ratio, and hit ratio.

## SWR proof

Watch the log panel after content has been cached and the manifest TTL (2s) expires. You'll see lines like:

```
[SWR] background refresh started for master.m3u8
[SWR] background refresh completed for master.m3u8
```

The first line proves the edge returned stale content immediately. The second proves the background refresh completed and updated the cache.

## Real Varnish mode (optional)

The edge above is simulated so its behavior is visible on screen. For the real thing, `edge-vmod/` compiles the exact same policy (per-class TTL, SWR grace, token-stripped keys) into a [Varnish](https://varnish-cache.org/) VMOD. Build it (`cargo build --release` in `edge-vmod/`, needs `brew install varnish`), then tick **Use real Varnish** in Edge Controls: the app spawns a real HTTP origin + `varnishd` loaded with the module, repoints the player at Varnish's port, and shows live `varnishstat` counters. See [`edge-vmod/README.md`](edge-vmod/README.md).

## Edge auth sideband lab (Docker)

`lab/varnish/` runs the "auth at the edge, origin stays dumb" pattern used by streaming CDNs, with real Varnish + [vmod-reqwest](https://github.com/varnish-rs/vmod-reqwest) doing the work. Three containers: a static HLS origin (ffmpeg test pattern), a tiny entitlement service, and Varnish as the edge. The edge intercepts the first request of a session, makes a sideband HTTP call to the entitlement service, and copies the resulting `Set-Cookie` headers onto the response using `copy_headers_to_resp()` — each cookie lands as its own header line, so the `Expires` date's comma never breaks anything. Subsequent requests carry the session cookie and skip the sideband entirely.

The VCL also sets a `CMSD-Dynamic` header per CTA-5006 as a second demonstration of multi-value-safe header handling via RFC 8941 structured fields.

```sh
cd lab/varnish
bash run.sh        # builds vmod, starts compose, waits for readiness
bash smoke.sh      # curl-driven proof of all four behaviors
docker compose down
```

`run.sh` builds `libvmod_reqwest.so` from a local vmod-reqwest checkout (defaults to `~/coding/varnish/vmod-reqwest`; override with `VMOD_REQWEST_SRC`). The build happens in Docker against `varnish:latest` so the ABI matches the runtime.

Tick **Edge Lab (Docker)** in Edge Controls to point the hls.js player at `http://localhost:8080` while compose is running.

## Honest limits

- **Single process (simulated mode)** — by default origin, edge, and player all live in the same Tauri app. There is no real network hop; latency is `tokio::time::sleep`. (Real Varnish mode, above, does use a real origin server + `varnishd`.)
- **Simulated network (simulated mode)** — no actual HTTP servers or TCP connections. The edge is a Tauri custom protocol handler; the origin is a function call with artificial delay.
- **No persistence** — cache is in-memory, lost on restart.
- **No real auth (simulated mode)** — the `?token=` parameter is a stub for demonstrating cache-key fragmentation, not a real authentication system. The Docker edge lab (`lab/varnish/`) adds a sideband auth flow with real cookies, but the entitlement service is a hardcoded demo — it is not a security boundary.
- **macOS only tested** — built and tested on macOS (Apple Silicon). Should build on Linux/Windows but untested.

## 90-second demo script

1. **Launch** — `cargo tauri dev`. Click **Use Test Source**. Wait for transcoding (~10s).
2. **Watch playback** — video plays through the edge. Note "ABR switch" events in the log and the rendition label updating.
3. **Check stats** — the stats panel shows cache hits accumulating as hls.js re-fetches manifests and loads segments.
4. **Flash crowd (collapsing ON)** — leave Request Collapsing checked. Click **Run** with 200 viewers. Note origin fetches (should be low relative to total requests). Collapse ratio should be high.
5. **Flash crowd (collapsing OFF)** — uncheck Request Collapsing. Click **Purge All**, then **Run** again. Origin fetches jump. Collapse ratio drops to 0%.
6. **Token fragmentation** — check **Include Token in Cache Key**. Click **Purge All**, then **Run** again. Hit ratio tanks because each viewer's token creates a unique cache key.
7. **Purge** — click **Purge All**. The next manifest fetch is a miss (visible in the log). Within seconds, SWR lines appear proving background refresh.
8. **SWR proof** — wait for "background refresh" lines in the log after a manifest TTL expires. The player never stalls because it received stale content while the refresh ran.

## Project structure

```
kickoff/
├── index.html              # Entry HTML (Vite root)
├── src/
│   ├── main.ts             # Frontend: player, controls, stats, sparklines
│   └── style.css            # Dark theme UI
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/
│   │   └── default.json    # Tauri v2 ACL
│   └── src/
│       ├── main.rs          # Entry point
│       ├── lib.rs           # Tauri setup, protocol handler, commands
│       ├── ingest.rs        # ffmpeg probe + ABR transcoding
│       ├── origin.rs        # Origin server (disk + latency)
│       ├── edge.rs          # Edge cache (LRU, TTL, SWR, collapsing)
│       ├── simulator.rs    # Flash crowd simulator
│       └── varnish.rs      # Optional real-Varnish mode (spawns varnishd + HTTP origin)
├── edge-vmod/              # The same edge policy as a real Varnish VMOD
│   ├── src/policy.rs        # Pure policy (unit-tested), mirrored from edge.rs
│   ├── src/lib.rs           # #[varnish::vmod] wrapper
│   ├── tests/*.vtc          # End-to-end tests against a real Varnish
│   └── example.vcl          # Standalone VCL wiring
├── lab/varnish/            # Docker-based edge auth sideband lab
│   ├── docker-compose.yml   # origin + entitlement + edge (varnish)
│   ├── default.vcl          # Auth sideband VCL using vmod-reqwest
│   ├── smoke.sh             # curl-driven proof of the four behaviors
│   └── run.sh               # Build vmod + compose up + readiness check
└── README.md
```
