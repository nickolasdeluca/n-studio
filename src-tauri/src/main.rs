// No console window behind the app on Windows release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod admin;
mod agent;
mod changes;
mod commands;
mod debugger;
mod editor;
mod error;
mod executive;
mod export;
mod files;
mod formatter;
mod history;
mod ibexpert;
mod menu;
mod objects;
mod profiler;
mod registry;
mod results;
mod script;
mod secrets;
mod session;
mod summary;
mod tools;
mod trace;
mod update;
mod window;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(update::Found::default())
        .manage(agent::Agents::default())
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            let registry = registry::Registry::load(&dir)?;
            app.manage(commands::AppState::new(
                registry,
                session::SessionStore::new(&dir),
                history::History::open(&dir),
                formatter::StyleStore::new(&dir),
                trace::Traces::new(&dir),
            ));
            // The window is made unseen, and shown where it was left
            let places = window::Places::open(&dir);
            window::restore(app, &places);
            app.manage(places);
            // In English until the window sends its texts
            #[cfg(target_os = "macos")]
            {
                menu::menu_bar(app.handle(), &menu::Texts::new())?;
                menu::listen(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let Some(places) = window.try_state::<std::sync::Arc<window::Places>>() {
                    places.changed(window, event);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            menu::menu_texts,
            commands::charsets,
            commands::list_registrations,
            commands::save_registration,
            commands::delete_registration,
            commands::move_folder,
            commands::place_registration,
            commands::test_connection,
            commands::connect,
            commands::disconnect,
            commands::explorer_tree,
            commands::object_list,
            commands::completion_catalog,
            objects::object_tabs,
            objects::object_tab,
            objects::extract_metadata,
            objects::object_tab_script,
            objects::data_changes_script,
            objects::table_inserts,
            objects::row_editing,
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
            editor::result_summary,
            editor::result_export,
            editor::result_inserts,
            editor::result_blob,
            editor::result_blob_save,
            editor::result_close,
            editor::sql_statements,
            editor::statement_parameters,
            script::script_run,
            script::script_cancel,
            executive::executive_outline,
            executive::executive_outline_file,
            executive::executive_run,
            executive::executive_cancel,
            executive::executive_password,
            executive::executive_params,
            executive::executive_input_outline,
            executive::executive_compare,
            executive::executive_end,
            executive::executive_connect_lines,
            profiler::profiler_start,
            profiler::profiler_finish,
            profiler::profiler_sessions,
            profiler::profiler_report,
            profiler::profiler_delete,
            admin::admin_view,
            admin::admin_form,
            admin::admin_script,
            admin::service_run,
            admin::service_cancel,
            trace::trace_start,
            trace::trace_stop,
            trace::trace_close,
            trace::trace_state,
            trace::trace_page,
            trace::trace_event,
            trace::trace_top,
            trace::trace_memory,
            trace::trace_sessions,
            trace::trace_open,
            trace::trace_folder,
            update::update_check,
            update::update_install,
            files::script_read,
            files::script_write,
            history::history_add,
            history::history_list,
            history::history_clear,
            formatter::format_sql,
            formatter::format_style_load,
            formatter::format_style_default,
            formatter::format_style_save,
            formatter::format_style_export,
            formatter::format_style_import,
            ibexpert::ibexpert_scan,
            session::session_load,
            session::session_save,
            tools::diagram,
            tools::write_text_file,
            tools::data_compare,
            tools::data_compare_cancel,
            tools::query_build,
            tools::query_sources,
            debugger::debug_open,
            debugger::debug_start,
            debugger::debug_command,
            agent::agent_start,
            agent::agent_input,
            agent::agent_resize,
            agent::agent_stop,
            agent::agent_reply,
            agent::agent_share,
        ])
        .run(tauri::generate_context!())
        .expect("N Studio failed to start");
}
