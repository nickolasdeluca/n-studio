// No console window behind the app on Windows release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod admin;
mod changes;
mod commands;
mod debugger;
mod editor;
mod error;
mod export;
mod objects;
mod registry;
mod results;
mod script;
mod secrets;
mod tools;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
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
            objects::object_tabs,
            objects::object_tab,
            objects::extract_metadata,
            objects::object_tab_script,
            objects::data_changes_script,
            objects::object_drop_script,
            objects::object_template,
            changes::object_action_form,
            changes::object_action_script,
            editor::editor_begin,
            editor::editor_end,
            editor::editor_cancel,
            editor::editor_describe,
            editor::editor_execute,
            editor::result_fetch,
            editor::result_rows,
            editor::result_sort,
            editor::result_export,
            editor::result_blob,
            editor::result_close,
            editor::sql_statements,
            editor::statement_parameters,
            script::script_run,
            script::script_cancel,
            admin::admin_view,
            admin::admin_form,
            admin::admin_script,
            admin::service_run,
            admin::service_cancel,
            tools::diagram,
            tools::write_text_file,
            tools::data_compare,
            tools::data_compare_cancel,
            tools::query_build,
            tools::query_sources,
            debugger::debug_open,
            debugger::debug_start,
            debugger::debug_command,
        ])
        .run(tauri::generate_context!())
        .expect("N Studio failed to start");
}
