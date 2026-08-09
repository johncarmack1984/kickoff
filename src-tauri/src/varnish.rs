//! Optional "real Varnish" mode.
//!
//! The default lab (`edge.rs`) *simulates* an edge cache so its behavior is
//! visible on screen. This module instead runs an **actual `varnishd`** in
//! front of an **actual HTTP origin**, both driven by the same transcoded
//! content, loading the `kickoff_edge` VMOD (`../edge-vmod`) so the exact policy
//! the lab visualizes runs for real. The player is pointed at Varnish's port,
//! and stats come from `varnishstat` instead of the in-app counters.
//!
//! It's entirely opt-in: nothing here runs unless the UI asks for it, and the
//! simulated path is untouched. If `varnishd` or the built VMOD is missing, the
//! start command returns a clear error the UI surfaces — the lab keeps working.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::origin::Origin;

/// A running `varnishd` plus the HTTP origin server feeding it. Dropping this
/// tears both down (the origin task is aborted when its handle drops; the
/// child is killed here).
pub struct VarnishSession {
    child: Child,
    workdir: PathBuf,
    origin_task: tauri::async_runtime::JoinHandle<()>,
    /// Base URL the player should load from, e.g. `http://127.0.0.1:6081`.
    pub cache_url: String,
    pub cache_port: u16,
    pub origin_port: u16,
}

impl Drop for VarnishSession {
    fn drop(&mut self) {
        self.origin_task.abort();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Serialize)]
pub struct VarnishStats {
    pub hits: u64,
    pub misses: u64,
    pub backend_fetches: u64,
}

/// Boot the origin server, then `varnishd`, and wait until it accepts
/// connections. `workdir` holds the generated VCL and Varnish's VSM directory.
pub async fn start(origin: Arc<Origin>, workdir: PathBuf) -> Result<VarnishSession, String> {
    let varnishd = which("varnishd")
        .ok_or("varnishd not found on PATH — install it with `brew install varnish`")?;
    let vmod = locate_vmod().ok_or(
        "kickoff_edge VMOD not found — run `cargo build --release` in kickoff/edge-vmod, \
         or set KICKOFF_VMOD_PATH to the built libvmod_kickoff_edge.{dylib,so}",
    )?;

    // 1. Real HTTP origin on an ephemeral port, wrapping the same Origin the
    //    simulated edge uses (disk content + artificial latency + counters).
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| format!("bind origin: {e}"))?;
    let origin_port = listener
        .local_addr()
        .map_err(|e| format!("origin addr: {e}"))?
        .port();
    let origin_task = spawn_origin_server(listener, origin);

    // 2. Pick a cache port and write the VCL that wires in the VMOD.
    let cache_port = free_port().await?;
    std::fs::create_dir_all(&workdir).map_err(|e| format!("workdir: {e}"))?;
    let vcl_path = workdir.join("kickoff.vcl");
    std::fs::write(&vcl_path, generate_vcl(&vmod, origin_port))
        .map_err(|e| format!("write vcl: {e}"))?;
    let vsm = workdir.join("vsm");

    // 3. Spawn varnishd in the foreground so this process owns its lifecycle.
    let mut child = Command::new(&varnishd)
        .arg("-F")
        .args(["-a", &format!("127.0.0.1:{cache_port}")])
        .arg("-f")
        .arg(&vcl_path)
        .arg("-n")
        .arg(&vsm)
        .args(["-s", "malloc,64m"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn varnishd: {e}"))?;

    // 4. Health-check: connect to the cache port, but bail early (with the
    //    child's stderr) if varnishd exits — e.g. a VCL/VMOD-ABI error.
    let addr = format!("127.0.0.1:{cache_port}");
    let mut ready = false;
    let mut last_err = String::new();
    for _ in 0..60 {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "varnishd exited before becoming ready ({status}): {}",
                drain_stderr(&mut child).trim()
            ));
        }
        match TcpStream::connect(&addr).await {
            Ok(_) => {
                ready = true;
                break;
            }
            Err(e) => {
                last_err = e.to_string();
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    if !ready {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("varnishd did not become ready: {last_err}"));
    }

    // Drain varnishd's stderr in the background so its pipe never fills.
    if let Some(stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                eprintln!("[varnishd] {line}");
            }
        });
    }

    Ok(VarnishSession {
        child,
        workdir,
        origin_task,
        cache_url: format!("http://{addr}"),
        cache_port,
        origin_port,
    })
}

impl VarnishSession {
    /// Read live counters from `varnishstat -1 -j` for this instance.
    pub fn stats(&self) -> Result<VarnishStats, String> {
        let varnishstat = which("varnishstat").ok_or("varnishstat not found on PATH")?;
        let out = Command::new(varnishstat)
            .arg("-n")
            .arg(self.workdir.join("vsm"))
            .args(["-1", "-j"])
            .output()
            .map_err(|e| format!("run varnishstat: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        parse_varnishstat(&out.stdout)
    }
}

/// A minimal HTTP/1.1 origin: reads a request, serves the object from `Origin`
/// (full body — Varnish handles ranges itself), and closes the connection.
/// GET/HEAD only, which is all Varnish sends to a backend for these objects.
fn spawn_origin_server(
    listener: TcpListener,
    origin: Arc<Origin>,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let origin = origin.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = serve_conn(stream, origin).await {
                            eprintln!("[varnish-origin] connection error: {e}");
                        }
                    });
                }
                Err(e) => {
                    eprintln!("[varnish-origin] accept failed: {e}");
                    break;
                }
            }
        }
    })
}

