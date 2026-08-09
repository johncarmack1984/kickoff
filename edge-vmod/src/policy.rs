//! Pure cache-policy logic, mirrored from the Tauri lab's `src-tauri/src/edge.rs`.
//!
//! Deliberately free of any Varnish types so it unit-tests without the Varnish
//! headers installed, and so the exact same decisions the desktop lab
//! visualizes can be lifted into a crate shared by both the app and this VMOD.

/// Manifest TTL — short, because `.m3u8` playlists change as the stream grows.
/// Matches `MANIFEST_TTL_SECS` in `edge.rs`.
pub const MANIFEST_TTL_SECS: u64 = 2;
/// Segment TTL — long, because media segments are immutable once written.
/// Matches `SEGMENT_TTL_SECS` in `edge.rs`.
pub const SEGMENT_TTL_SECS: u64 = 300;
/// Stale-while-revalidate grace window. Matches `SWR_GRACE_SECS` in `edge.rs`.
pub const SWR_GRACE_SECS: u64 = 10;

/// Per-viewer / volatile query params dropped from the cache key, so every
/// viewer shares one slot. The lab's `?token=` stub lives here; this is the
/// `token_in_cache_key = false` half of `EdgeCache::build_cache_key`.
const STRIP_PARAMS: &[&str] = &["token"];

/// What kind of HLS object a path is. Drives the TTL.
#[derive(Debug, PartialEq, Eq)]
pub enum Class {
    Manifest,
    Segment,
    Other,
}

/// Classify by extension, ignoring any query string / fragment.
pub fn classify(path: &str) -> Class {
    let p = path.split(['?', '#']).next().unwrap_or(path);
    if p.ends_with(".m3u8") {
        Class::Manifest
    } else if p.ends_with(".m4s")
        || p.ends_with(".mp4")
        || p.ends_with(".ts")
        || p.ends_with(".cmfv")
        || p.ends_with(".cmfa")
    {
        Class::Segment
    } else {
        Class::Other
    }
}

/// TTL in whole seconds for a path — the `ttl_for` function from `edge.rs`,
/// generalized to name the two classes explicitly. Unknown objects get the
/// short manifest TTL (fail safe: revalidate often rather than serve stale).
pub fn ttl_secs(path: &str) -> u64 {
    match classify(path) {
        Class::Manifest | Class::Other => MANIFEST_TTL_SECS,
        Class::Segment => SEGMENT_TTL_SECS,
    }
}

/// Canonical cache key: the URL with volatile per-viewer params stripped.
/// Mirrors `EdgeCache::build_cache_key` with token fragmentation OFF.
pub fn normalize_key(url: &str) -> String {
    let (path, query) = match url.split_once('?') {
        Some((p, q)) => (p, q),
        None => return url.to_string(),
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|kv| {
            let k = kv.split_once('=').map(|(k, _)| k).unwrap_or(kv);
            !STRIP_PARAMS.contains(&k)
        })
        .collect();
    if kept.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", kept.join("&"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension() {
        assert_eq!(classify("/master.m3u8"), Class::Manifest);
        assert_eq!(classify("/variant.m3u8?token=abc"), Class::Manifest);
        assert_eq!(classify("/seg/00042.m4s"), Class::Segment);
        assert_eq!(classify("/seg/00042.ts"), Class::Segment);
        assert_eq!(classify("/favicon.ico"), Class::Other);
    }

    #[test]
    fn ttl_matches_the_lab() {
        assert_eq!(ttl_secs("/master.m3u8"), MANIFEST_TTL_SECS);
        assert_eq!(ttl_secs("/seg/1.m4s"), SEGMENT_TTL_SECS);
        // Unknown objects fail safe to the short TTL.
        assert_eq!(ttl_secs("/whatever"), MANIFEST_TTL_SECS);
    }

    #[test]
    fn strips_token_but_keeps_real_params() {
        assert_eq!(normalize_key("/master.m3u8?token=viewer-7"), "/master.m3u8");
        assert_eq!(
            normalize_key("/seg/1.m4s?token=v7&bitrate=hi"),
            "/seg/1.m4s?bitrate=hi"
        );
        assert_eq!(normalize_key("/seg/1.m4s"), "/seg/1.m4s");
    }
}
