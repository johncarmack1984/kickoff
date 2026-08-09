import { invoke } from "@tauri-apps/api/core";
import Hls from "hls.js";

const $id = (id: string) => document.getElementById(id)!;

interface Stats {
  edge_hits: number;
  edge_misses: number;
  origin_fetches: number;
  swr_refreshes: number;
  collapsed: number;
  hit_ratio: number;
  collapse_ratio: number;
  cache_entries: number;
  origin_total_fetches: number;
  collapsing_enabled: boolean;
  token_in_cache_key: boolean;
  origin_latency_ms: number;
}

interface SimResult {
  viewers: number;
  total_requests: number;
  edge_hits: number;
  edge_misses: number;
  origin_fetches: number;
  collapsed: number;
  hit_ratio: number;
  collapse_ratio: number;
}

interface VarnishInfo {
  base_url: string;
  cache_port: number;
  origin_port: number;
}

interface VarnishStats {
  hits: number;
  misses: number;
  backend_fetches: number;
}

const hitHistory: number[] = [];
const originHistory: number[] = [];
let lastOriginFetches = 0;
let statsInterval: number | null = null;
let hls: Hls | null = null;
// Where the player loads the stream from. The simulated edge uses the custom
// `kickoff://` scheme; real-Varnish mode swaps in `http://127.0.0.1:<port>`.
let streamBase = "kickoff://localhost";
let varnishMode = false;

function log(msg: string, cls = "log-info") {
  const out = $id("log-output");
  const line = document.createElement("div");
  line.className = `log-line ${cls}`;
  line.textContent = `[${new Date().toLocaleTimeString()}] ${msg}`;
  out.appendChild(line);
  out.scrollTop = out.scrollHeight;
  if (out.children.length > 500) out.removeChild(out.firstChild!);
}

async function init() {
  try {
    const ver = await invoke<string>("probe_ffmpeg");
    $id("ffmpeg-status").textContent = ver;
    log(`ffmpeg: ${ver}`);
  } catch (e) {
    $id("ffmpeg-status").textContent = `${e}`;
    $id("ffmpeg-status").style.color = "#f87171";
    log(`ffmpeg check failed: ${e}`, "log-miss");
    return;
  }

  $id("btn-pick-file").addEventListener("click", async () => {
    try {
      const path = await invoke<string | null>("pick_video_file");
      if (path) {
        await startIngest(path);
      }
    } catch (e) {
      log(`File pick failed: ${e}`, "log-miss");
    }
  });

  $id("btn-use-testsrc").addEventListener("click", () => startIngest(null));

  $id("latency-slider").addEventListener("input", (e) => {
    const ms = parseInt((e.target as HTMLInputElement).value);
    $id("latency-value").textContent = String(ms);
    invoke("set_origin_latency", { ms });
  });

  $id("chk-collapsing").addEventListener("change", (e) => {
    const enabled = (e.target as HTMLInputElement).checked;
    invoke("toggle_collapsing", { enabled });
    log(`Request collapsing: ${enabled ? "ON" : "OFF"}`);
  });

  $id("chk-token-key").addEventListener("change", (e) => {
    const enabled = (e.target as HTMLInputElement).checked;
    invoke("toggle_token_in_cache_key", { enabled });
    log(`Token in cache key: ${enabled ? "ON" : "OFF"}`);
  });

  $id("chk-varnish").addEventListener("change", async (e) => {
    const on = (e.target as HTMLInputElement).checked;
    const statusEl = $id("varnish-status");
    if (on) {
      statusEl.textContent = "starting varnishd…";
      try {
        const info = await invoke<VarnishInfo>("varnish_start");
        streamBase = info.base_url;
        varnishMode = true;
        statusEl.textContent = `on — ${info.base_url} (origin :${info.origin_port})`;
        log(`Real Varnish started at ${info.base_url}`, "log-hit");
        startPlayer();
      } catch (err) {
        (e.target as HTMLInputElement).checked = false;
        varnishMode = false;
        statusEl.textContent = `failed: ${err}`;
        log(`Varnish start failed: ${err}`, "log-miss");
      }
    } else {
      try {
        await invoke("varnish_stop");
      } catch {
        // already gone
      }
      varnishMode = false;
      streamBase = "kickoff://localhost";
      statusEl.textContent = "off — uses the simulated edge";
      log("Real Varnish stopped; back to simulated edge");
      startPlayer();
    }
  });

  $id("btn-purge-all").addEventListener("click", async () => {
    await invoke("purge_all");
    log("Purge all: cache cleared");
  });

  $id("btn-purge-prefix").addEventListener("click", async () => {
    const prefix = (document.getElementById("purge-prefix") as HTMLInputElement).value;
    if (prefix) {
      await invoke("purge_prefix", { prefix });
      log(`Purge prefix: ${prefix}`);
    }
  });

  $id("btn-reset-stats").addEventListener("click", async () => {
    await invoke("reset_stats");
    hitHistory.length = 0;
    originHistory.length = 0;
    lastOriginFetches = 0;
    log("Stats reset");
  });

  $id("btn-flash-crowd").addEventListener("click", runFlashCrowd);
}

