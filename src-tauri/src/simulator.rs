use crate::edge::EdgeCache;
use serde::Serialize;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

#[derive(Clone, Serialize)]
pub struct SimulatorResult {
    pub viewers: u32,
    pub total_requests: u64,
    pub edge_hits: u64,
    pub edge_misses: u64,
    pub origin_fetches: u64,
    pub collapsed: u64,
    pub hit_ratio: f64,
    pub collapse_ratio: f64,
}

pub async fn run_flash_crowd(
    edge: Arc<EdgeCache>,
    viewer_count: u32,
    manifest_path: &str,
) -> SimulatorResult {
    let segments = discover_segments(edge.clone(), manifest_path).await;
    edge.purge_all().await;
    edge.stats.reset();

    let mut handles = Vec::new();
    let spread = Duration::from_secs(10);
    let interval = if viewer_count > 1 {
        spread / (viewer_count - 1)
    } else {
        Duration::ZERO
    };

    for i in 0..viewer_count {
        let edge = edge.clone();
        let segments = segments.clone();
        let delay = interval * i;
        let token = format!("viewer-{i}");

        handles.push(tauri::async_runtime::spawn(async move {
            sleep(delay).await;
            for seg in &segments {
                let _ = edge.get(seg, Some(&token)).await;
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }

    let hits = edge.stats.hits.load(std::sync::atomic::Ordering::Relaxed);
    let misses = edge.stats.misses.load(std::sync::atomic::Ordering::Relaxed);
    let origin_fetches = edge.stats.origin_fetches.load(std::sync::atomic::Ordering::Relaxed);
    let collapsed = edge.stats.collapsed.load(std::sync::atomic::Ordering::Relaxed);
    let total = hits + misses;

    SimulatorResult {
        viewers: viewer_count,
        total_requests: total,
        edge_hits: hits,
        edge_misses: misses,
        origin_fetches,
        collapsed,
        hit_ratio: if total > 0 { hits as f64 / total as f64 } else { 0.0 },
        collapse_ratio: if misses > 0 {
            1.0 - (origin_fetches as f64 / misses as f64)
        } else {
            0.0
        },
    }
}

async fn discover_segments(edge: Arc<EdgeCache>, master_path: &str) -> Vec<String> {
    let mut paths = vec![master_path.to_string()];

    let master_resp = edge.get(master_path, None).await;
    let master_body = match master_resp {
        Ok(r) => String::from_utf8_lossy(&r.body).to_string(),
        Err(_) => return paths,
    };

    let master_dir = if let Some(pos) = master_path.rfind('/') {
        &master_path[..=pos]
    } else {
        ""
    };

    let variant_playlists: Vec<String> = master_body
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| format!("{master_dir}{l}"))
        .collect();

    for variant in &variant_playlists {
        paths.push(variant.clone());
        if let Ok(resp) = edge.get(variant, None).await {
            let body = String::from_utf8_lossy(&resp.body);
            let variant_dir = if let Some(pos) = variant.rfind('/') {
                &variant[..=pos]
            } else {
                ""
            };
            for line in body.lines() {
                if !line.starts_with('#') && !line.is_empty() {
                    paths.push(format!("{variant_dir}{line}"));
                }
            }
        }
    }

    paths
}
