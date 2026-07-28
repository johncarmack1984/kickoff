use std::path::{Path, PathBuf};
use std::process::Command;

pub fn probe_ffmpeg() -> Result<String, String> {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map_err(|_| "ffmpeg not found. Install it (brew install ffmpeg) and restart.".into())
        .and_then(|out| {
            if out.status.success() {
                let version = String::from_utf8_lossy(&out.stdout);
                let first_line = version.lines().next().unwrap_or("ffmpeg");
                Ok(first_line.to_string())
            } else {
                Err("ffmpeg found but returned an error.".into())
            }
        })
}

pub struct IngestResult {
    pub output_dir: PathBuf,
    pub master_playlist: String,
}

struct Rung {
    height: u32,
    bitrate_kbps: u32,
}

const LADDER: &[Rung] = &[
    Rung { height: 240, bitrate_kbps: 400 },
    Rung { height: 480, bitrate_kbps: 1000 },
    Rung { height: 720, bitrate_kbps: 2500 },
];

pub fn transcode(input: Option<&Path>, output_dir: &Path) -> Result<IngestResult, String> {
    std::fs::create_dir_all(output_dir).map_err(|e| format!("Failed to create output dir: {e}"))?;

    for rung in LADDER {
        let rung_dir = output_dir.join(format!("{}p", rung.height));
        std::fs::create_dir_all(&rung_dir).map_err(|e| format!("mkdir: {e}"))?;
        transcode_rung(input, &rung_dir, rung)?;
    }

    let master = build_master_playlist(input, output_dir)?;
    let master_path = output_dir.join("master.m3u8");
    std::fs::write(&master_path, &master).map_err(|e| format!("write master playlist: {e}"))?;

    Ok(IngestResult {
        output_dir: output_dir.to_path_buf(),
        master_playlist: "master.m3u8".into(),
    })
}

fn transcode_rung(input: Option<&Path>, rung_dir: &Path, rung: &Rung) -> Result<(), String> {
    let bitrate = format!("{}k", rung.bitrate_kbps);
    let bufsize = format!("{}k", rung.bitrate_kbps * 2);
    let playlist = rung_dir.join("playlist.m3u8");
    let seg_pattern = rung_dir.join("seg_%03d.m4s");
    let init_name = "init.mp4";

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y");

    if let Some(path) = input {
        cmd.args(["-i", &path.to_string_lossy()]);
    } else {
        cmd.args([
            "-f", "lavfi", "-i",
            &format!("testsrc=duration=30:size=1280x720:rate=30"),
            "-f", "lavfi", "-i", "anullsrc=r=44100:cl=stereo",
            "-t", "30",
        ]);
    }

    cmd.args([
        "-vf", &format!("scale=-2:{}", rung.height),
        "-pix_fmt", "yuv420p",
        "-c:v", "libx264",
        "-preset", "veryfast",
        "-b:v", &bitrate,
        "-maxrate", &bitrate,
        "-bufsize", &bufsize,
        "-g", "96",
        "-keyint_min", "96",
        "-sc_threshold", "0",
    ]);

    if input.is_some() {
        cmd.args(["-c:a", "aac", "-b:a", "128k", "-ac", "2"]);
    } else {
        cmd.args(["-c:a", "aac", "-b:a", "64k"]);
    }

    cmd.args([
        "-f", "hls",
        "-hls_time", "4",
        "-hls_segment_type", "fmp4",
        "-hls_fmp4_init_filename", init_name,
        "-hls_segment_filename", &seg_pattern.to_string_lossy(),
        "-hls_playlist_type", "vod",
        &playlist.to_string_lossy(),
    ]);

    let output = cmd.output().map_err(|e| format!("ffmpeg exec: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffmpeg failed for {}p: {stderr}", rung.height));
    }
    Ok(())
}

fn probe_resolution(input: Option<&Path>) -> (u32, u32) {
    if let Some(path) = input {
        let output = Command::new("ffprobe")
            .args([
                "-v", "error",
                "-select_streams", "v:0",
                "-show_entries", "stream=width,height",
                "-of", "csv=p=0:s=x",
                &path.to_string_lossy(),
            ])
            .output();
        if let Ok(out) = output {
            let s = String::from_utf8_lossy(&out.stdout);
            let parts: Vec<&str> = s.trim().split('x').collect();
            if parts.len() == 2 {
                if let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
                    return (w, h);
                }
            }
        }
    }
    (1280, 720)
}

fn build_master_playlist(input: Option<&Path>, _output_dir: &Path) -> Result<String, String> {
    let (src_w, src_h) = probe_resolution(input);
    let aspect = src_w as f64 / src_h as f64;

    let mut lines = vec!["#EXTM3U".to_string()];
    for rung in LADDER {
        let h = rung.height;
        let w = ((h as f64 * aspect) as u32 / 2) * 2;
        let bw = rung.bitrate_kbps * 1000;
        lines.push(format!(
            "#EXT-X-STREAM-INF:BANDWIDTH={bw},RESOLUTION={w}x{h}"
        ));
        lines.push(format!("{h}p/playlist.m3u8"));
    }
    Ok(lines.join("\n") + "\n")
}
