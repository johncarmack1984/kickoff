use crate::origin::Origin;
use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

pub struct EdgeCache {
    origin: Arc<Origin>,
    cache: Arc<Mutex<LruCache<String, CacheEntry>>>,
    inflight: Mutex<HashMap<String, Arc<tokio::sync::OnceCell<Result<InflightResult, String>>>>>,
    pub stats: Arc<CacheStats>,
    pub config: Arc<EdgeConfig>,
}

struct CacheEntry {
    body: Vec<u8>,
    content_type: String,
    inserted: Instant,
    ttl: Duration,
    swr_grace: Duration,
}

#[derive(Clone)]
struct InflightResult {
    body: Vec<u8>,
    content_type: String,
}

pub struct CacheStats {
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub origin_fetches: AtomicU64,
    pub swr_refreshes: AtomicU64,
    pub collapsed: AtomicU64,
}

pub struct EdgeConfig {
    pub collapsing_enabled: AtomicBool,
    pub token_in_cache_key: AtomicBool,
}

pub struct EdgeResponse {
    pub body: Vec<u8>,
    pub content_type: String,
    pub cache_status: &'static str,
}

const MANIFEST_TTL_SECS: u64 = 2;
const SEGMENT_TTL_SECS: u64 = 300;
const SWR_GRACE_SECS: u64 = 10;
const CACHE_CAPACITY: usize = 512;

impl CacheStats {
    fn new() -> Self {
        Self {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            origin_fetches: AtomicU64::new(0),
            swr_refreshes: AtomicU64::new(0),
            collapsed: AtomicU64::new(0),
        }
    }

    pub fn reset(&self) {
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.origin_fetches.store(0, Ordering::Relaxed);
        self.swr_refreshes.store(0, Ordering::Relaxed);
        self.collapsed.store(0, Ordering::Relaxed);
    }
}

impl EdgeConfig {
    fn new() -> Self {
        Self {
            collapsing_enabled: AtomicBool::new(true),
            token_in_cache_key: AtomicBool::new(false),
        }
    }
}

impl EdgeCache {
    pub fn new(origin: Arc<Origin>) -> Self {
        Self {
            origin,
            cache: Arc::new(Mutex::new(LruCache::new(
                NonZeroUsize::new(CACHE_CAPACITY).unwrap(),
            ))),
            inflight: Mutex::new(HashMap::new()),
            stats: Arc::new(CacheStats::new()),
            config: Arc::new(EdgeConfig::new()),
        }
    }

    pub async fn get(&self, path: &str, token: Option<&str>) -> Result<EdgeResponse, String> {
        let cache_key = self.build_cache_key(path, token);

        {
            let mut cache = self.cache.lock().await;
            if let Some(entry) = cache.get(&cache_key) {
                let age = entry.inserted.elapsed();

                if age < entry.ttl {
                    self.stats.hits.fetch_add(1, Ordering::Relaxed);
                    return Ok(EdgeResponse {
                        body: entry.body.clone(),
                        content_type: entry.content_type.clone(),
                        cache_status: "HIT",
                    });
                }

                if age < entry.ttl + entry.swr_grace {
                    self.stats.hits.fetch_add(1, Ordering::Relaxed);
                    self.stats.swr_refreshes.fetch_add(1, Ordering::Relaxed);
                    let stale_body = entry.body.clone();
                    let stale_ct = entry.content_type.clone();

                    let origin = self.origin.clone();
                    let cache_arc = self.cache.clone();
                    let stats = self.stats.clone();
                    let path_owned = path.to_string();
                    let key_owned = cache_key.clone();
                    let ttl = ttl_for(path);
                    let swr = Duration::from_secs(SWR_GRACE_SECS);

                    eprintln!("[SWR] background refresh started for {path}");
                    tauri::async_runtime::spawn(async move {
                        match origin.fetch(&path_owned).await {
                            Ok(resp) => {
                                stats.origin_fetches.fetch_add(1, Ordering::Relaxed);
                                let mut cache = cache_arc.lock().await;
                                cache.put(
                                    key_owned,
                                    CacheEntry {
                                        body: resp.body,
                                        content_type: resp.content_type,
                                        inserted: Instant::now(),
                                        ttl,
                                        swr_grace: swr,
                                    },
                                );
                                eprintln!("[SWR] background refresh completed for {path_owned}");
                            }
                            Err(e) => {
                                eprintln!("[SWR] background refresh failed for {path_owned}: {e}");
                            }
                        }
                    });

                    return Ok(EdgeResponse {
                        body: stale_body,
                        content_type: stale_ct,
                        cache_status: "HIT-SWR",
                    });
                }
            }
        }

        self.stats.misses.fetch_add(1, Ordering::Relaxed);
        self.fetch_with_collapsing(path, &cache_key).await
    }