async function startIngest(path: string | null) {
  const status = $id("ingest-status");
  status.textContent = "Transcoding... this may take a moment.";
  status.style.color = "#facc15";
  log(`Ingest starting${path ? `: ${path}` : " (test source)"}...`);

  try {
    const result = await invoke<{ ready: boolean; message: string }>("start_ingest", { path });
    status.textContent = result.message;
    status.style.color = "#4ade80";
    log(result.message, "log-hit");

    $id("setup-panel").classList.add("hidden");
    $id("main-content").classList.remove("hidden");

    startPlayer();
    startStatsPolling();
  } catch (e) {
    status.textContent = `Failed: ${e}`;
    status.style.color = "#f87171";
    log(`Ingest failed: ${e}`, "log-miss");
  }
}

function startPlayer() {
  const video = $id("video") as HTMLVideoElement;
  const src = `${streamBase}/master.m3u8`;

  // Tear down any existing player so switching cache engines reloads cleanly.
  if (hls) {
    hls.destroy();
    hls = null;
  }

  if (Hls.isSupported()) {
    const player = new Hls({
      enableWorker: false,
      debug: false,
      maxBufferLength: 10,
      maxMaxBufferLength: 30,
    });
    hls = player;

    player.loadSource(src);
    player.attachMedia(video);

    player.on(Hls.Events.MANIFEST_PARSED, (_ev, data) => {
      log(`HLS: ${data.levels.length} quality levels parsed`);
      video.play().catch(() => {});
    });

    player.on(Hls.Events.LEVEL_SWITCHED, (_ev, data) => {
      const level = player.levels[data.level];
      const label = `${level.height}p @ ${Math.round(level.bitrate / 1000)}kbps`;
      $id("current-rendition").textContent = label;
      log(`ABR switch: ${label}`);
    });

    player.on(Hls.Events.ERROR, (_ev, data) => {
      if (data.fatal) {
        log(`HLS fatal error: ${data.type} / ${data.details}`, "log-miss");
      }
    });

    player.on(Hls.Events.FRAG_LOADED, (_ev, data) => {
      const frag = data.frag;
      log(`Segment loaded: level=${frag.level} sn=${frag.sn}`);
    });
  } else if (video.canPlayType("application/vnd.apple.mpegurl")) {
    video.src = src;
    video.addEventListener("loadedmetadata", () => video.play().catch(() => {}));
  } else {
    log("HLS not supported in this browser", "log-miss");
  }
}

