mod edge;
mod ingest;
mod origin;
mod simulator;
mod varnish;

use edge::EdgeCache;
use http::header::*;
use http::response::Builder as ResponseBuilder;
use http::StatusCode;
use http_range::HttpRange;
use origin::Origin;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::Manager;

struct AppState {
    edge: Arc<EdgeCache>,
    origin: Arc<Origin>,
    /// The running real-Varnish session, if the user has started one. `None` in
    /// the default simulated mode.
    varnish: tokio::sync::Mutex<Option<varnish::VarnishSession>>,
}

#[derive(Serialize)]
struct VarnishInfo {
    /// Base URL the player should load the stream from.
    base_url: String,
    cache_port: u16,
    origin_port: u16,
}

#[derive(Serialize)]
struct Stats {
    edge_hits: u64,
    edge_misses: u64,
    origin_fetches: u64,
    swr_refreshes: u64,
    collapsed: u64,
    hit_ratio: f64,
    collapse_ratio: f64,
    cache_entries: usize,
    origin_total_fetches: u64,
    collapsing_enabled: bool,
    token_in_cache_key: bool,
    origin_latency_ms: u64,
}

#[derive(Clone, Serialize)]
struct IngestStatus {
    ready: bool,
    message: String,
}

#[tauri::command]
fn probe_ffmpeg() -> Result<String, String> {
    ingest::probe_ffmpeg()
}

