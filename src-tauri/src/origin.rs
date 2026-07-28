use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

pub struct Origin {
    root: Mutex<Option<PathBuf>>,
    latency: AtomicU64,
    fetch_counts: Mutex<HashMap<String, u64>>,
    total_fetches: AtomicU64,
}

pub struct OriginResponse {
    pub body: Vec<u8>,
    pub content_type: String,
}

impl Origin {
    pub fn new() -> Self {
        Self {
            root: Mutex::new(None),
            latency: AtomicU64::new(150),
            fetch_counts: Mutex::new(HashMap::new()),
            total_fetches: AtomicU64::new(0),
        }
    }

    pub fn set_root(&self, root: PathBuf) {
        *self.root.lock().unwrap() = Some(root);
    }

    pub fn set_latency_ms(&self, ms: u64) {
        self.latency.store(ms, Ordering::Relaxed);
    }

    pub fn latency_ms(&self) -> u64 {
        self.latency.load(Ordering::Relaxed)
    }

    pub fn total_fetches(&self) -> u64 {
        self.total_fetches.load(Ordering::Relaxed)
    }

    pub fn reset_stats(&self) {
        self.total_fetches.store(0, Ordering::Relaxed);
        self.fetch_counts.lock().unwrap().clear();
    }

    pub fn fetch_count_for(&self, key: &str) -> u64 {
        self.fetch_counts.lock().unwrap().get(key).copied().unwrap_or(0)
    }

    pub async fn fetch(&self, path: &str) -> Result<OriginResponse, String> {
        let latency_ms = self.latency.load(Ordering::Relaxed);
        if latency_ms > 0 {
            tokio::time::sleep(Duration::from_millis(latency_ms)).await;
        }

        let root = self.root.lock().unwrap().clone()
            .ok_or_else(|| "Origin not initialized (no content ingested yet)".to_string())?;

        let file_path = root.join(path);
        if !file_path.starts_with(&root) {
            return Err("Path traversal blocked".into());
        }

        let body = std::fs::read(&file_path)
            .map_err(|e| format!("Origin read {}: {e}", file_path.display()))?;

        let content_type = content_type_for(&file_path);

        self.total_fetches.fetch_add(1, Ordering::Relaxed);
        {
            let mut counts = self.fetch_counts.lock().unwrap();
            *counts.entry(path.to_string()).or_insert(0) += 1;
        }

        Ok(OriginResponse { body, content_type })
    }
}

fn content_type_for(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("m3u8") => "application/vnd.apple.mpegurl".into(),
        Some("m4s") => "video/iso.segment".into(),
        Some("mp4") => "video/mp4".into(),
        _ => "application/octet-stream".into(),
    }
}
