use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use tauri::{AppHandle, Emitter, State};

mod ffmpeg_extractor;
use ffmpeg_extractor::Region;

// ---- 共有状態 ---------------------------------------------------------------

type SharedVideoPath = Arc<Mutex<Option<PathBuf>>>;

/// 現在開いている動画ファイルのパスを保持する。
/// カスタムプロトコルハンドラから Arc で共有する。
struct VideoState(SharedVideoPath);

/// 進行中のピーク生成タスクにキャンセルを伝えるフラグ。
/// 新しい生成が始まるのたびに旧フラグを true にして上書きする。
struct PeakCancelState(Mutex<Arc<AtomicBool>>);

/// 使用する ffmpeg バイナリのパス。
/// "ffmpeg" のままならシステム PATH から解決される。
struct FfmpegState(Mutex<Option<PathBuf>>);

// ---- コマンド ----------------------------------------------------------------

/// フロントから呼ばれる: ストリーム配信するファイルを登録する。
#[tauri::command]
fn set_video_path(state: State<VideoState>, path: String) -> Result<(), String> {
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    *guard = if path.is_empty() {
        None
    } else {
        let p = PathBuf::from(&path);
        if !p.exists() {
            return Err(format!("file not found: {path}"));
        }
        Some(p)
    };
    Ok(())
}

use serde::Serialize;

#[derive(Clone, Serialize)]
struct PeaksChunkPayload {
    peaks: Vec<f32>,
    offset: usize,
    total: usize,
    duration: f64,
    done: bool,
}

/// ffmpeg が利用可能かチェックし、FfmpegState にパスをセットする。
/// 戻り値: "system" / "sidecar" / "not_found"
#[tauri::command]
fn check_ffmpeg(state: State<FfmpegState>) -> Result<String, String> {
    // システム PATH にある ffmpeg を試す
    if ffmpeg_extractor::probe_ffmpeg(Path::new("ffmpeg")) {
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        *guard = Some(PathBuf::from("ffmpeg"));
        return Ok("system".to_string());
    }

    // ffmpeg-sidecar がダウンロード済みのバイナリを試す
    let sidecar = ffmpeg_sidecar::paths::ffmpeg_path();
    if sidecar.exists() && ffmpeg_extractor::probe_ffmpeg(&sidecar) {
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        *guard = Some(sidecar);
        return Ok("sidecar".to_string());
    }

    Ok("not_found".to_string())
}

/// ユーザーが指定したパスの ffmpeg を登録する。
#[tauri::command]
fn set_ffmpeg_path(state: State<FfmpegState>, path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !ffmpeg_extractor::probe_ffmpeg(&p) {
        return Err(format!("指定パスの ffmpeg が動作しません: {}", path));
    }
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    *guard = Some(p);
    Ok(())
}