async fn serve_conn(mut stream: TcpStream, origin: Arc<Origin>) -> std::io::Result<()> {
    // Read until end of headers. Bodies aren't expected on GET/HEAD.
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 64 * 1024 {
            break;
        }
    }

    let head = String::from_utf8_lossy(&buf);
    let req_line = head.lines().next().unwrap_or("");
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/");

    // Path only; Origin keys on path and ignores the query string.
    let raw_path = target.split(['?', '#']).next().unwrap_or("/");
    let decoded = percent_encoding::percent_decode(raw_path.as_bytes())
        .decode_utf8_lossy()
        .to_string();
    let path = decoded.trim_start_matches('/').to_string();

    let response = match origin.fetch(&path).await {
        Ok(r) => {
            let mut bytes = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
                 Accept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                r.content_type,
                r.body.len()
            )
            .into_bytes();
            if method != "HEAD" {
                bytes.extend_from_slice(&r.body);
            }
            bytes
        }
        Err(e) => {
            let body = e.into_bytes();
            let mut bytes = format!(
                "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .into_bytes();
            if method != "HEAD" {
                bytes.extend_from_slice(&body);
            }
            bytes
        }
    };

    stream.write_all(&response).await?;
    stream.flush().await?;
    Ok(())
}

fn generate_vcl(vmod: &Path, origin_port: u16) -> String {
    // Same policy as edge-vmod/example.vcl, with the VMOD loaded by absolute
    // path and CORS added so the webview player can read it. `import ... from`
    // an absolute path avoids needing -p vmod_path.
    format!(
        r#"vcl 4.1;

import kickoff_edge from "{vmod}";

backend origin {{
    .host = "127.0.0.1";
    .port = "{origin_port}";
}}

sub vcl_hash {{
    hash_data(kickoff_edge.normalize_key(req.url));
    return (lookup);
}}

sub vcl_backend_response {{
    set beresp.ttl = kickoff_edge.ttl(bereq.url);
    set beresp.grace = kickoff_edge.swr_grace();
    set beresp.http.X-Kickoff-Class = kickoff_edge.class(bereq.url);
}}

sub vcl_deliver {{
    set resp.http.X-Cache = obj.hits > 0 ? "HIT" : "MISS";
    set resp.http.Access-Control-Allow-Origin = "*";
    set resp.http.Access-Control-Expose-Headers = "X-Cache, X-Kickoff-Class, Content-Length, Content-Range";
}}
"#,
        vmod = vmod.display(),
        origin_port = origin_port
    )
}

fn parse_varnishstat(bytes: &[u8]) -> Result<VarnishStats, String> {
    let v: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|e| format!("parse varnishstat json: {e}"))?;
    // Varnish >= 6.5 nests counters under "counters"; older versions put them
    // at the top level alongside "version"/"timestamp".
    let counters = v.get("counters").unwrap_or(&v);
    let get = |key: &str| -> u64 {
        counters
            .get(key)
            .and_then(|c| c.get("value").or(Some(c)))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    Ok(VarnishStats {
        hits: get("MAIN.cache_hit"),
        misses: get("MAIN.cache_miss"),
        backend_fetches: get("MAIN.backend_fetch"),
    })
}

/// Read whatever varnishd has written to stderr so far (best effort).
fn drain_stderr(child: &mut Child) -> String {
    let mut s = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut s);
    }
    s
}

/// Find an executable on PATH, falling back to the usual Homebrew locations
/// (varnishd lives in `sbin`, which GUI apps often don't inherit).
fn which(bin: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let cand = dir.join(bin);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    for p in [
        "/opt/homebrew/sbin",
        "/opt/homebrew/bin",
        "/usr/local/sbin",
        "/usr/local/bin",
    ] {
        let cand = Path::new(p).join(bin);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// Locate the built VMOD. Honors `KICKOFF_VMOD_PATH`, else searches the
/// `edge-vmod/target/release` output relative to the cwd and the executable.
fn locate_vmod() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("KICKOFF_VMOD_PATH") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    let names = ["libvmod_kickoff_edge.dylib", "libvmod_kickoff_edge.so"];
    let subs = [
        "edge-vmod/target/release",
        "../edge-vmod/target/release",
        "../../edge-vmod/target/release",
    ];

    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(Path::to_path_buf);
        for _ in 0..6 {
            if let Some(d) = dir {
                roots.push(d.clone());
                dir = d.parent().map(Path::to_path_buf);
            }
        }
    }

    for root in roots {
        for sub in subs {
            for name in names {
                let cand = root.join(sub).join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

/// Grab a free TCP port by binding to :0 and releasing it. Racy but fine for a
/// local dev tool.
async fn free_port() -> Result<u16, String> {
    let l = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| format!("pick port: {e}"))?;
    l.local_addr()
        .map(|a| a.port())
        .map_err(|e| format!("port addr: {e}"))
}
