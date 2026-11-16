//! Where the window was left: its monitor, place and size, and whether it
//! was maximized, kept as it is moved and not only when it closes, so that
//! an app that was killed opens where it was too.
//!
//! The place kept is the one the window has when it is neither maximized
//! nor minimized: that is where it goes back to when it is restored, and a
//! maximized window opens maximized on the monitor that place is on.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewWindow, Window, WindowEvent};

const FILE: &str = "window.json";
/// How long a move or a resize may go on before it is written down
const EVERY: Duration = Duration::from_millis(400);
/// How much of the window has to be on a monitor for it to open there
const IN_SIGHT: i32 = 120;
/// How long a window opened maximized is given to become so
const SETTLES_IN: Duration = Duration::from_secs(3);

/// In physical pixels: `x` and `y` of the window's frame, `width` and
/// `height` of what is inside it
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Place {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    /// Of the monitor it was on: how many pixels its points are
    #[serde(default = "one")]
    scale: f64,
    #[serde(default)]
    maximized: bool,
    #[serde(default)]
    fullscreen: bool,
    /// Kept for what it tells; the window never opens minimized, which
    /// would look like an app that did not start
    #[serde(default)]
    minimized: bool,
}

fn one() -> f64 {
    1.0
}

impl Place {
    /// Whether enough of its top is on a monitor to be seen and taken hold of
    fn in_sight(&self, monitors: &[(i32, i32, u32, u32)]) -> bool {
        let (left, right, top) = (self.x, self.x.saturating_add(self.width as i32), self.y);
        monitors.iter().any(|&(x, y, width, height)| {
            let across = right.min(x.saturating_add(width as i32)) - left.max(x);
            across >= IN_SIGHT.min(self.width as i32) && top >= y - 8 && top < y.saturating_add(height as i32) - 40
        })
    }
}

/// The last place known, and whether the file has it
pub struct Places {
    path: PathBuf,
    place: Mutex<Option<Place>>,
    unwritten: AtomicBool,
    /// Set while a window opened maximized is not so yet: until when what
    /// it reports is not taken. A window is placed and maximized a moment
    /// after it is asked to be (on macOS, in the main thread's next turn),
    /// and what it says before that is where it was made, which would
    /// replace the place it goes back to.
    settling: Mutex<Option<Instant>>,
}

impl Places {
    fn read(path: &Path) -> Option<Place> {
        let place: Place = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        (place.width > 0 && place.height > 0).then_some(place)
    }

    /// Reads what was kept, and starts writing down what changes: from a
    /// thread, so a window being dragged writes a few times a second and
    /// not at every pixel
    pub fn open(dir: &Path) -> Arc<Self> {
        let path = dir.join(FILE);
        let places = Arc::new(Self {
            place: Mutex::new(Self::read(&path)),
            path,
            unwritten: AtomicBool::new(false),
            settling: Mutex::new(None),
        });
        let writer = Arc::downgrade(&places);
        std::thread::spawn(move || {
            while let Some(places) = writer.upgrade() {
                places.write();
                drop(places);
                std::thread::sleep(EVERY);
            }
        });
        places
    }

    /// Writes the place if it changed since it was written. To a file
    /// beside and then renamed: an app killed while it writes leaves the
    /// place it had before, not half of one.
    pub fn write(&self) {
        if !self.unwritten.swap(false, Ordering::Relaxed) {
            return;
        }
        let Some(place) = *self.place.lock().unwrap() else { return };
        let Ok(text) = serde_json::to_vec_pretty(&place) else { return };
        let beside = self.path.with_extension("json.tmp");
        // A place that cannot be kept is no reason to trouble anyone
        if fs::write(&beside, text).is_ok() {
            let _ = fs::rename(&beside, &self.path);
        }
    }

