//! The app updating itself: the latest release on GitHub is asked whether
//! it is newer than this build, and when the user says so it is fetched,
//! checked against the key this build carries, put in this one's place and
//! started.
//!
//! The release says what it holds in `latest.json`, which the release
//! script writes: a version, and for each system the file to fetch and its
//! signature. Only a published release that is not a pre-release answers
//! at `releases/latest`.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::error::{AppError, AppResult};

/// The event a download reports its progress through
pub const PROGRESS: &str = "update-progress";
/// Where a version that cannot put itself in place is fetched by hand
const PAGE: &str = "https://github.com/nickolasdeluca/n-studio/releases/latest";

/// The update last found, kept for when the user asks for it
#[derive(Default)]
pub struct Found(Mutex<Option<Update>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Available {
    version: String,
    current: String,
    /// The release's page, for its notes and for fetching it by hand
    page: &'static str,
    /// Whether this install can replace itself: on Linux an AppImage, and a
    /// `.deb`, which the package manager installs after asking for the
    /// administrator's password; any other install is fetched by hand
    installable: bool,
}

#[derive(Clone, Serialize)]
struct Progress {
    received: u64,
    /// `None` when the server did not say how much there is
    total: Option<u64>,
}

/// See [`Available::installable`]. The bundler marks each Linux build with
/// the package it went into.
fn installable() -> bool {
    use tauri::utils::{config::BundleType, platform::bundle_type};
    !cfg!(target_os = "linux")
        || std::env::var_os("APPIMAGE").is_some()
        || bundle_type() == Some(BundleType::Deb)
}

fn failed(e: tauri_plugin_updater::Error) -> AppError {
    AppError::Network { message: format!("The update could not be fetched: {e}") }
}

/// Asks the latest release whether it is newer than this build. A build
/// made for development never is told so: it would replace itself with a
/// release.
#[tauri::command]
pub async fn update_check(app: AppHandle, found: State<'_, Found>) -> AppResult<Option<Available>> {
    if cfg!(debug_assertions) {
        return Ok(None);
    }
    let update = app.updater().map_err(failed)?.check().await.map_err(failed)?;
    let available = update.as_ref().map(|update| Available {
        version: update.version.clone(),
        current: update.current_version.clone(),
        page: PAGE,
        installable: installable(),
    });
    *found.0.lock().unwrap() = update;
    Ok(available)
}

/// Fetches the update found, puts it in this build's place and starts it.
/// Answers only when that failed: the app that asked is gone otherwise.
#[tauri::command]
pub async fn update_install(app: AppHandle, found: State<'_, Found>) -> AppResult<()> {
    let update = found.0.lock().unwrap().clone();
    let Some(update) = update else {
        return Err(AppError::Invalid { message: "No update was found to install.".into() });
    };
    let mut received = 0u64;
    let progress = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                received += chunk as u64;
                // Nothing to tell when there is no window to hear it
                let _ = progress.emit(PROGRESS, Progress { received, total });
            },
            || {},
        )
        .await
        .map_err(failed)?;
    app.restart()
}
