// No console window behind the app on Windows release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod editor;
mod error;
mod registry;
mod results;
mod secrets;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            let registry = registry::Registry::load(&dir)?;
            app.manage(commands::AppState::new(registry));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::charsets,
            commands::list_registrations,
            commands::save_registration,
            commands::delete_registration,
            commands::test_connection,
            commands::connect,
            commands::disconnect,
            commands::explorer_tree,
            commands::object_list,
            editor::editor_begin,
            editor::editor_end,
            editor::editor_cancel,
            editor::editor_describe,
            editor::editor_execute,
            editor::result_fetch,
            editor::result_rows,
            editor::result_blob,
            editor::result_close,
            editor::sql_statements,
            editor::statement_parameters,
        ])
        .run(tauri::generate_context!())
        .expect("N Studio failed to start");
}
