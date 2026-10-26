//! M0 comparison: the grid-shell spike rebuilt in Slint, measured the same way.
//!
//! `FBX_BENCH=synthetic:<rows>:<cols>` runs unattended — random jumps, then
//! continuous scrolling — prints one `BENCH {json}` line and exits. Without it,
//! opens 1M x 6 synthetic rows for interactive use.
//!
//! Frames are timed with Slint's rendering notifier, i.e. where they are drawn.

#[path = "../../rowstore/rowstore.rs"]
mod rowstore;

slint::include_modules!();

use rand::Rng;
use rowstore::{synthetic_columns, synthetic_pages, Page, PAGE};
use slint::{ComponentHandle, Model, ModelRc, RenderingState, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

const ROW_H: f32 = 22.0;
const COL_W: f32 = 150.0;
const GUTTER_W: f32 = 72.0;
/// Rows and columns kept filled beyond the viewport on each side.
const OVERSCAN_ROWS: usize = 10;
const OVERSCAN_COLS: usize = 1;

/// The visible window of the result, mapped onto a fixed ring of row slots:
/// row `r` always lives in slot `r % slots`, so scrolling by a few rows
/// refills only those few slots.
struct Grid {
    pages: Vec<Page>,
    total: usize,
    cols: usize,
    slots: Rc<VecModel<RowData>>,
    slot_cells: Vec<Rc<VecModel<CellData>>>,
    /// Row held by each slot, and the column window it was filled for.
    slot_row: Vec<Option<usize>>,
    col_window: (usize, usize),
    /// The scroll position the slots were last filled for.
    filled_for: (f32, f32),
}

impl Grid {
    fn new(pages: Vec<Page>, cols: usize) -> Self {
        let total = pages.iter().map(|p| p.rows).sum();
        Self {
            pages,
            total,
            cols,
            slots: Rc::new(VecModel::default()),
            slot_cells: Vec::new(),
            slot_row: Vec::new(),
            col_window: (0, 0),
            filled_for: (f32::NAN, f32::NAN),
        }
    }

    fn cell(&self, row: usize, col: usize) -> CellData {
        match self.pages[row / PAGE].cell(row % PAGE, col) {
            Some(text) => CellData { text: SharedString::from(text), is_null: false },
            None => CellData { text: SharedString::new(), is_null: true },
        }
    }

    /// Refills whatever slots the current scroll position needs.
    fn refresh(&mut self, ui: &AppWindow) {
        let sy = -ui.get_scroll_y();
        let sx = -ui.get_scroll_x();
        let (vw, vh) = (ui.get_view_width(), ui.get_view_height());

        let first = ((sy / ROW_H) as usize).saturating_sub(OVERSCAN_ROWS);
        let n = (vh / ROW_H).ceil() as usize + 2 * OVERSCAN_ROWS + 1;

        let c0 = (((sx - GUTTER_W).max(0.0) / COL_W) as usize).saturating_sub(OVERSCAN_COLS);
        let c1 = ((((sx + vw - GUTTER_W) / COL_W).ceil() as usize) + OVERSCAN_COLS).min(self.cols);
        let cols = (c0.min(c1), c1);

        // Slot count follows the viewport height; rebuilding is rare (resize).
        if self.slot_cells.len() != n {
            self.slot_cells = (0..n).map(|_| Rc::new(VecModel::default())).collect();
            self.slot_row = vec![None; n];
            let empty = (0..n).map(|k| RowData {
                index: -1,
                first_col: 0,
                cells: ModelRc::from(self.slot_cells[k].clone()),
            });
            self.slots.set_vec(empty.collect::<Vec<_>>());
        }

        let cols_moved = cols != self.col_window;
        self.col_window = cols;

        for r in first..first + n {
            let k = r % n;
            let row = (r < self.total).then_some(r);
            if self.slot_row[k] == row && !cols_moved {
                continue;
            }
            self.slot_row[k] = row;

            let Some(row) = row else {
                self.slots.set_row_data(k, RowData { index: -1, first_col: 0, cells: ModelRc::from(self.slot_cells[k].clone()) });
                continue;
            };

            let cells = &self.slot_cells[k];
            let width = cols.1 - cols.0;
            if cells.row_count() != width {
                cells.set_vec((cols.0..cols.1).map(|c| self.cell(row, c)).collect::<Vec<_>>());
            } else {
                for (j, c) in (cols.0..cols.1).enumerate() {
                    cells.set_row_data(j, self.cell(row, c));
                }
            }
            self.slots.set_row_data(
                k,
                RowData { index: row as i32, first_col: cols.0 as i32, cells: ModelRc::from(cells.clone()) },
            );
        }

        self.filled_for = (sy, sx);
    }

    /// The slots match the current scroll position: nothing visible is stale.
    fn current(&self, ui: &AppWindow) -> bool {
        self.filled_for == (-ui.get_scroll_y(), -ui.get_scroll_x())
    }
}

/// Benchmark phases, advanced one rendered frame at a time.
enum Phase {
    Settle(u32),
    Jump { done: usize, started: Instant },
    Scroll { done: usize, last: Instant },
    Finished,
}

struct Bench {
    phase: Phase,
    /// When the renderer last reported a frame; see the watchdog in `main`.
    last_frame: Instant,
    jumps: Vec<f64>,
    frames: Vec<f64>,
    render: Vec<f64>,
    render_started: Option<Instant>,
    /// Moves the scroll position at 60 Hz during the scroll phase, the way
    /// input events arrive: independent of when frames come out.
    scroller: Option<slint::Timer>,
    stale_frames: usize,
    report: serde_json::Value,
}

const JUMPS: usize = 100;
const SCROLL_FRAMES: usize = 600;

fn main() -> Result<(), slint::PlatformError> {
    let bench = std::env::var("FBX_BENCH").ok();
    let (rows, cols) = match bench.as_deref().and_then(|b| b.strip_prefix("synthetic:")) {
        Some(spec) => {
            let mut it = spec.split(':').map(|n| n.parse::<usize>().expect("synthetic:<rows>:<cols>"));
            (it.next().unwrap(), it.next().unwrap())
        }
        None => (1_000_000, 6),
    };

    let t = Instant::now();
    let pages = synthetic_pages(rows, cols);
    let build_ms = t.elapsed().as_secs_f64() * 1000.0;

    let ui = AppWindow::new()?;
    let grid = Rc::new(RefCell::new(Grid::new(pages, cols)));
    {
        let g = grid.borrow();
        ui.set_columns(ModelRc::from(Rc::new(VecModel::from(
            synthetic_columns(cols).into_iter().map(SharedString::from).collect::<Vec<_>>(),
        ))));
        ui.set_row_count(g.total as i32);
        ui.set_slots(ModelRc::from(g.slots.clone()));
        ui.set_status(format!("{} rows x {} columns", g.total, cols).into());
    }

    let weak = ui.as_weak();
    let g = grid.clone();
    ui.on_viewport_changed(move || {
        if let Some(ui) = weak.upgrade() {
            g.borrow_mut().refresh(&ui);
        }
    });

    if let Some(config) = bench {
        let state = Rc::new(RefCell::new(Bench {
            phase: Phase::Settle(10),
            last_frame: Instant::now(),
            jumps: Vec::new(),
            frames: Vec::new(),
            render: Vec::new(),
            render_started: None,
            scroller: None,
            stale_frames: 0,
            report: serde_json::json!({ "config": config, "toolkit": "slint", "build_store_ms": build_ms.round() }),
        }));
        let weak = ui.as_weak();
        let g = grid.clone();
        let notifier_state = state.clone();
        ui.window()
            .set_rendering_notifier(move |rendering, _| {
                let state = &notifier_state;
                let mut b = state.borrow_mut();
                match rendering {
                    RenderingState::BeforeRendering => b.render_started = Some(Instant::now()),
                    RenderingState::AfterRendering => {
                        b.last_frame = Instant::now();
                        if let Some(t) = b.render_started.take() {
                            b.render.push(t.elapsed().as_secs_f64() * 1000.0);
                        }
                        // Step the benchmark after this frame, outside the renderer.
                        let (weak, g, state) = (weak.clone(), g.clone(), state.clone());
                        slint::Timer::single_shot(Duration::ZERO, move || {
                            if let Some(ui) = weak.upgrade() {
                                step(&ui, &mut g.borrow_mut(), &mut state.borrow_mut());
                            }
                        });
                    }
                    _ => {}
                }
            })
            .expect("rendering notifier (needs the FemtoVG or Skia renderer)");

        // Some renderers accept a notifier and never call it (Skia on Metal),
        // and an occluded window gets no frames. Either way, fail loudly
        // instead of waiting forever.
        let watchdog = slint::Timer::default();
        let weak = ui.as_weak();
        watchdog.start(slint::TimerMode::Repeated, Duration::from_secs(1), move || {
            let mut b = state.borrow_mut();
            if matches!(b.phase, Phase::Finished) || b.last_frame.elapsed() < Duration::from_secs(5) {
                return;
            }
            b.report["error"] = "no frame reported for 5 s: the renderer does not call the rendering \
                                 notifier, or the window is occluded"
                .into();
            b.phase = Phase::Finished;
            if let Some(ui) = weak.upgrade() {
                finish(&ui, &mut b);
            }
        });
        ui.run()
    } else {
        ui.run()
    }
}

/// One benchmark step, run after each rendered frame.
fn step(ui: &AppWindow, grid: &mut Grid, b: &mut Bench) {
    let now = Instant::now();
    if std::env::var_os("FBX_BENCH_TRACE").is_some() {
        let phase = match &b.phase {
            Phase::Settle(n) => format!("settle {n}"),
            Phase::Jump { done, .. } => format!("jump {done}"),
            Phase::Scroll { done, .. } => format!("scroll {done}"),
            Phase::Finished => "finished".into(),
        };
        eprintln!("[bench] frame: {phase}");
    }
    let max_y = (grid.total as f32 * ROW_H - ui.get_view_height()).max(0.0);

    // A frame drawn from slots filled for another scroll position showed
    // stale or empty rows — the analogue of the web grid's placeholders.
    if matches!(b.phase, Phase::Scroll { .. }) && !grid.current(ui) {
        b.stale_frames += 1;
    }

    b.phase = match std::mem::replace(&mut b.phase, Phase::Finished) {
        Phase::Settle(0) => {
            b.render.clear();
            start_jump(ui, grid, max_y, 0)
        }
        Phase::Settle(n) => {
            ui.window().request_redraw();
            Phase::Settle(n - 1)
        }
        Phase::Jump { done, started } => {
            b.jumps.push(started.elapsed().as_secs_f64() * 1000.0);
            if done + 1 < JUMPS {
                start_jump(ui, grid, max_y, done + 1)
            } else {
                ui.set_scroll_y(-(max_y / 2.0).round());
                grid.refresh(ui);
                b.render.clear();

                let scroller = slint::Timer::default();
                let weak = ui.as_weak();
                scroller.start(slint::TimerMode::Repeated, Duration::from_micros(16_667), move || {
                    let Some(ui) = weak.upgrade() else { return };
                    // The `changed` handler refills the slots: a user's path.
                    ui.set_scroll_y((ui.get_scroll_y() - 3.0 * ROW_H).max(-max_y));
                });
                b.scroller = Some(scroller);
                Phase::Scroll { done: 0, last: now }
            }
        }
        Phase::Scroll { done, last } => {
            if done > 0 {
                b.frames.push(now.duration_since(last).as_secs_f64() * 1000.0);
            }
            if done < SCROLL_FRAMES {
                if ui.get_scroll_y() <= -max_y {
                    // At the end nothing changes and no frame would come; keep
                    // frames coming, as requestAnimationFrame does in grid-shell.
                    ui.window().request_redraw();
                }
                Phase::Scroll { done: done + 1, last: now }
            } else {
                b.scroller = None;
                finish(ui, b);
                Phase::Finished
            }
        }
        Phase::Finished => Phase::Finished,
    };
}

fn start_jump(ui: &AppWindow, grid: &mut Grid, max_y: f32, done: usize) -> Phase {
    let target = rand::thread_rng().gen_range(0.0..=max_y);
    let started = Instant::now();
    ui.set_scroll_y(-(target / ROW_H).floor() * ROW_H);
    grid.refresh(ui);
    Phase::Jump { done, started }
}

fn finish(ui: &AppWindow, b: &mut Bench) {
    b.report["jump_ms"] = summary(&b.jumps);
    let mut scroll = summary(&b.frames);
    scroll["over_20ms"] = b.frames.iter().filter(|&&f| f > 20.0).count().into();
    scroll["stale_frames"] = b.stale_frames.into();
    b.report["scroll_frame_ms"] = scroll;
    b.report["render_ms"] = summary(&b.render);
    b.report["rss_kb"] = rss_kb().into();
    println!("BENCH {}", b.report);
    let _ = ui.hide();
    let _ = slint::quit_event_loop();
}

fn summary(xs: &[f64]) -> serde_json::Value {
    let mut s = xs.to_vec();
    s.sort_by(f64::total_cmp);
    let at = |p: f64| s.get(((p / 100.0) * s.len() as f64) as usize).or(s.last()).map(|v| (v * 100.0).round() / 100.0);
    serde_json::json!({ "n": s.len(), "p50": at(50.0), "p95": at(95.0), "max": s.last().copied() })
}

fn rss_kb() -> u64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}
