use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use tauri::{AppHandle, Emitter, State};

mod symphonia_extractor;
use symphonia_extractor::{Region, extract_region};

// ---- 共有状態 ---------------------------------------------------------------

type SharedVideoPath = Arc<Mutex<Option<PathBuf>>>;

/// 現在開いている動画ファイルのパスを保持する。
/// カスタムプロトコルハンドラから Arc で共有する。
struct VideoState(SharedVideoPath);

/// 進行中のピーク生成タスクにキャンセルを伝えるフラグ。
/// 新しい生成が始まるのたびに旧フラグを true にして上書きする。
struct PeakCancelState(Mutex<Arc<AtomicBool>>);

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

/// フロントから呼ばれる: ファイルを symphonia でデコードしピークをチャンク単位でイベント送信する。
/// 新しい呼び出しがあると波形を辺失する前の切り替えを防ぐため、前のタスクをキャンセルする。
#[tauri::command]
async fn generate_peaks(
    app: AppHandle,
    cancel_state: State<'_, PeakCancelState>,
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

    tauri::async_runtime::spawn_blocking(move || {
        symphonia_extractor::generate_peaks_streaming(
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

/// 既存コマンド: リージョンを WAV としてエクスポートする。
/// リージョンごとに必要な範囲のみシーク・デコードするためメモリ効率がよい。
#[tauri::command]
fn export_regions(
    app: AppHandle,
    input_path: String,
    out_dir: String,
    regions: Vec<Region>,
) -> Result<(), String> {
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

        let out_path = out_dir.join(&region.name);
        extract_region(&input_path, &out_path, region.clone())
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
            generate_peaks,
            export_regions
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
