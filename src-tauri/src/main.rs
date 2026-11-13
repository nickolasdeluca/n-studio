// No console window behind the app on Windows release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod admin;
mod changes;
mod commands;
mod debugger;
mod editor;
mod error;
mod export;
mod files;
mod formatter;
mod history;
mod ibexpert;
mod objects;
mod registry;
mod results;
mod script;
mod secrets;
mod session;
mod tools;

use tauri::Manager;

/// What the toolbar's View button offers, in the menu bar's View menu too:
/// a Mac user looks there for a panel that was closed. The window carries
/// each choice out (`view-menu`), as it does its own button's.
#[cfg(target_os = "macos")]
fn view_menu(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem};
    use tauri::Emitter;

    let menu = Menu::default(app)?;
    let view = menu.items()?.into_iter().find_map(|item| match item {
        MenuItemKind::Submenu(submenu) if submenu.text().is_ok_and(|text| text == "View") => Some(submenu),
        _ => None,
    });
    if let Some(view) = view {
        view.prepend_items(&[
            &MenuItem::with_id(app, "view:explorer", "Database Explorer", true, None::<&str>)?,
            &MenuItem::with_id(app, "view:objects", "Objects", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "view:reset", "Reset Layout", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
        ])?;
    }
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        if let Some(choice) = event.id().as_ref().strip_prefix("view:") {
            // Nothing to tell when there is no window to hear it
            let _ = app.emit("view-menu", choice);
        }
    });
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            let registry = registry::Registry::load(&dir)?;
            app.manage(commands::AppState::new(
                registry,
                session::SessionStore::new(&dir),
                history::History::open(&dir),
                formatter::StyleStore::new(&dir),
            ));
            #[cfg(target_os = "macos")]
            view_menu(app.handle())?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
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
            editor::result_blob_save,
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
        ])
        .run(tauri::generate_context!())
        .expect("N Studio failed to start");
}
