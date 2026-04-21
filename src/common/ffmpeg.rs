use ffmpeg_next as ffmpeg;
use std::{
    env,
    io,
    path::{Path as FsPath},
    process::{Command, Stdio},
    sync::OnceLock,
    time::Duration,
};



static FFMPEG_INIT: OnceLock<Result<(), String>> = OnceLock::new();
static AUTO_GPU_ENCODER: OnceLock<Option<String>> = OnceLock::new();

fn io_other<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}

pub fn ensure_ffmpeg_init() -> Result<(), io::Error> {
    FFMPEG_INIT
        .get_or_init(|| ffmpeg::init().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| io_other(format!("ffmpeg init failed: {e}")))?;
    Ok(())
}

fn detect_gpu_encoder() -> Option<String> {
    let output = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-encoders")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let encoders = String::from_utf8_lossy(&output.stdout);
    if encoders.contains("h264_nvenc") {
        Some("h264_nvenc".to_string())
    } else {
        None
    }
}

fn preferred_gpu_encoder() -> Option<String> {
    AUTO_GPU_ENCODER.get_or_init(detect_gpu_encoder).clone()
}

fn run_ffmpeg_encode(
    input_path: &FsPath,
    output_path: &FsPath,
    input_fps: u32,
    encoder: &str,
) -> Result<(), io::Error> {
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-f")
        .arg("hevc")
        .arg("-framerate")
        .arg(input_fps.to_string())
        .arg("-fflags")
        .arg("+genpts")
        .arg("-i")
        .arg(input_path)
        .arg("-an")
        .arg("-c:v")
        .arg(encoder)
        .arg("-pix_fmt")
        .arg("yuv420p");

    match encoder {
        "libx264" => {
            cmd.arg("-preset").arg("veryfast");
        }
        "h264_nvenc" => {
            // Balanced NVENC preset across recent NVIDIA generations.
            cmd.arg("-preset").arg("p4");
        }
        _ => {}
    }

    let ffmpeg_output = cmd
        .arg("-movflags")
        .arg("+faststart")
        .arg(output_path)
        .output()
        .map_err(io_other)?;

    if !ffmpeg_output.status.success() {
        return Err(io_other(String::from_utf8_lossy(&ffmpeg_output.stderr)));
    }

    Ok(())
}

/// Convert a concatenated raw HEVC elementary stream into a broadly compatible MP4.
/// Uses ffmpeg CLI for container/timestamp handling and H.264 output.
/// Tries GPU encoding first (when available), then falls back to CPU.
pub fn remux_hevc_to_mp4(
    input_path: &FsPath,
    output_path: &FsPath,
    input_fps: u32,
) -> Result<(), io::Error> {
    let mut encoders_to_try: Vec<String> = Vec::new();

    if let Ok(forced) = env::var("FFMPEG_ENCODER") {
        let trimmed = forced.trim();
        if !trimmed.is_empty() {
            encoders_to_try.push(trimmed.to_string());
        }
    } else if let Some(gpu_encoder) = preferred_gpu_encoder() {
        encoders_to_try.push(gpu_encoder);
    }

    if !encoders_to_try.iter().any(|enc| enc == "libx264") {
        encoders_to_try.push("libx264".to_string());
    }

    let mut errors: Vec<String> = Vec::new();
    for encoder in encoders_to_try {
        match run_ffmpeg_encode(input_path, output_path, input_fps, &encoder) {
            Ok(()) => return Ok(()),
            Err(e) => {
                errors.push(format!("{encoder}: {e}"));
            }
        }
    }

    Err(io_other(format!(
        "ffmpeg encode failed for all encoders ({})",
        errors.join(" | ")
    )))
}

pub async fn probe_duration_seconds(
    input_path: &FsPath,
    timeout: Duration,
) -> Result<f32, io::Error> {
    let mut cmd = tokio::process::Command::new("ffprobe");
    cmd.arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .arg("-of")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(input_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| io_other("ffprobe timed out"))?
        .map_err(io_other)?;

    if !output.status.success() {
        return Err(io_other(format!(
            "ffprobe failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let duration_raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let duration: f32 = duration_raw
        .parse()
        .map_err(|e| io_other(format!("ffprobe returned invalid duration `{duration_raw}`: {e}")))?;

    if !duration.is_finite() || duration.is_sign_negative() {
        return Err(io_other(format!(
            "ffprobe returned non-finite duration `{duration}`"
        )));
    }

    Ok(duration)
}
