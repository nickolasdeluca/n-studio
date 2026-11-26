//! The menu bar of a Mac: the one the system's apps have, with the panels the
//! toolbar's View button offers in its View menu too (a Mac user looks there
//! for a panel that was closed). Its texts are the window's: it sends them
//! in its language once it starts, and the bar is made again with them.
//! Other systems have no menu bar, and nothing is done there.

use std::collections::HashMap;

use crate::error::AppResult;

/// The texts the bar is made with, by what each is for; English for any the
/// window did not send
pub type Texts = HashMap<String, String>;

#[cfg(target_os = "macos")]
pub fn menu_bar(app: &tauri::AppHandle, texts: &Texts) -> tauri::Result<()> {
    use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu, HELP_SUBMENU_ID, WINDOW_SUBMENU_ID};

    let name = app.package_info().name.clone();
    let say = |id: &str, english: &str| texts.get(id).cloned().unwrap_or_else(|| english.replace("{app}", &name));
    let about = AboutMetadata {
        name: Some(name.clone()),
        version: Some(app.package_info().version.to_string()),
        copyright: app.config().bundle.copyright.clone(),
        authors: app.config().bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };
    let item = |id: &str, english: &str| MenuItem::with_id(app, id, say(id, english), true, None::<&str>);
    let separator = || PredefinedMenuItem::separator(app);

    let menu = Menu::with_items(
        app,
        &[
            &Submenu::with_items(
                app,
                &name,
                true,
                &[
                    &PredefinedMenuItem::about(app, Some(&say("about", "About {app}")), Some(about))?,
                    &separator()?,
                    &PredefinedMenuItem::services(app, Some(&say("services", "Services")))?,
                    &separator()?,
                    &PredefinedMenuItem::hide(app, Some(&say("hide", "Hide {app}")))?,
                    &PredefinedMenuItem::hide_others(app, Some(&say("hideOthers", "Hide Others")))?,
                    &separator()?,
                    &PredefinedMenuItem::quit(app, Some(&say("quit", "Quit {app}")))?,
                ],
            )?,
            &Submenu::with_items(
                app,
                say("file", "File"),
                true,
                &[&PredefinedMenuItem::close_window(app, Some(&say("closeWindow", "Close Window")))?],
            )?,
            &Submenu::with_items(
                app,
                say("edit", "Edit"),
                true,
                &[
                    &PredefinedMenuItem::undo(app, Some(&say("undo", "Undo")))?,
                    &PredefinedMenuItem::redo(app, Some(&say("redo", "Redo")))?,
                    &separator()?,
                    &PredefinedMenuItem::cut(app, Some(&say("cut", "Cut")))?,
                    &PredefinedMenuItem::copy(app, Some(&say("copy", "Copy")))?,
                    &PredefinedMenuItem::paste(app, Some(&say("paste", "Paste")))?,
                    &PredefinedMenuItem::select_all(app, Some(&say("selectAll", "Select All")))?,
                ],
            )?,
            &Submenu::with_items(
                app,
                say("view", "View"),
                true,
                &[
                    &item("view:explorer", "Database Explorer")?,
                    &item("view:objects", "Objects")?,
                    &item("view:agent", "AI Agent")?,
                    &separator()?,
                    &item("view:reset", "Reset Layout")?,
                    &separator()?,
                    &PredefinedMenuItem::fullscreen(app, Some(&say("fullscreen", "Enter Full Screen")))?,
                ],
            )?,
            // By their ids the system knows them: it lists the windows in one
            // and puts its search in the other
            &Submenu::with_id_and_items(
                app,
                WINDOW_SUBMENU_ID,
                say("window", "Window"),
                true,
                &[
                    &PredefinedMenuItem::minimize(app, Some(&say("minimize", "Minimize")))?,
                    &PredefinedMenuItem::maximize(app, Some(&say("zoom", "Zoom")))?,
                    &separator()?,
                    &PredefinedMenuItem::close_window(app, Some(&say("closeWindow", "Close Window")))?,
                ],
            )?,
            &Submenu::with_id_and_items(app, HELP_SUBMENU_ID, say("help", "Help"), true, &[])?,
        ],
    )?;
    app.set_menu(menu)?;
    Ok(())
}

/// What a choice of the bar's View menu tells the window (`view-menu`): it
/// carries it out as it does its own button's
#[cfg(target_os = "macos")]
pub fn listen(app: &tauri::AppHandle) {
    use tauri::Emitter;
    app.on_menu_event(|app, event| {
        if let Some(choice) = event.id().as_ref().strip_prefix("view:") {
            // Nothing to tell when there is no window to hear it
            let _ = app.emit("view-menu", choice);
        }
    });
}

/// The window's texts for the menu bar, in its language
#[tauri::command]
pub fn menu_texts(app: tauri::AppHandle, texts: Texts) -> AppResult<()> {
    #[cfg(target_os = "macos")]
    menu_bar(&app, &texts).map_err(|e| crate::error::AppError::Invalid { message: e.to_string() })?;
    #[cfg(not(target_os = "macos"))]
    let _ = (app, texts);
    Ok(())
}
