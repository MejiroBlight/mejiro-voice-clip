use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Region {
    pub name: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Wav,
    Mp4,
}

fn ffmpeg_cmd(path: &Path) -> Command {
    let mut cmd = Command::new(path);
    #[cfg(windows)]
    {
        // Windows では Tauri メインスレッドから CLI プロセスを起動すると
        // コンソールウィンドウが表示されるため抑制する
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    cmd
}

/// ffmpeg バイナリが `path` で動作するか確認する。
pub fn probe_ffmpeg(path: &Path) -> bool {
    ffmpeg_cmd(path)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// リージョン範囲を WAV または MP4 としてエクスポートする。
pub fn extract_region(
    ffmpeg: &Path,
    input: &Path,
    output_dir: &Path,
    region: &Region,
    format: &ExportFormat,
) -> anyhow::Result<()> {
    let file_name = match format {
        ExportFormat::Wav => {
            let mut n = region.name.clone();
            if !n.to_lowercase().ends_with(".wav") { n.push_str(".wav"); }
            n
        }
        ExportFormat::Mp4 => {
            let mut n = region.name.clone();
            if !n.to_lowercase().ends_with(".mp4") { n.push_str(".mp4"); }
            n
        }
    };
    let out_path = output_dir.join(&file_name);

    // 共通引数: 入力・タイムレンジ
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-ss".into(), format!("{:.6}", region.start),
        "-to".into(), format!("{:.6}", region.end),
        "-i".into(), input.to_string_lossy().into_owned(),
    ];

    match format {
        ExportFormat::Wav => {
            args.extend([
                "-vn".into(),
                "-acodec".into(), "pcm_s16le".into(),
                "-ar".into(), "44100".into(),
            ]);
        }
        ExportFormat::Mp4 => {
            // 映像: libx264 (fast preset) / 音声: aac
            args.extend([
                "-vcodec".into(), "libx264".into(),
                "-preset".into(), "fast".into(),
                "-crf".into(), "18".into(),
                "-acodec".into(), "aac".into(),
                "-b:a".into(), "192k".into(),
                "-movflags".into(), "+faststart".into(),
            ]);
        }
    }

    args.push(out_path.to_string_lossy().into_owned());

    let status = ffmpeg_cmd(ffmpeg)
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to spawn ffmpeg for region export")?;

    if !status.success() {
        anyhow::bail!(
            "ffmpeg exited with status {} for region '{}'",
            status,
            region.name
        );
    }
    Ok(())
}

/// ファイルをチャンク単位でデコードし、ピークをコールバックで通知するストリーミング生成。
/// on_chunk(peaks, offset, total_peaks, duration_secs, is_done)
pub fn generate_peaks_streaming(
    ffmpeg: &Path,
    input: &Path,
    peaks_count: usize,
    chunk_secs: f64,
    cancel: &AtomicBool,
    mut on_chunk: impl FnMut(Vec<f32>, usize, usize, f64, bool),
) -> anyhow::Result<()> {
    let duration_secs = get_duration(ffmpeg, input).unwrap_or(0.0);

    // 44100 Hz モノラルでデコード
    const SAMPLE_RATE: f64 = 44100.0;
    let total_samples = (duration_secs * SAMPLE_RATE) as usize;

    let window_size = if total_samples > 0 && peaks_count > 0 {
        (total_samples / peaks_count).max(1)
    } else {
        (SAMPLE_RATE as usize / peaks_count.max(1)).max(1)
    };

    let total_peaks = if window_size > 0 && total_samples > 0 {
        (total_samples + window_size - 1) / window_size
    } else {
        peaks_count
    };

    let peaks_per_chunk =
        ((chunk_secs * SAMPLE_RATE) / window_size as f64).ceil() as usize;
    let peaks_per_chunk = peaks_per_chunk.max(1);

    // ffmpeg で 44100 Hz モノ f32le PCM を stdout に出力する
    let mut child = ffmpeg_cmd(ffmpeg)
        .args([
            "-i",
            &input.to_string_lossy(),
            "-vn",
            "-ac",
            "1",
            "-ar",
            &(SAMPLE_RATE as u32).to_string(),
            "-f",
            "f32le",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn ffmpeg for peak generation")?;

    let mut stdout = child.stdout.take().context("failed to get ffmpeg stdout")?;

    // 読み取りバッファ (f32 × 4096 サンプル分)
    let mut buf = vec![0u8; 4096 * 4];
    let mut window_max = 0.0f32;
    let mut window_frames = 0usize;
    let mut chunk: Vec<f32> = Vec::with_capacity(peaks_per_chunk);
    let mut global_offset = 0usize;

    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            return Ok(());
        }

        let n = match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                let _ = child.kill();
                return Err(e.into());
            }
        };

        let mut i = 0;
        while i + 4 <= n {
            let sample =
                f32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
            window_max = window_max.max(sample.abs());
            window_frames += 1;

            if window_frames >= window_size {
                chunk.push(window_max);
                window_max = 0.0;
                window_frames = 0;

                if chunk.len() >= peaks_per_chunk {
                    let offset = global_offset;
                    global_offset += chunk.len();
                    on_chunk(
                        std::mem::replace(&mut chunk, Vec::with_capacity(peaks_per_chunk)),
                        offset,
                        total_peaks,
                        duration_secs,
                        false,
                    );

                    if cancel.load(Ordering::Relaxed) {
                        let _ = child.kill();
                        return Ok(());
                    }
                }
            }
            i += 4;
        }
    }

    let _ = child.wait();

    if window_frames > 0 {
        chunk.push(window_max);
    }

    let offset = global_offset;
    on_chunk(chunk, offset, total_peaks, duration_secs, true);

    Ok(())
}

/// ffmpeg の stderr から音声の長さを取得する。
fn get_duration(ffmpeg: &Path, input: &Path) -> anyhow::Result<f64> {
    let output = ffmpeg_cmd(ffmpeg)
        .args(["-i", &input.to_string_lossy()])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("failed to run ffmpeg for duration probe")?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    parse_duration_from_ffmpeg(&stderr)
}

fn parse_duration_from_ffmpeg(stderr: &str) -> anyhow::Result<f64> {
    for line in stderr.lines() {
        if let Some(pos) = line.find("Duration: ") {
            let rest = &line[pos + 10..];
            let end = rest.find(',').unwrap_or(rest.len());
            let time_str = rest[..end].trim();
            return parse_hms(time_str);
        }
    }
    anyhow::bail!("duration not found in ffmpeg output")
}

fn parse_hms(s: &str) -> anyhow::Result<f64> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        anyhow::bail!("invalid time string: {s}");
    }
    let h: f64 = parts[0].parse().context("parsing hours")?;
    let m: f64 = parts[1].parse().context("parsing minutes")?;
    let sec: f64 = parts[2].parse().context("parsing seconds")?;
    Ok(h * 3600.0 + m * 60.0 + sec)
}
