//! `kickoff_edge` — the Tauri lab's edge-cache policy, compiled as a real
//! Varnish VMOD.
//!
//! The desktop app in `../src-tauri` *simulates* an edge cache (see
//! `edge.rs`): per-class TTLs, stale-while-revalidate, request collapsing, and
//! token-stripped cache keys, all so the behavior is visible on screen. This
//! crate exposes that same policy to a real Varnish, so the lab can drive an
//! actual cache instead of a `tokio::time::sleep` stand-in. Each function here
//! maps one lab concept onto a Varnish mechanism (see `example.vcl`):
//!
//! | Lab concept              | Varnish mechanism        | This VMOD        |
//! |--------------------------|--------------------------|------------------|
//! | per-class TTL (`ttl_for`)| `beresp.ttl`             | [`kickoff_edge::ttl`]        |
//! | SWR grace window         | `beresp.grace`           | [`kickoff_edge::swr_grace`]  |
//! | token-stripped key       | `hash_data` in `vcl_hash`| [`kickoff_edge::normalize_key`] |
//! | request collapsing       | Varnish waiting list     | *(built in)*     |

mod policy;

/// Kickoff edge-cache policy.
#[varnish::vmod(docs = "README.md")]
mod kickoff_edge {
    use std::time::Duration;

    use super::policy;

    /// TTL for an object, chosen by its HLS class: manifests are short (they
    /// change as the stream grows), segments are long (immutable once written).
    /// This is the lab's `ttl_for`, verbatim policy.
    pub fn ttl(path: &str) -> Duration {
        Duration::from_secs(policy::ttl_secs(path))
    }

    /// Stale-while-revalidate grace: how long Varnish may serve a stale object
    /// while a background fetch refreshes it. Set this on `beresp.grace`.
    pub fn swr_grace() -> Duration {
        Duration::from_secs(policy::SWR_GRACE_SECS)
    }

    /// The cache class of a path: `"manifest"`, `"segment"`, or `"other"`.
    /// Handy to stamp on a debug response header while tuning.
    pub fn class(path: &str) -> String {
        match policy::classify(path) {
            policy::Class::Manifest => "manifest",
            policy::Class::Segment => "segment",
            policy::Class::Other => "other",
        }
        .to_string()
    }

    /// Canonical cache key: the request URL with volatile per-viewer params
    /// (the lab's `?token=` stub) stripped, so every viewer shares one slot.
    /// Feed this to `hash_data()` in `vcl_hash`. Flipping the lab's
    /// "Include Token in Cache Key" toggle is the same as *not* calling this.
    pub fn normalize_key(url: &str) -> String {
        policy::normalize_key(url)
    }
}

#[cfg(test)]
mod tests {
    // End-to-end tests: each `tests/*.vtc` boots a real Varnish with this VMOD
    // and drives traffic through it. Requires `varnishtest` on PATH (it ships
    // with Varnish); the pure-logic tests in `policy.rs` need nothing.
    varnish::run_vtc_tests!("tests/*.vtc");
}
