mod commands;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::pick_folder,
            commands::scan,
            commands::transfer,
            commands::write_manifest_cmd,
            commands::verify_cmd,
            commands::hash_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
