# kickoff_edge — Kickoff's edge policy as a Varnish VMOD

Kickoff is a desktop lab that makes CDN-edge behavior visible by *simulating* an origin + edge cache in-process. This crate takes the policy that simulation encodes — the part actually worth watching — and compiles it into a real [Varnish](https://varnish-cache.org/) module, so the lab can drive an actual cache instead of `tokio::time::sleep`.

The decision logic lives in [`src/policy.rs`](src/policy.rs), mirrored line-for-line from the app's `src-tauri/src/edge.rs` and kept free of Varnish types so it unit-tests on its own (`cargo test`). The `#[varnish::vmod]` block in [`src/lib.rs`](src/lib.rs) is a thin wrapper over it.

## What maps to what

| Lab concept (`edge.rs`) | Varnish mechanism | VMOD function |
|---|---|---|
| per-class TTL (`ttl_for`) | `beresp.ttl` | `kickoff_edge.ttl(path)` |
| stale-while-revalidate grace | `beresp.grace` | `kickoff_edge.swr_grace()` |
| token-stripped cache key | `hash_data()` in `vcl_hash` | `kickoff_edge.normalize_key(url)` |
| request collapsing | Varnish waiting list | *(built in — no call needed)* |

`MANIFEST_TTL_SECS = 2`, `SEGMENT_TTL_SECS = 300`, `SWR_GRACE_SECS = 10` — the same constants as the app.

## Build

Requires the Varnish development headers (the `varnish` crate links `libvarnishapi`):

```sh
brew install varnish        # macOS; or apt-get install varnish-dev on Debian/Ubuntu
cargo build --release
```

This produces `target/release/libvmod_kickoff_edge.dylib` (`.so` on Linux). Load it with `varnishd -p vmod_path=./target/release -f example.vcl`.

> Without the Varnish headers the crate won't link (that's inherent to any VMOD). The pure policy in `src/policy.rs` still compiles and its tests still run.

## Tests

- `cargo test` runs the pure-logic unit tests in `src/policy.rs` with nothing installed.
- With `varnishtest` on PATH (it ships with Varnish), the same `cargo test` also runs the end-to-end cases in [`tests/`](tests): `keys.vtc` proves two viewers with different `?token=` collapse onto one cached object, and `class.vtc` proves segments classify correctly and real query params are *not* collapsed. Each boots a real Varnish with this VMOD and asserts `X-Cache` hit/miss.

## Try it

See [`example.vcl`](example.vcl) for a complete config that wires all four behaviors. Run the app's flash-crowd simulator against `:6081` and watch `varnishstat` — hit ratio, backend fetches, and grace hits are now real numbers, not simulated counters.

## Drive it from the Kickoff app

The desktop app can spawn a real `varnishd` loaded with this VMOD instead of its simulated edge: build this crate (`cargo build --release`), launch the app, ingest a source, then tick **Use real Varnish** in Edge Controls. The app starts a real HTTP origin + `varnishd`, repoints the player at Varnish's port, and shows live `varnishstat` counters. If the app can't find the built module, set `KICKOFF_VMOD_PATH` to the `libvmod_kickoff_edge.{dylib,so}` it produced.