/// ffmpeg-sidecar を使って ffmpeg を自動ダウンロードし、FfmpegState に登録する。
/// ダウンロード進捗は "ffmpeg-download-progress" イベントで通知する。
#[tauri::command]
async fn download_ffmpeg(app: AppHandle, state: State<'_, FfmpegState>) -> Result<(), String> {
    use ffmpeg_sidecar::download::{auto_download_with_progress, FfmpegDownloadProgressEvent};

    tauri::async_runtime::spawn_blocking(move || {
        auto_download_with_progress(|event| {
            let payload = match event {
                FfmpegDownloadProgressEvent::Starting => {
                    serde_json::json!({ "status": "starting", "pct": 0 })
                }
                FfmpegDownloadProgressEvent::Downloading { total_bytes, downloaded_bytes } => {
                    let pct = if total_bytes > 0 {
                        (downloaded_bytes as f64 / total_bytes as f64 * 100.0) as u32
                    } else {
                        0
                    };
                    serde_json::json!({
                        "status": "downloading",
                        "pct": pct,
                        "downloaded": downloaded_bytes,
                        "total": total_bytes
                    })
                }
                FfmpegDownloadProgressEvent::UnpackingArchive => {
                    serde_json::json!({ "status": "unpacking", "pct": 99 })
                }
                FfmpegDownloadProgressEvent::Done => {
                    serde_json::json!({ "status": "done", "pct": 100 })
                }
            };
            let _ = app.emit("ffmpeg-download-progress", payload);
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    let path = ffmpeg_sidecar::paths::ffmpeg_path();
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    *guard = Some(path);
    Ok(())
}

/// フロントから呼ばれる: ファイルを ffmpeg でデコードしピークをチャンク単位でイベント送信する。
/// 新しい呼び出しがあると波形を切り替える前の処理をキャンセルする。
#[tauri::command]
async fn generate_peaks(
    app: AppHandle,
    cancel_state: State<'_, PeakCancelState>,
    ffmpeg_state: State<'_, FfmpegState>,
    path: String,
    peaks_count: usize,
) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("file not found: {}", path));
    }

    // 旧タスクをキャンセルし、新しいフラグを作成する
    let cancel = {
        let mut guard = cancel_state.0.lock().map_err(|e| e.to_string())?;
        guard.store(true, Ordering::Relaxed); // 旧タスクを止める
        let new_flag = Arc::new(AtomicBool::new(false));
        *guard = new_flag.clone();
        new_flag
    };

    let ffmpeg = {
        let guard = ffmpeg_state.0.lock().map_err(|e| e.to_string())?;
        guard.clone().ok_or_else(|| "ffmpeg が設定されていません".to_string())?
    };

    tauri::async_runtime::spawn_blocking(move || {
        ffmpeg_extractor::generate_peaks_streaming(
            &ffmpeg,
            &p,
            peaks_count,
            60.0,
            &cancel,
            |peaks, offset, total, duration, done| {
                let _ = app.emit(
                    "peaks-chunk",
                    PeaksChunkPayload { peaks, offset, total, duration, done },
                );
            },
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// リージョンを WAV としてエクスポートする。
#[tauri::command]
fn export_regions(
    app: AppHandle,
    ffmpeg_state: State<FfmpegState>,
    input_path: String,
    out_dir: String,
    regions: Vec<Region>,
) -> Result<(), String> {
    let ffmpeg = {
        let guard = ffmpeg_state.0.lock().map_err(|e| e.to_string())?;
        guard.clone().ok_or_else(|| "ffmpeg が設定されていません".to_string())?
    };
    let input_path = PathBuf::from(&input_path);
    if !input_path.exists() {
        return Err(format!("input file does not exist: {}", input_path.display()));
    }

    let out_dir = PathBuf::from(out_dir);
    if !out_dir.exists() {
        std::fs::create_dir_all(&out_dir)
            .map_err(|e| format!("failed to create output directory: {e}"))?;
    }

    let total = regions.len();
    for (idx, region) in regions.into_iter().enumerate() {
        let progress = (idx as f64 / total.max(1) as f64) * 100.0;
        let _ = app.emit("export-progress", progress);

        let msg = format!(
            "Exporting region: {} ({:.2}->{:.2})",
            region.name, region.start, region.end
        );
        let _ = app.emit("export-log", msg);

        ffmpeg_extractor::extract_region(&ffmpeg, &input_path, &out_dir, &region)
            .map_err(|e| format!("failed to export region: {e}"))?;

        let _ = app.emit("export-log", format!("Finished: {}", region.name));
    }

    let _ = app.emit("export-progress", 100.0);
    Ok(())
}

// ---- カスタムプロトコルヘルパー ----------------------------------------------

/// "bytes=START-END" または "bytes=START-" をパースする。
fn parse_range(value: &str) -> Option<(u64, Option<u64>)> {
    let s = value.strip_prefix("bytes=")?;
    let mut iter = s.splitn(2, '-');
    let start: u64 = iter.next()?.parse().ok()?;
    let end: Option<u64> = iter
        .next()
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok());
    Some((start, end))
}

fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

// ---- エントリポイント --------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Arc<Mutex> を作りプロトコルクロージャと managed state で共有する
    let video_path: SharedVideoPath = Arc::new(Mutex::new(None));
    let proto_path = video_path.clone();

    tauri::Builder::default()
        .manage(VideoState(video_path))
        .manage(PeakCancelState(Mutex::new(Arc::new(AtomicBool::new(false)))))
        .manage(FfmpegState(Mutex::new(None)))
        // --------------------------------------------------------------
        // stream://localhost/video → 登録済みファイルを Range 対応で配信
        // --------------------------------------------------------------
        .register_uri_scheme_protocol("stream", move |_ctx, request| {
            let path = proto_path.lock().unwrap().clone();
            let path = match path {
                Some(p) => p,
                None => {
                    return tauri::http::Response::builder()
                        .status(404)
                        .header("Content-Type", "text/plain")
                        .body(b"no video registered".to_vec())
                        .unwrap()
                }
            };

            let file_size = match std::fs::metadata(&path) {
                Ok(m) => m.len(),
                Err(e) => {
                    return tauri::http::Response::builder()
                        .status(500)
                        .body(e.to_string().into_bytes())
                        .unwrap()
                }
            };

            // Range ヘッダをパースして配信範囲を決定
            let range_str = request
                .headers()
                .get("Range")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_owned());

            let (start, end, is_range) = if let Some(ref r) = range_str {
                if let Some((s, e)) = parse_range(r) {
                    // start がファイルサイズを超えていたら 416 を返す
                    if s >= file_size {
                        return tauri::http::Response::builder()
                            .status(416)
                            .header("Content-Range", format!("bytes */{}", file_size))
                            .body(vec![])
                            .unwrap();
                    }
                    let e = e.unwrap_or(file_size.saturating_sub(1));
                    // 1 リクエストあたり最大 1 MB に制限してメモリ使用量を抑える
                    let e = e.min(s + 1024 * 1024 - 1).min(file_size.saturating_sub(1));
                    (s, e, true)
                } else {
                    return tauri::http::Response::builder()
                        .status(400)
                        .body(b"invalid Range header".to_vec())
                        .unwrap();
                }
            } else {
                // Range なし: 先頭 1 MB だけ返し Accept-Ranges を通知する
                let e = (1024u64 * 1024 - 1).min(file_size.saturating_sub(1));
                (0, e, false)
            };

            // ディスクから必要な範囲だけ読む
            let length = (end - start + 1) as usize;
            let mut buf = vec![0u8; length];
            match (|| -> std::io::Result<usize> {
                let mut f = std::fs::File::open(&path)?;
                f.seek(SeekFrom::Start(start))?;
                let mut total = 0;
                while total < buf.len() {
                    match f.read(&mut buf[total..]) {
                        Ok(0) => break,
                        Ok(n) => total += n,
                        Err(e) => return Err(e),
                    }
                }
                Ok(total)
            })() {
                Ok(n) => buf.truncate(n),
                Err(e) => {
                    return tauri::http::Response::builder()
                        .status(500)
                        .body(e.to_string().into_bytes())
                        .unwrap()
                }
            }

            let actual_end = start + buf.len() as u64 - 1;
            let mime = mime_for(&path);
            let status = if is_range { 206u16 } else { 200u16 };

            let mut builder = tauri::http::Response::builder()
                .status(status)
                .header("Content-Type", mime)
                .header("Content-Length", buf.len().to_string())
                .header("Accept-Ranges", "bytes");

            if is_range {
                builder = builder.header(
                    "Content-Range",
                    format!("bytes {}-{}/{}", start, actual_end, file_size),
                );
            }

            builder.body(buf).unwrap()
        })
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            set_video_path,
            check_ffmpeg,
            set_ffmpeg_path,
            download_ffmpeg,
            generate_peaks,
            export_regions
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
