//! The profiler of Firebird 5, on a registration's own connection: the one
//! its editors run on, so what the user runs while a session is open is what
//! it measures.

use fb_meta::profiler::{self, Profile, ProfileSession};
use tauri::AppHandle;

use crate::commands::blocking;
use crate::error::{AppError, AppResult};

fn five(major: u32) -> AppResult<()> {
    if major < 5 {
        return Err(AppError::Invalid { message: "The profiler is in Firebird 5 and later.".into() });
    }
    Ok(())
}

/// Starts a session: its id
#[tauri::command]
pub async fn profiler_start(app: AppHandle, id: String) -> AppResult<i64> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        five(conn.info().major)?;
        Ok(profiler::start(&conn, "N Studio")?)
    })
    .await
}

/// Finishes the open session, writing down what it measured, or, with
/// `discard`, without writing anything down
#[tauri::command]
pub async fn profiler_finish(app: AppHandle, id: String, discard: bool) -> AppResult<()> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        Ok(if discard { profiler::cancel(&conn)? } else { profiler::finish(&conn)? })
    })
    .await
}

#[tauri::command]
pub async fn profiler_sessions(app: AppHandle, id: String) -> AppResult<Vec<ProfileSession>> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        five(conn.info().major)?;
        Ok(profiler::sessions(&conn)?)
    })
    .await
}

#[tauri::command]
pub async fn profiler_report(app: AppHandle, id: String, profile: i64) -> AppResult<Profile> {
    blocking(app, move |state| {
        let conn = state.session(&id)?;
        profiler::report(&conn, profile)?
            .ok_or_else(|| AppError::NotFound { message: "That profiler session is no longer there.".into() })
    })
    .await
}

#[tauri::command]
pub async fn profiler_delete(app: AppHandle, id: String, profile: i64) -> AppResult<()> {
    blocking(app, move |state| Ok(profiler::delete(&*state.session(&id)?, profile)?)).await
}