function startStatsPolling() {
  if (statsInterval) clearInterval(statsInterval);
  statsInterval = window.setInterval(async () => {
    try {
      if (varnishMode) {
        const vs = await invoke<VarnishStats | null>("varnish_stats");
        if (vs) {
          const total = vs.hits + vs.misses;
          $id("stat-hits").textContent = String(vs.hits);
          $id("stat-misses").textContent = String(vs.misses);
          $id("stat-origin").textContent = String(vs.backend_fetches);
          // Not exposed by varnishstat's basic counters — blank them so stale
          // simulated numbers don't linger.
          $id("stat-collapsed").textContent = "—";
          $id("stat-swr").textContent = "—";
          $id("stat-collapse-ratio").textContent = "—";
          $id("stat-cache-entries").textContent = "—";
          const ratio = total > 0 ? vs.hits / total : 0;
          $id("stat-hit-ratio").textContent = total > 0 ? (ratio * 100).toFixed(1) + "%" : "—";

          hitHistory.push(ratio);
          if (hitHistory.length > 60) hitHistory.shift();
          drawSparkline("sparkline-hits", hitHistory, 0, 1, "#4ade80");

          const delta = vs.backend_fetches - lastOriginFetches;
          lastOriginFetches = vs.backend_fetches;
          originHistory.push(delta);
          if (originHistory.length > 60) originHistory.shift();
          drawSparkline("sparkline-origin", originHistory, 0, undefined, "#f87171");
        }
        return;
      }

      const s = await invoke<Stats>("get_stats");
      $id("stat-hits").textContent = String(s.edge_hits);
      $id("stat-misses").textContent = String(s.edge_misses);
      $id("stat-origin").textContent = String(s.origin_fetches);
      $id("stat-collapsed").textContent = String(s.collapsed);
      $id("stat-swr").textContent = String(s.swr_refreshes);
      $id("stat-hit-ratio").textContent = (s.hit_ratio * 100).toFixed(1) + "%";
      $id("stat-collapse-ratio").textContent = (s.collapse_ratio * 100).toFixed(1) + "%";
      $id("stat-cache-entries").textContent = String(s.cache_entries);

      hitHistory.push(s.hit_ratio);
      if (hitHistory.length > 60) hitHistory.shift();
      drawSparkline("sparkline-hits", hitHistory, 0, 1, "#4ade80");

      const delta = s.origin_fetches - lastOriginFetches;
      lastOriginFetches = s.origin_fetches;
      originHistory.push(delta);
      if (originHistory.length > 60) originHistory.shift();
      drawSparkline("sparkline-origin", originHistory, 0, undefined, "#f87171");
    } catch {
      // stats not available yet
    }
  }, 500);
}

function drawSparkline(
  canvasId: string,
  data: number[],
  min: number,
  max: number | undefined,
  color: string,
) {
  const canvas = document.getElementById(canvasId) as HTMLCanvasElement;
  const ctx = canvas.getContext("2d")!;
  const w = canvas.width;
  const h = canvas.height;

  ctx.clearRect(0, 0, w, h);
  if (data.length < 2) return;

  const actualMax = max ?? Math.max(...data, 1);
  const step = w / (data.length - 1);

  ctx.beginPath();
  ctx.strokeStyle = color;
  ctx.lineWidth = 1.5;

  for (let i = 0; i < data.length; i++) {
    const x = i * step;
    const y = h - ((data[i] - min) / (actualMax - min)) * (h - 4) - 2;
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  }
  ctx.stroke();
}

async function runFlashCrowd() {
  const countEl = $id("viewer-count") as HTMLInputElement;
  const count = parseInt(countEl.value) || 200;
  const statusEl = $id("sim-status");
  const btn = $id("btn-flash-crowd") as HTMLButtonElement;

  btn.disabled = true;
  statusEl.textContent = `Running flash crowd: ${count} viewers...`;
  log(`Flash crowd started: ${count} viewers`, "log-sim");

  try {
    const result = await invoke<SimResult>("run_flash_crowd", { viewerCount: count });
    statusEl.textContent = [
      `Done: ${result.total_requests} requests,`,
      `${result.origin_fetches} origin fetches,`,
      `hit ratio ${(result.hit_ratio * 100).toFixed(1)}%,`,
      `collapse ratio ${(result.collapse_ratio * 100).toFixed(1)}%`,
    ].join(" ");

    log(
      `Flash crowd result: ${result.total_requests} req, ` +
        `${result.edge_hits} hits, ${result.edge_misses} misses, ` +
        `${result.origin_fetches} origin, collapse=${(result.collapse_ratio * 100).toFixed(1)}%`,
      "log-sim",
    );
  } catch (e) {
    statusEl.textContent = `Failed: ${e}`;
    log(`Flash crowd failed: ${e}`, "log-miss");
  } finally {
    btn.disabled = false;
  }
}

init();
