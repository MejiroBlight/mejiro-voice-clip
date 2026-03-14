// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Emitter};

mod symphonia_extractor;
use symphonia_extractor::{Region, decode, extract};

#[tauri::command]
fn export_regions(
    app: AppHandle,
    input_path: String,
    out_dir: String,
    regions: Vec<Region>,
) -> Result<(), String> {
    let input_path = Path::new(&input_path);
    if !input_path.exists() {
        return Err(format!("input file does not exist: {}", input_path.display()));
    }

    let out_dir = PathBuf::from(out_dir);
    if !out_dir.exists() {
        std::fs::create_dir_all(&out_dir)
            .map_err(|e| format!("failed to create output directory: {e}"))?;
    }

    let _ = app.emit("export-log", "Decoding input file...");
    let decoded = decode(input_path).map_err(|e| format!("decode error: {e}"))?;
    let _ = app.emit("export-log", "Decode complete.");

    let total = regions.len();
    for (idx, region) in regions.into_iter().enumerate() {
        // Send progress before processing each region
        let progress = ((idx) as f64 / total.max(1) as f64) * 100.0;
        let _ = app.emit("export-progress", progress);

        let msg = format!("Exporting region: {} ({}->{})", region.name, region.start, region.end);
        let _ = app.emit("export-log", msg);

        let out_path = out_dir.join(&region.name);
        extract(&decoded, &out_path, region.clone())
            .map_err(|e| format!("failed to export region: {e}"))?;

        let _ = app.emit("export-log", format!("Finished: {}", region.name));
    }

    // Final progress = 100%
    let _ = app.emit("export-progress", 100.0);

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![export_regions])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