    /// Puts the window where it was left, if that is still on a monitor,
    /// and shows it
    pub fn restore(&self, window: &WebviewWindow) {
        let kept = *self.place.lock().unwrap();
        if let Some(place) = kept {
            let monitors: Vec<_> = window
                .available_monitors()
                .unwrap_or_default()
                .iter()
                .map(|m| (m.position().x, m.position().y, m.size().width, m.size().height))
                .collect();
            // Nothing below is worth failing to start over
            if place.in_sight(&monitors) {
                // macOS places windows in points, and a window's pixels
                // are those of the monitor it is on: pixels kept on one
                // monitor mean another place to a window made on another
                if cfg!(target_os = "macos") && place.scale > 0.0 {
                    let points = |pixels: f64| pixels / place.scale;
                    let _ = window.set_position(LogicalPosition::new(points(place.x.into()), points(place.y.into())));
                    let _ = window.set_size(LogicalSize::new(points(place.width.into()), points(place.height.into())));
                } else {
                    let _ = window.set_position(PhysicalPosition::new(place.x, place.y));
                    let _ = window.set_size(PhysicalSize::new(place.width, place.height));
                }
            }
        }
        let _ = window.show();
        if let Some(place) = kept {
            if place.maximized || place.fullscreen {
                *self.settling.lock().unwrap() = Some(Instant::now() + SETTLES_IN);
            }
            if place.maximized {
                let _ = window.maximize();
            }
            if place.fullscreen {
                let _ = window.set_fullscreen(true);
            }
        }
    }

    /// The window moved, changed size or state: takes note of it
    pub fn changed(&self, window: &Window, event: &WindowEvent) {
        match event {
            WindowEvent::Moved(_) | WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {}
            WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed => return self.write(),
            _ => return,
        }
        let minimized = window.is_minimized().unwrap_or(false);
        let maximized = window.is_maximized().unwrap_or(false);
        let fullscreen = window.is_fullscreen().unwrap_or(false);
        {
            let mut settling = self.settling.lock().unwrap();
            match *settling {
                Some(until) if !(maximized || fullscreen) && Instant::now() < until => return,
                _ => *settling = None,
            }
        }
        let mut kept = self.place.lock().unwrap();
        let mut place = match (*kept, window.outer_position(), window.inner_size()) {
            // Only a window that is neither minimized nor filling the
            // screen is where it would go back to
            (_, Ok(at), Ok(size)) if !(minimized || maximized || fullscreen) && size.width > 0 && size.height > 0 => {
                let scale = window.scale_factor().unwrap_or(1.0);
                Place { x: at.x, y: at.y, width: size.width, height: size.height, scale, maximized, fullscreen, minimized }
            }
            (Some(place), _, _) => place,
            _ => return,
        };
        // Minimized, it is still maximized or not as it was before
        if !minimized {
            place.maximized = maximized;
            place.fullscreen = fullscreen;
        }
        place.minimized = minimized;
        if *kept != Some(place) {
            *kept = Some(place);
            self.unwritten.store(true, Ordering::Relaxed);
        }
    }
}

/// The app's window, put where it was left; `places` goes on following it
pub fn restore(app: &tauri::App, places: &Places) {
    if let Some(window) = app.get_webview_window("main") {
        places.restore(&window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: [(i32, i32, u32, u32); 1] = [(0, 0, 1920, 1080)];
    const TWO: [(i32, i32, u32, u32); 2] = [(0, 0, 1920, 1080), (-2560, -200, 2560, 1440)];

    fn at(x: i32, y: i32) -> Place {
        Place { x, y, width: 1200, height: 800, scale: 1.0, maximized: false, fullscreen: false, minimized: false }
    }

    #[test]
    fn a_window_opens_where_it_was_only_while_that_is_on_a_monitor() {
        assert!(at(100, 80).in_sight(&ONE));
        // Half off the right edge is still within reach
        assert!(at(1500, 80).in_sight(&ONE));
        // On the monitor at the left, while there is one
        assert!(at(-2000, 100).in_sight(&TWO));
        assert!(!at(-2000, 100).in_sight(&ONE));
        // Its title under the screen's end, or all but a sliver past its side
        assert!(!at(100, 1070).in_sight(&ONE));
        assert!(!at(1900, 80).in_sight(&ONE));
        // Where Windows says a minimized window is
        assert!(!at(-32000, -32000).in_sight(&TWO));
    }

    #[test]
    fn what_was_kept_is_read_back_and_a_broken_file_is_no_place() {
        let dir = tempfile::tempdir().unwrap();
        let places = Places {
            path: dir.path().join(FILE),
            place: Mutex::new(Some(at(10, 20))),
            unwritten: AtomicBool::new(true),
            settling: Mutex::new(None),
        };
        places.write();
        assert_eq!(Places::read(&places.path), Some(at(10, 20)));
        // Written once: nothing changed since
        fs::remove_file(&places.path).unwrap();
        places.write();
        assert_eq!(Places::read(&places.path), None);
        fs::write(&places.path, "{ \"x\": 1").unwrap();
        assert_eq!(Places::read(&places.path), None);
    }
}