    async fn fetch_with_collapsing(
        &self,
        path: &str,
        cache_key: &str,
    ) -> Result<EdgeResponse, String> {
        let collapsing = self.config.collapsing_enabled.load(Ordering::Relaxed);

        if collapsing {
            let cell = {
                let mut inflight = self.inflight.lock().await;
                if let Some(existing) = inflight.get(cache_key) {
                    self.stats.collapsed.fetch_add(1, Ordering::Relaxed);
                    existing.clone()
                } else {
                    let cell = Arc::new(tokio::sync::OnceCell::new());
                    inflight.insert(cache_key.to_string(), cell.clone());
                    cell
                }
            };

            let origin = self.origin.clone();
            let path_owned = path.to_string();
            let stats = self.stats.clone();

            let result = cell
                .get_or_init(|| async move {
                    stats.origin_fetches.fetch_add(1, Ordering::Relaxed);
                    match origin.fetch(&path_owned).await {
                        Ok(resp) => Ok(InflightResult {
                            body: resp.body,
                            content_type: resp.content_type,
                        }),
                        Err(e) => Err(e),
                    }
                })
                .await;

            {
                let mut inflight = self.inflight.lock().await;
                inflight.remove(cache_key);
            }

            match result {
                Ok(ir) => {
                    let mut cache = self.cache.lock().await;
                    cache.put(
                        cache_key.to_string(),
                        CacheEntry {
                            body: ir.body.clone(),
                            content_type: ir.content_type.clone(),
                            inserted: Instant::now(),
                            ttl: ttl_for(path),
                            swr_grace: Duration::from_secs(SWR_GRACE_SECS),
                        },
                    );
                    Ok(EdgeResponse {
                        body: ir.body.clone(),
                        content_type: ir.content_type.clone(),
                        cache_status: "MISS",
                    })
                }
                Err(e) => Err(e.clone()),
            }
        } else {
            self.stats.origin_fetches.fetch_add(1, Ordering::Relaxed);
            let resp = self.origin.fetch(path).await?;
            let mut cache = self.cache.lock().await;
            cache.put(
                cache_key.to_string(),
                CacheEntry {
                    body: resp.body.clone(),
                    content_type: resp.content_type.clone(),
                    inserted: Instant::now(),
                    ttl: ttl_for(path),
                    swr_grace: Duration::from_secs(SWR_GRACE_SECS),
                },
            );
            Ok(EdgeResponse {
                body: resp.body,
                content_type: resp.content_type,
                cache_status: "MISS",
            })
        }
    }

    fn build_cache_key(&self, path: &str, token: Option<&str>) -> String {
        if self.config.token_in_cache_key.load(Ordering::Relaxed) {
            if let Some(t) = token {
                return format!("{path}?token={t}");
            }
        }
        path.to_string()
    }

    pub async fn purge_all(&self) {
        self.cache.lock().await.clear();
        eprintln!("[EDGE] purge-all: cache cleared");
    }

    pub async fn purge_prefix(&self, prefix: &str) {
        let mut cache = self.cache.lock().await;
        let keys_to_remove: Vec<String> = cache
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, _)| k.clone())
            .collect();
        let count = keys_to_remove.len();
        for key in keys_to_remove {
            cache.pop(&key);
        }
        eprintln!("[EDGE] purge-prefix '{prefix}': removed {count} entries");
    }

    pub async fn cache_len(&self) -> usize {
        self.cache.lock().await.len()
    }
}

fn ttl_for(path: &str) -> Duration {
    if path.ends_with(".m3u8") {
        Duration::from_secs(MANIFEST_TTL_SECS)
    } else {
        Duration::from_secs(SEGMENT_TTL_SECS)
    }
}
