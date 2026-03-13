// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use std::path::Path;

use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

mod symphonia_extractor;
use symphonia_extractor::extract_audio_to_wav;

#[tauri::command]
fn extract_audio_from_video(input_path: String) -> Result<String, String> {
    let input_path = Path::new(&input_path);

    if !input_path.exists() {
        return Err(format!(
            "input file does not exist: {}",
            input_path.display()
        ));
    }

    let output_path = input_path.with_extension("wav");

    // First try Symphonia-based extraction.
    match extract_audio_to_wav(input_path, &output_path) {
        Ok(()) => Ok(output_path.to_string_lossy().to_string()),
        Err(sym_err) => Err(format!("failed to extract audio with Symphonia: {sym_err}")),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![extract_audio_from_video])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