#[tauri::command]
async fn pick_video_file() -> Result<Option<String>, String> {
    tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .add_filter("Video", &["mp4", "mov", "mkv", "avi", "webm"])
            .pick_file()
            .map(|p| p.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn start_ingest(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    path: Option<String>,
) -> Result<IngestStatus, String> {
    let output_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?
        .join("output");

    let input = path.map(PathBuf::from);
    let result = tokio::task::spawn_blocking(move || {
        ingest::transcode(input.as_deref(), &output_dir)
    })
    .await
    .map_err(|e| format!("join: {e}"))??;

    state.origin.set_root(result.output_dir.clone());

    Ok(IngestStatus {
        ready: true,
        message: format!("Ingested. Master playlist: {}", result.master_playlist),
    })
}

#[tauri::command]
async fn get_stats(state: tauri::State<'_, AppState>) -> Result<Stats, String> {
    let hits = state.edge.stats.hits.load(Ordering::Relaxed);
    let misses = state.edge.stats.misses.load(Ordering::Relaxed);
    let origin_fetches = state.edge.stats.origin_fetches.load(Ordering::Relaxed);
    let total = hits + misses;

    Ok(Stats {
        edge_hits: hits,
        edge_misses: misses,
        origin_fetches,
        swr_refreshes: state.edge.stats.swr_refreshes.load(Ordering::Relaxed),
        collapsed: state.edge.stats.collapsed.load(Ordering::Relaxed),
        hit_ratio: if total > 0 {
            hits as f64 / total as f64
        } else {
            0.0
        },
        collapse_ratio: if misses > 0 {
            1.0 - (origin_fetches as f64 / misses as f64)
        } else {
            0.0
        },
        cache_entries: state.edge.cache_len().await,
        origin_total_fetches: state.origin.total_fetches(),
        collapsing_enabled: state
            .edge
            .config
            .collapsing_enabled
            .load(Ordering::Relaxed),
        token_in_cache_key: state
            .edge
            .config
            .token_in_cache_key
            .load(Ordering::Relaxed),
        origin_latency_ms: state.origin.latency_ms(),
    })
}

#[tauri::command]
fn set_origin_latency(state: tauri::State<'_, AppState>, ms: u64) {
    state.origin.set_latency_ms(ms);
}

#[tauri::command]
fn toggle_collapsing(state: tauri::State<'_, AppState>, enabled: bool) {
    state
        .edge
        .config
        .collapsing_enabled
        .store(enabled, Ordering::Relaxed);
}

#[tauri::command]
fn toggle_token_in_cache_key(state: tauri::State<'_, AppState>, enabled: bool) {
    state
        .edge
        .config
        .token_in_cache_key
        .store(enabled, Ordering::Relaxed);
}

#[tauri::command]
async fn purge_all(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.edge.purge_all().await;
    Ok(())
}

#[tauri::command]
async fn purge_prefix(state: tauri::State<'_, AppState>, prefix: String) -> Result<(), String> {
    state.edge.purge_prefix(&prefix).await;
    Ok(())
}

#[tauri::command]
async fn reset_stats(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.edge.stats.reset();
    state.origin.reset_stats();
    Ok(())
}

#[tauri::command]
async fn run_flash_crowd(
    state: tauri::State<'_, AppState>,
    viewer_count: u32,
) -> Result<simulator::SimulatorResult, String> {
    let edge = state.edge.clone();
    Ok(simulator::run_flash_crowd(edge, viewer_count, "master.m3u8").await)
}

#[tauri::command]
async fn varnish_start(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<VarnishInfo, String> {
    let workdir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?
        .join("varnish");

    let mut guard = state.varnish.lock().await;
    // Drop any previous session first (its Drop kills the old varnishd).
    *guard = None;

    let session = varnish::start(state.origin.clone(), workdir).await?;
    let info = VarnishInfo {
        base_url: session.cache_url.clone(),
        cache_port: session.cache_port,
        origin_port: session.origin_port,
    };
    *guard = Some(session);
    Ok(info)
}

#[tauri::command]
async fn varnish_stop(state: tauri::State<'_, AppState>) -> Result<(), String> {
    *state.varnish.lock().await = None;
    Ok(())
}

#[tauri::command]
async fn varnish_stats(
    state: tauri::State<'_, AppState>,
) -> Result<Option<varnish::VarnishStats>, String> {
    match state.varnish.lock().await.as_ref() {
        Some(session) => Ok(Some(session.stats()?)),
        None => Ok(None),
    }
}

fn handle_protocol_request(
    edge: Arc<EdgeCache>,
    request: http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    tauri::async_runtime::spawn(async move {
        if request.method() == http::Method::OPTIONS {
            let resp = ResponseBuilder::new()
                .status(StatusCode::NO_CONTENT)
                .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .header(ACCESS_CONTROL_ALLOW_METHODS, "GET, HEAD, OPTIONS")
                .header(ACCESS_CONTROL_ALLOW_HEADERS, "Range")
                .header(ACCESS_CONTROL_MAX_AGE, "86400")
                .body(Vec::new())
                .unwrap();
            responder.respond(resp);
            return;
        }

        let uri = request.uri().to_string();
        let raw_path = request.uri().path();
        let decoded_path = percent_encoding::percent_decode(raw_path.as_bytes())
            .decode_utf8_lossy()
            .to_string();
        let path = decoded_path.trim_start_matches('/');

        let query = request.uri().query().unwrap_or("");
        let token = extract_token(query);

        let edge_result = edge.get(path, token.as_deref()).await;

        match edge_result {
            Ok(edge_resp) => {
                let response = build_range_response(
                    &request,
                    &edge_resp.body,
                    &edge_resp.content_type,
                    edge_resp.cache_status,
                );
                responder.respond(response);
            }
            Err(e) => {
                eprintln!("[PROTOCOL] error for {uri}: {e}");
                let resp = ResponseBuilder::new()
                    .status(StatusCode::NOT_FOUND)
                    .header(CONTENT_TYPE, "text/plain")
                    .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                    .body(e.into_bytes())
                    .unwrap();
                responder.respond(resp);
            }
        }
    });
}

fn build_range_response(
    request: &http::Request<Vec<u8>>,
    full_body: &[u8],
    content_type: &str,
    cache_status: &str,
) -> http::Response<Vec<u8>> {
    let len = full_body.len() as u64;
    let resp = ResponseBuilder::new()
        .header(CONTENT_TYPE, content_type)
        .header(ACCEPT_RANGES, "bytes")
        .header("X-Cache", cache_status)
        .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(ACCESS_CONTROL_EXPOSE_HEADERS, "Content-Range, Content-Length, X-Cache");

    if let Some(range_header) = request.headers().get("range") {
        if let Ok(range_str) = range_header.to_str() {
            if let Ok(ranges) = HttpRange::parse(range_str, len) {
                if let Some(range) = ranges.first() {
                    let start = range.start as usize;
                    let end = (range.start + range.length) as usize;
                    let end = end.min(full_body.len());
                    let slice = &full_body[start..end];

                    return resp
                        .status(StatusCode::PARTIAL_CONTENT)
                        .header(
                            CONTENT_RANGE,
                            format!("bytes {}-{}/{len}", start, end - 1),
                        )
                        .header(CONTENT_LENGTH, slice.len())
                        .body(slice.to_vec())
                        .unwrap();
                }
            }
        }

        return resp
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(CONTENT_RANGE, format!("bytes */{len}"))
            .body(Vec::new())
            .unwrap();
    }

    resp.header(CONTENT_LENGTH, len)
        .body(full_body.to_vec())
        .unwrap()
}

fn extract_token(query: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next()?;
        let value = parts.next()?;
        if key == "token" {
            Some(value.to_string())
        } else {
            None
        }
    })
}

pub fn run() {
    let origin = Arc::new(Origin::new());
    let edge = Arc::new(EdgeCache::new(origin.clone()));

    let edge_for_protocol = edge.clone();

    tauri::Builder::default()
        .manage(AppState {
            edge: edge.clone(),
            origin: origin.clone(),
            varnish: tokio::sync::Mutex::new(None),
        })
        .register_asynchronous_uri_scheme_protocol(
            "kickoff",
            move |_ctx, request, responder| {
                handle_protocol_request(edge_for_protocol.clone(), request, responder);
            },
        )
        .invoke_handler(tauri::generate_handler![
            probe_ffmpeg,
            pick_video_file,
            start_ingest,
            get_stats,
            set_origin_latency,
            toggle_collapsing,
            toggle_token_in_cache_key,
            purge_all,
            purge_prefix,
            reset_stats,
            run_flash_crowd,
            varnish_start,
            varnish_stop,
            varnish_stats,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
