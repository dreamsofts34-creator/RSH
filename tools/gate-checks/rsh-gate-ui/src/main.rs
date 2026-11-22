//! `rsh-gate-ui` — v2.0.2 UI gate checks (plan §C) that need a
//! rendered UI: slider fill geometry + click values, wizard step
//! gating + Next click-through, theme repaint, tray disconnected
//! state, tray Light theme (DS-22), banner wrap + grow (DS-21),
//! Extras card size ON/OFF + OFF no-input + responsive card gaps
//! (cards layout fix, CARDS-LAYOUT-RESEARCH).
//!
//! The real `.slint` files are compiled with `slint-interpreter`
//! on the testing backend with the software renderer — no X, no
//! window manager, no daemon. The wizard's Rust handlers live in
//! `rsh-ui` (a binary crate, not importable), so the click-through
//! mirrors the real `on_setup_next` handler from
//! `crates/rsh-ui/src/main.rs` (step + 1, finish at step 3) and the
//! gate doc says so: this proves the Slint wiring (button exists,
//! enabled exactly when its gate passes, fires the callback, step
//! advances), not the production Rust handler. The production
//! handler stays a user-PC verification.
//!
//! Usage: `cargo run -p rsh-gate-ui -- [--only <id>] [--list]`
//! Exit code 0 only when every selected check passes.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use i_slint_backend_testing::testing_backend::{TestingBackend, TestingBackendOptions};
use slint_interpreter::{Compiler, ComponentHandle, ComponentInstance, Value};

/// Accent blue (`Theme.accent`).
const ACCENT: (i32, i32, i32) = (0x2f, 0x6b, 0xff);
/// Status red (disconnected dot).
const RED: (i32, i32, i32) = (0xff, 0x6b, 0x6b);
/// Status green (connected dot).
const GREEN: (i32, i32, i32) = (0x3d, 0xd6, 0x8c);
/// Dark-theme surface family (`Theme.bg/side/panel/panel2`).
const DARK_FAMILY: &[(i32, i32, i32)] = &[
    (0x0d, 0x11, 0x18),
    (0x10, 0x15, 0x1d),
    (0x16, 0x1c, 0x27),
    (0x1d, 0x25, 0x33),
];
/// Light-theme surface family.
const LIGHT_FAMILY: &[(i32, i32, i32)] = &[
    (0xf5, 0xf3, 0xff),
    (0xea, 0xe7, 0xf8),
    (0xff, 0xff, 0xff),
    (0xef, 0xed, 0xf8),
];

/// Repo root, derived from this crate's manifest dir
/// (`tools/gate-checks/rsh-gate-ui` → three levels up).
#[must_use]
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
}

/// One check's outcome.
struct Outcome {
    passed: bool,
    detail: String,
}

impl Outcome {
    fn fail(detail: String) -> Self {
        Self {
            passed: false,
            detail,
        }
    }
}

/// Accumulates one check's sub-assertions.
struct Report {
    ok: bool,
    lines: Vec<String>,
}

impl Report {
    fn new() -> Self {
        Self {
            ok: true,
            lines: Vec::new(),
        }
    }

    fn check(&mut self, name: &str, passed: bool, detail: &str) {
        if !passed {
            self.ok = false;
        }
        self.lines.push(format!(
            "{}: {} — {detail}",
            name,
            if passed { "PASS" } else { "FAIL" }
        ));
    }

    fn finish(self) -> Outcome {
        Outcome {
            passed: self.ok,
            detail: self.lines.join(" | "),
        }
    }
}

/// Initialise the testing backend once (software renderer, so
/// `take_snapshot` works; embedded Noto Sans, so text metrics are
/// deterministic). Must run before any component is created.
fn init_platform() {
    i_slint_core::platform::set_platform(Box::new(TestingBackend::new(TestingBackendOptions {
        renderer_name: Some("software".into()),
        mock_time: true,
        threading: false,
    })))
    .expect("testing platform");
    // No configure_test_fonts(): the published crate's embedded
    // NotoSans paths point at the Slint repo's tests/ dir, which
    // is not shipped in the registry crate — and the gate's pixel
    // checks only read rectangles/fills, never glyphs, so system
    // font fallback is fine.
}

/// Compile `file` (repo-relative) and instantiate `component`.
fn compile(file: &str, component: &str) -> Result<ComponentInstance, String> {
    let root = repo_root();
    let path = root.join(file);
    let mut compiler = Compiler::default();
    compiler.set_include_paths(vec![root.join("crates/rsh-ui/ui")]);
    let result = spin_on::spin_on(compiler.build_from_path(path.clone()));
    if result.has_errors() {
        let diags: Vec<String> = result.diagnostics().map(|d| format!("{d:?}")).collect();
        return Err(format!("compile {file} failed: {}", diags.join("; ")));
    }
    result
        .component(component)
        .ok_or_else(|| format!("component {component} not found in {file}"))?
        .create()
        .map_err(|e| format!("create {component} failed: {e}"))
}

/// Render the instance and return raw RGBA bytes + size.
fn snapshot(instance: &ComponentInstance) -> Result<(Vec<u8>, u32, u32), String> {
    instance.show().map_err(|e| format!("show failed: {e}"))?;
    let buffer = instance
        .window()
        .take_snapshot()
        .map_err(|e| format!("snapshot failed: {e}"))?;
    Ok((buffer.as_bytes().to_vec(), buffer.width(), buffer.height()))
}

/// Count pixels within `tol` of `color`; also returns the bounding
/// box (in pixels, as `usize` — snapshots are far below 2^31 px).
#[must_use]
fn count_color(
    rgba: &[u8],
    width: u32,
    color: (i32, i32, i32),
    tol: i32,
) -> (usize, usize, usize, usize, usize) {
    let (mut count, mut min_x, mut max_x, mut min_y, mut max_y) =
        (0usize, usize::MAX, usize::MIN, usize::MAX, usize::MIN);
    for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
        let (red, green, blue) = (i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
        if (red - color.0).abs() <= tol
            && (green - color.1).abs() <= tol
            && (blue - color.2).abs() <= tol
        {
            count += 1;
            let px_x = i % width as usize;
            let px_y = i / width as usize;
            min_x = min_x.min(px_x);
            max_x = max_x.max(px_x);
            min_y = min_y.min(px_y);
            max_y = max_y.max(px_y);
        }
    }
    if count == 0 {
        (0, 0, 0, 0, 0)
    } else {
        (count, min_x, max_x, min_y, max_y)
    }
}

/// Count pixels matching any color in `family`.
#[must_use]
fn count_family(rgba: &[u8], width: u32, family: &[(i32, i32, i32)]) -> usize {
    family
        .iter()
        .map(|color| count_color(rgba, width, *color, 6).0)
        .sum()
}

/// Merge sorted pixel coordinates into runs, bridging gaps of at
/// most 4 px (rounded corners / anti-aliasing) and dropping runs
/// shorter than 50 px. Returns (start, length) pairs.
fn merge_runs(coords: &[usize]) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    let mut prev: usize = 0;
    for &coord in coords {
        match start {
            None => {
                start = Some(coord);
                prev = coord;
            }
            Some(_) if coord - prev <= 5 => {
                prev = coord;
            }
            Some(first) => {
                if prev + 1 - first >= 50 {
                    runs.push((first, prev + 1 - first));
                }
                start = Some(coord);
                prev = coord;
            }
        }
    }
    if let Some(first) = start.filter(|&first| prev + 1 - first >= 50) {
        runs.push((first, prev + 1 - first));
    }
    runs
}

/// Panel-coloured runs in one pixel column (`Theme.panel` #161C27,
/// the Card background). Used by the Extras card-size check: the
/// column x = 25 sits inside the left card's 14px padding, so it
/// crosses no inner element — a run is exactly one card. Returns
/// (top, height) pairs, top to bottom.
#[must_use]
fn panel_runs(rgba: &[u8], width: u32, height: u32, col: usize) -> Vec<(usize, usize)> {
    let mut ys: Vec<usize> = Vec::new();
    for y in 0..height as usize {
        let idx = (y * width as usize + col) * 4;
        let (red, green, blue) = (
            i32::from(rgba[idx]),
            i32::from(rgba[idx + 1]),
            i32::from(rgba[idx + 2]),
        );
        if (red - 0x16).abs() <= 6 && (green - 0x1c).abs() <= 6 && (blue - 0x27).abs() <= 6 {
            ys.push(y);
        }
    }
    merge_runs(&ys)
}

/// Panel-coloured runs along one horizontal pixel row (same colour
/// as `panel_runs`). Used to measure card WIDTHS: a scan through a
/// card's top padding strip (no content there) gives one run per
/// card, so the run lengths are the card widths.
#[must_use]
fn panel_row_runs(rgba: &[u8], width: u32, y: usize) -> Vec<(usize, usize)> {
    let mut xs: Vec<usize> = Vec::new();
    for col in 0..width as usize {
        let idx = (y * width as usize + col) * 4;
        let (red, green, blue) = (
            i32::from(rgba[idx]),
            i32::from(rgba[idx + 1]),
            i32::from(rgba[idx + 2]),
        );
        if (red - 0x16).abs() <= 6 && (green - 0x1c).abs() <= 6 && (blue - 0x27).abs() <= 6 {
            xs.push(col);
        }
    }
    merge_runs(&xs)
}

/// Bounding box (`min_x`, `min_y`, `max_x`, `max_y`, `count`) of pure-white
/// pixels in a region — the Switch knob (`background: white`, 14x14)
/// is the only solid white block in a card header (title text is
/// Theme.text #E9EDF4, never #FFFFFF; the OFF note is Theme.faint).
fn white_bbox(
    rgba: &[u8],
    width: u32,
    height: u32,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
) -> (usize, usize, usize, usize, usize) {
    let (mut count, mut min_x, mut max_x, mut min_y, mut max_y) =
        (0usize, usize::MAX, usize::MIN, usize::MAX, usize::MIN);
    for y in y0..y1.min(height as usize) {
        for x in x0..x1.min(width as usize) {
            let i = (y * width as usize + x) * 4;
            if rgba[i] == 255 && rgba[i + 1] == 255 && rgba[i + 2] == 255 {
                count += 1;
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
    }
    if count == 0 {
        (0, 0, 0, 0, 0)
    } else {
        (min_x, min_y, max_x, max_y, count)
    }
}

/// Accent-pixel count per row (used to find the slider fill band).
#[must_use]
fn accent_rows(rgba: &[u8], width: u32, height: u32) -> Vec<usize> {
    let mut rows = vec![0usize; height as usize];
    for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
        let (r, g, b) = (i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
        if (r - ACCENT.0).abs() <= 12 && (g - ACCENT.1).abs() <= 12 && (b - ACCENT.2).abs() <= 12 {
            rows[i / width as usize] += 1;
        }
    }
    rows
}

/// Accent pixels in the footer strip (bottom 30% of the window).
/// In the wizard probe this is exactly the enabled Next/Finish
/// primary button (76x30 accent fill); the disabled state renders
/// a panel2 rectangle instead, so a count well above zero means
/// next-enabled. `next-enabled` itself is a *private* property of
/// `SetupWizard` and cannot be read from outside the component — this
/// pixel read is the honest behavioral equivalent of reading it.
#[must_use]
fn footer_accent_count(rgba: &[u8], width: u32, height: u32) -> usize {
    let footer_first_row = height as usize * 70 / 100;
    rgba[footer_first_row * width as usize * 4..]
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| {
            (i32::from(px[0]) - ACCENT.0).abs() <= 12
                && (i32::from(px[1]) - ACCENT.1).abs() <= 12
                && (i32::from(px[2]) - ACCENT.2).abs() <= 12
        })
        .count()
}

/// Bounding box (`min_x`, `max_x`, `min_y`, `max_y`) of the accent pixels
/// in the footer strip, or `None` when the Next button is disabled.
///
/// The scan is restricted to the main column (x >= 264, the sidebar
/// width): the sidebar's "now" step circle is accent-filled too, and
/// with the preview-faithful 10px step-row padding (UI-UX polish) the
/// step-4 circle at step 3 sits inside the bottom 30% strip — a
/// whole-window union bbox would then centre on dead space between
/// the circle and the Finish button and the finish click would miss.
/// The footer button always lives in the main column, so sidebar
/// pixels can never be it.
#[must_use]
fn footer_accent_box(rgba: &[u8], width: u32, height: u32) -> Option<(usize, usize, usize, usize)> {
    let footer_first_row = height as usize * 70 / 100;
    let main_column_first_col = 264_usize;
    let mut bbox: Option<(usize, usize, usize, usize)> = None;
    for y in footer_first_row..height as usize {
        for x in main_column_first_col..width as usize {
            let idx = (y * width as usize + x) * 4;
            let (red, green, blue) = (
                i32::from(rgba[idx]),
                i32::from(rgba[idx + 1]),
                i32::from(rgba[idx + 2]),
            );
            if (red - ACCENT.0).abs() <= 12
                && (green - ACCENT.1).abs() <= 12
                && (blue - ACCENT.2).abs() <= 12
            {
                bbox = Some(match bbox {
                    None => (x, x, y, y),
                    Some((min_x, max_x, min_y, max_y)) => {
                        (min_x.min(x), max_x.max(x), min_y.min(y), max_y.max(y))
                    }
                });
            }
        }
    }
    bbox
}

/// Advance the mocked clock by `time_in_ms`, updating animations,
/// firing timers, and running change handlers — the same sequence
/// as the testing backend's `mock_elapsed_time`, reimplemented here
/// with public `i-slint-core` APIs (that helper is behind the
/// `internal` feature, which cannot compile from the published
/// crate — see `Cargo.toml`).
fn mock_elapsed_time(time_in_ms: u64) {
    let tick = i_slint_core::animations::CURRENT_ANIMATION_DRIVER.with(|driver| {
        let mut tick = driver.current_tick();
        tick += core::time::Duration::from_millis(time_in_ms);
        driver.update_animations(tick);
        tick
    });
    i_slint_core::timers::TimerList::maybe_activate_timers(tick);
    i_slint_core::properties::ChangeTracker::run_change_handlers();
}

/// Simulate a mouse click at logical (x, y): move + press, advance
/// the mock clock 50 ms, release — the same sequence as the testing
/// backend's `send_mouse_click`, reimplemented with public APIs.
fn click(instance: &ComponentInstance, x: f32, y: f32) {
    use i_slint_core::api::LogicalPosition;
    use i_slint_core::items::PointerEventButton;
    use i_slint_core::platform::WindowEvent;

    let adapter = i_slint_core::window::WindowInner::from_pub(instance.window()).window_adapter();
    let position = LogicalPosition::new(x, y);
    let button = PointerEventButton::Left;
    adapter
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    adapter
        .window()
        .dispatch_event(WindowEvent::PointerPressed { position, button });
    mock_elapsed_time(50);
    adapter
        .window()
        .dispatch_event(WindowEvent::PointerReleased { position, button });
}

/// Per-row counts of pixels within `tol` of `color`, restricted to
/// the column range `x0..x1`. Used to find a wrapped text's rendered
/// extent inside a bordered banner: the border rows carry a
/// full-width run of the border color and the side borders put 2 px
/// on every interior row, so the range excludes the border columns
/// (callers pass the background bbox inset by 2 px) and rows with a
/// small-but-nonzero count are text rows.
fn color_row_counts(
    rgba: &[u8],
    width: u32,
    color: (i32, i32, i32),
    tol: i32,
    x0: usize,
    x1: usize,
) -> Vec<usize> {
    let height = rgba.len() / 4 / width as usize;
    let mut rows = vec![0usize; height];
    for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
        let x = i % width as usize;
        if x < x0 || x >= x1 {
            continue;
        }
        let (red, green, blue) = (i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
        if (red - color.0).abs() <= tol
            && (green - color.1).abs() <= tol
            && (blue - color.2).abs() <= tol
        {
            rows[i / width as usize] += 1;
        }
    }
    rows
}

/// Bounding box of pixels within `tol` of `color`, restricted to
/// the rectangle `region` (x0, x1, y0, y1). Used for the real-MainWindow
/// toast: the window's Overview page contains other ok-bg elements
/// (status pills/bars), so the whole-snapshot bbox of ok-bg spans
/// the page, not the toast — the toast lives at x 312..792 (centred
/// 480 px card) and y 64.., so the region pins the measurement to
/// it. Returns (`count`, `min_x`, `max_x`, `min_y`, `max_y`) like
/// [`count_color`].
fn count_color_rect(
    rgba: &[u8],
    width: u32,
    color: (i32, i32, i32),
    tol: i32,
    region: (usize, usize, usize, usize),
) -> (usize, usize, usize, usize, usize) {
    let (x0, x1, y0, y1) = region;
    let (mut count, mut min_x, mut max_x, mut min_y, mut max_y) =
        (0usize, usize::MAX, usize::MIN, usize::MAX, usize::MIN);
    for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
        let px_x = i % width as usize;
        let px_y = i / width as usize;
        if px_x < x0 || px_x >= x1 || px_y < y0 || px_y >= y1 {
            continue;
        }
        let (red, green, blue) = (i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
        if (red - color.0).abs() <= tol
            && (green - color.1).abs() <= tol
            && (blue - color.2).abs() <= tol
        {
            count += 1;
            min_x = min_x.min(px_x);
            max_x = max_x.max(px_x);
            min_y = min_y.min(px_y);
            max_y = max_y.max(px_y);
        }
    }
    if count == 0 {
        (0, 0, 0, 0, 0)
    } else {
        (count, min_x, max_x, min_y, max_y)
    }
}

fn get_num(instance: &ComponentInstance, name: &str) -> Option<f64> {
    match instance.get_property(name) {
        Ok(Value::Number(value)) => Some(value),
        _ => None,
    }
}

fn get_bool(instance: &ComponentInstance, name: &str) -> Option<bool> {
    match instance.get_property(name) {
        Ok(Value::Bool(value)) => Some(value),
        _ => None,
    }
}

fn get_str(instance: &ComponentInstance, name: &str) -> Option<String> {
    match instance.get_property(name) {
        Ok(Value::String(value)) => Some(value.to_string()),
        _ => None,
    }
}

fn set_num(instance: &ComponentInstance, name: &str, value: f64) -> Result<(), String> {
    instance
        .set_property(name, Value::Number(value))
        .map_err(|e| format!("set {name} failed: {e}"))
}

fn set_bool(instance: &ComponentInstance, name: &str, value: bool) -> Result<(), String> {
    instance
        .set_property(name, Value::Bool(value))
        .map_err(|e| format!("set {name} failed: {e}"))
}

fn set_str(instance: &ComponentInstance, name: &str, value: &str) -> Result<(), String> {
    instance
        .set_property(name, Value::String(value.into()))
        .map_err(|e| format!("set {name} failed: {e}"))
}

/// Gate: slider visual (A6) — the fill must run min → thumb at
/// min/mid/max. Pixel proof: in the fill band (rows with many
/// accent pixels) the fill must start at the left edge and end at
/// `value * width`, growing monotonically with `value`.
#[allow(clippy::cast_precision_loss)]
fn ui_slider_visual() -> Outcome {
    let mut report = Report::new();
    let instance = match compile(
        "tools/gate-checks/rsh-gate-ui/ui/slider_probe.slint",
        "GateSliderWindow",
    ) {
        Ok(instance) => instance,
        Err(error) => return Outcome::fail(error),
    };
    let mut ends: Vec<f64> = Vec::new();
    for value in [0.25_f64, 0.5, 0.75] {
        if let Err(error) = set_num(&instance, "value", value) {
            return Outcome::fail(error);
        }
        let (rgba, width, height) = match snapshot(&instance) {
            Ok(snap) => snap,
            Err(error) => return Outcome::fail(error),
        };
        let rows = accent_rows(&rgba, width, height);
        // Fill band: rows with > 50 accent pixels (the 4 px fill);
        // the 14 px thumb outline contributes only a few per row.
        let band: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 50)
            .map(|(y, _)| y)
            .collect();
        if band.is_empty() {
            report.check(
                &format!("slider-fill-{value}"),
                false,
                "no fill band found in the snapshot",
            );
            continue;
        }
        let (mut min_x, mut max_x) = (usize::MAX, usize::MIN);
        for y in &band {
            for x in 0..width as usize {
                let i = (*y * width as usize + x) * 4;
                let (red, green, blue) = (
                    i32::from(rgba[i]),
                    i32::from(rgba[i + 1]),
                    i32::from(rgba[i + 2]),
                );
                if (red - ACCENT.0).abs() <= 12
                    && (green - ACCENT.1).abs() <= 12
                    && (blue - ACCENT.2).abs() <= 12
                {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                }
            }
        }
        let expected_end = value * f64::from(width);
        ends.push(max_x as f64);
        report.check(
            &format!("slider-fill-{value}"),
            min_x <= 8 && (max_x as f64 - expected_end).abs() <= 12.0,
            &format!(
                "fill starts at x={min_x} (must be <= 8) and ends at x={max_x} (expected {expected_end:.0})"
            ),
        );
    }
    report.check(
        "slider-fill-monotonic",
        ends.len() == 3 && ends[0] < ends[1] && ends[1] < ends[2],
        &format!("fill ends grow with value: {ends:?}"),
    );
    report.finish()
}

/// Gate: slider click (A6/A2) — clicking the track at 25% / 75%
/// must fire `changed` with ≈ 0.25 / 0.75.
#[allow(clippy::cast_precision_loss)]
fn ui_slider_click() -> Outcome {
    let mut report = Report::new();
    let instance = match compile(
        "tools/gate-checks/rsh-gate-ui/ui/slider_probe.slint",
        "GateSliderWindow",
    ) {
        Ok(instance) => instance,
        Err(error) => return Outcome::fail(error),
    };
    let last = Rc::new(RefCell::new(f64::NAN));
    {
        let last = Rc::clone(&last);
        if let Err(error) = instance.set_callback("changed", move |args| {
            if let Some(Value::Number(value)) = args.first() {
                *last.borrow_mut() = *value;
            }
            Value::Void
        }) {
            return Outcome::fail(format!("set changed handler failed: {error}"));
        }
    }
    if let Err(error) = set_num(&instance, "value", 0.5) {
        return Outcome::fail(error);
    }
    let (rgba, width, height) = match snapshot(&instance) {
        Ok(snap) => snap,
        Err(error) => return Outcome::fail(error),
    };
    // Track centre y: middle of the fill band.
    let rows = accent_rows(&rgba, width, height);
    let band: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 50)
        .map(|(y, _)| y)
        .collect();
    if band.is_empty() {
        return Outcome::fail("no fill band found — cannot locate the track".into());
    }
    let track_y =
        (band.first().copied().unwrap_or(0) + band.last().copied().unwrap_or(0)) as f32 / 2.0;
    for (x_frac, expected) in [(0.25_f32, 0.25_f64), (0.75_f32, 0.75_f64)] {
        click(&instance, x_frac * width as f32, track_y);
        let got = *last.borrow();
        report.check(
            &format!("slider-click-{expected}"),
            (got - expected).abs() <= 0.06,
            &format!(
                "click at {}% of the track fired changed({got:.2}), expected {expected:.2}",
                x_frac * 100.0
            ),
        );
    }
    report.finish()
}

/// One truth-table row: (case name, flags to set).
type GateCase<'a> = (&'a str, &'a [(&'a str, bool)]);

/// Gate: wizard gating + click-through (A2/A3).
///
/// `next-enabled` per step, then a real mouse click-through of
/// the Next button (located from the rendered accent fill) with
/// the production `on_setup_next` logic mirrored (step + 1,
/// finish at step 3). Skip/Back are proven via `invoke` (their
/// buttons are text, not locatable by colour in the frame).
#[allow(clippy::cast_precision_loss)]
#[allow(clippy::too_many_lines)]
fn ui_wizard_gating() -> Outcome {
    let mut report = Report::new();
    let instance = match compile(
        "tools/gate-checks/rsh-gate-ui/ui/wizard_probe.slint",
        "GateWizardWindow",
    ) {
        Ok(instance) => instance,
        Err(error) => return Outcome::fail(error),
    };
    // next-enabled truth table (setup.slint: `next-enabled` —
    // v2.2, 4 steps: 0 Connect, 1 Drivers, 2 Calibrate, 3 Games.
    // Step 2 is the Calibrate gate: Next is enabled when the
    // calibration backend is unavailable (explicit skip, D16) or
    // when centre + pedals are done).
    let cases: &[GateCase] = &[
        ("step0-blocked", &[("connect-done", false)]),
        ("step0-open", &[("connect-done", true)]),
        (
            "step1-blocked",
            &[("vjoy-installed", false), ("vjoy-fallback", false)],
        ),
        ("step1-vjoy", &[("vjoy-installed", true)]),
        ("step1-fallback", &[("vjoy-fallback", true)]),
        ("step2-no-calib", &[("calibration-available", false)]),
        (
            "step2-blocked",
            &[
                ("calibration-available", true),
                ("centre-done", false),
                ("pedals-done", false),
            ],
        ),
        (
            "step2-open",
            &[
                ("calibration-available", true),
                ("centre-done", true),
                ("pedals-done", true),
            ],
        ),
    ];
    // (step, expected next-enabled) per case, in order.
    let steps = [0, 0, 1, 1, 1, 2, 2, 2];
    let expected = [false, true, false, true, true, true, false, true];
    for (i, (name, flags)) in cases.iter().enumerate() {
        let step = steps[i];
        if let Err(error) = set_num(&instance, "step", f64::from(step)) {
            return Outcome::fail(error);
        }
        for prop in [
            "connect-done",
            "vjoy-installed",
            "vjoy-fallback",
            "calibration-available",
            "centre-done",
            "pedals-done",
        ] {
            if let Err(error) = set_bool(&instance, prop, false) {
                return Outcome::fail(error);
            }
        }
        for (prop, value) in *flags {
            if let Err(error) = set_bool(&instance, prop, *value) {
                return Outcome::fail(error);
            }
        }
        // next-enabled is private in SetupWizard: read it
        // behaviourally — the enabled Next button is an accent fill
        // in the footer, the disabled state is a panel2 rectangle.
        let (rgba, width, height) = match snapshot(&instance) {
            Ok(snap) => snap,
            Err(error) => return Outcome::fail(error),
        };
        let accent = footer_accent_count(&rgba, width, height);
        let got = accent > 100;
        report.check(
            &format!("wizard-gate-{name}"),
            got == expected[i],
            &format!(
                "step {step} {name}: next-enabled={got} (footer accent px={accent}), expected {}",
                expected[i]
            ),
        );
    }

    // Click-through: mirrored on_setup_next (step + 1, finish at 3).
    let step_state = Rc::new(Cell::new(0_i32));
    let finished = Rc::new(Cell::new(false));
    let clicks = Rc::new(Cell::new(0_usize));
    {
        let weak = instance.as_weak();
        let step_state = Rc::clone(&step_state);
        let finished = Rc::clone(&finished);
        let clicks = Rc::clone(&clicks);
        if let Err(error) = instance.set_callback("next", move |_| {
            clicks.set(clicks.get() + 1);
            let step = step_state.get();
            if step >= 3 {
                finished.set(true);
            } else {
                step_state.set(step + 1);
                if let Some(window) = weak.upgrade() {
                    let _ = window.set_property("step", Value::Number(f64::from(step + 1)));
                }
            }
            Value::Void
        }) {
            return Outcome::fail(format!("set next handler failed: {error}"));
        }
    }
    // Drive: step 0 (connect-done) → 1 (vjoy) → 2 (calibration
    // done) → 3 → finish.
    let drive: &[(&str, bool)] = &[
        ("connect-done", true),
        ("vjoy-installed", true),
        ("centre-done", true),
    ];
    if let Err(error) = set_num(&instance, "step", 0.0) {
        return Outcome::fail(error);
    }
    step_state.set(0);
    for (i, (prop, _)) in drive.iter().enumerate() {
        if *prop == "centre-done" {
            if let Err(error) = set_bool(&instance, "calibration-available", true) {
                return Outcome::fail(error);
            }
            if let Err(error) = set_bool(&instance, "pedals-done", true) {
                return Outcome::fail(error);
            }
        }
        if let Err(error) = set_bool(&instance, prop, true) {
            return Outcome::fail(error);
        }
        // Locate the enabled Next button: accent fill in the footer.
        let (rgba, width, height) = match snapshot(&instance) {
            Ok(snap) => snap,
            Err(error) => return Outcome::fail(error),
        };
        let Some(bx) = footer_accent_box(&rgba, width, height) else {
            report.check(
                &format!("wizard-click-step-{}", step_state.get()),
                false,
                "enabled Next button not found in the footer (no accent fill)",
            );
            break;
        };
        let cx = (bx.0 + bx.1) as f32 / 2.0;
        let cy = (bx.2 + bx.3) as f32 / 2.0;
        let before = clicks.get();
        click(&instance, cx, cy);
        let step_now = step_state.get();
        let want_step = i32::try_from(i).unwrap_or(-1) + 1;
        report.check(
            &format!("wizard-click-to-step-{want_step}"),
            clicks.get() == before + 1 && step_now == want_step,
            &format!("clicked Next at ({cx:.0},{cy:.0}): handler fired, step is now {step_now}"),
        );
        if step_now != want_step {
            break;
        }
    }
    // Finish: one more Next at step 3.
    let (rgba, width, height) = match snapshot(&instance) {
        Ok(snap) => snap,
        Err(error) => return Outcome::fail(error),
    };
    if let Some(bx) = footer_accent_box(&rgba, width, height) {
        click(
            &instance,
            (bx.0 + bx.1) as f32 / 2.0,
            (bx.2 + bx.3) as f32 / 2.0,
        );
    } else {
        report.check(
            "wizard-click-finish-nobox",
            false,
            "enabled Finish button not found in the footer (no accent fill)",
        );
    }
    report.check(
        "wizard-click-finish",
        finished.get(),
        "Next at step 3 finishes the wizard (Finish button)",
    );

    // Skip / Back via invoke (text buttons, not colour-locatable).
    let skipped = Rc::new(Cell::new(false));
    {
        let skipped = Rc::clone(&skipped);
        if let Err(error) = instance.set_callback("skip", move |_| {
            skipped.set(true);
            Value::Void
        }) {
            return Outcome::fail(format!("set skip handler failed: {error}"));
        }
    }
    if let Err(error) = instance.invoke("skip", &[]) {
        return Outcome::fail(format!("invoke skip failed: {error}"));
    }
    report.check(
        "wizard-skip-invokes",
        skipped.get(),
        "skip() callback fires (Skip for now button wiring)",
    );
    report.finish()
}

/// Gate: wizard fake data (A3) — the wizard's defaults are
/// placeholders, never fake live values.
fn ui_wizard_no_fake_data() -> Outcome {
    let mut report = Report::new();
    let instance = match compile(
        "tools/gate-checks/rsh-gate-ui/ui/wizard_probe.slint",
        "GateWizardWindow",
    ) {
        Ok(instance) => instance,
        Err(error) => return Outcome::fail(error),
    };
    report.check(
        "wizard-default-games",
        get_str(&instance, "games-text").as_deref() == Some("No games added yet."),
        "games-text defaults to 'No games added yet.'",
    );
    report.check(
        "wizard-default-hint",
        get_str(&instance, "connect-hint").as_deref() == Some(""),
        "connect-hint defaults to '' (no error to show)",
    );
    report.check(
        "wizard-default-flags",
        get_bool(&instance, "connect-done") == Some(false),
        "connect-done=false by default",
    );
    report.finish()
}

/// Gate: theme switch (A8) — setting `theme` repaints the whole
/// window: dark surfaces dominate in Dark, light surfaces in
/// Light, Auto renders as Dark.
fn ui_theme_switch() -> Outcome {
    let mut report = Report::new();
    let instance = match compile("crates/rsh-ui/ui/main.slint", "MainWindow") {
        Ok(instance) => instance,
        Err(error) => return Outcome::fail(error),
    };
    for (theme, want_dark) in [(0_i32, true), (1, false), (2, true)] {
        if let Err(error) = set_num(&instance, "theme", f64::from(theme)) {
            return Outcome::fail(error);
        }
        let (rgba, width, height) = match snapshot(&instance) {
            Ok(snap) => snap,
            Err(error) => return Outcome::fail(error),
        };
        let total = (width * height) as usize;
        let dark = count_family(&rgba, width, DARK_FAMILY);
        let light = count_family(&rgba, width, LIGHT_FAMILY);
        let dark_pct = dark * 100 / total.max(1);
        let light_pct = light * 100 / total.max(1);
        let name = match theme {
            0 => "theme-dark",
            1 => "theme-light",
            _ => "theme-auto",
        };
        report.check(
            name,
            if want_dark {
                dark_pct > 40 && light_pct < 1
            } else {
                light_pct > 40 && dark_pct < 1
            },
            &format!("theme {theme}: dark {dark_pct}% / light {light_pct}% of pixels"),
        );
    }
    report.finish()
}

/// Gate: tray popup disconnected state (A12) — defaults show
/// "Disconnected" with zero bars and a red dot; setting live
/// values flips the dot green and fills the bars. Proves the
/// popup renders live data, not hardcoded values.
fn ui_tray_disconnected() -> Outcome {
    let mut report = Report::new();
    let instance = match compile("crates/rsh-daemon/ui/tray.slint", "TrayPopup") {
        Ok(instance) => instance,
        Err(error) => return Outcome::fail(error),
    };
    report.check(
        "tray-default-label",
        get_str(&instance, "conn-label").as_deref() == Some("Disconnected")
            && get_bool(&instance, "connected") == Some(false),
        "conn-label defaults to 'Disconnected', connected=false",
    );
    report.check(
        "tray-default-bars",
        get_str(&instance, "throttle-text").as_deref() == Some("0%")
            && get_str(&instance, "steer-text").as_deref() == Some("+0°"),
        "throttle '0%', steer '+0°' by default (no fake live values)",
    );
    let (rgba, width, _) = match snapshot(&instance) {
        Ok(snap) => snap,
        Err(error) => return Outcome::fail(error),
    };
    let (red, _, _, _, _) = count_color(&rgba, width, RED, 12);
    let (green, _, _, _, _) = count_color(&rgba, width, GREEN, 12);
    report.check(
        "tray-disconnected-dot-red",
        red > 0 && green == 0,
        &format!("disconnected: red dot pixels={red}, green={green}"),
    );
    if let Err(error) = set_bool(&instance, "connected", true) {
        return Outcome::fail(error);
    }
    if let Err(error) = set_str(&instance, "conn-label", "Connected") {
        return Outcome::fail(error);
    }
    if let Err(error) = set_num(&instance, "throttle", 0.75) {
        return Outcome::fail(error);
    }
    if let Err(error) = set_str(&instance, "throttle-text", "75%") {
        return Outcome::fail(error);
    }
    let (rgba, width, _) = match snapshot(&instance) {
        Ok(snap) => snap,
        Err(error) => return Outcome::fail(error),
    };
    let (red, _, _, _, _) = count_color(&rgba, width, RED, 12);
    let (green, _, _, _, _) = count_color(&rgba, width, GREEN, 12);
    report.check(
        "tray-connected-dot-green",
        green > 0 && red == 0,
        &format!("connected: green dot pixels={green}, red={red}"),
    );
    report.finish()
}

/// Gate: tray popup Light theme (DS-22, U-04) — the popup was
/// hardcoded dark. `light-theme` repaints it: the background is
/// #11161F in Dark and #FFFFFF in Light (token `tray-bg`).
/// Pixel proof via the flat background colour (tol 4 on the dark
/// side so Light-mode text #171B26, which is 6/5/7 away, is not
/// mistaken for the dark background).
fn ui_tray_light_theme() -> Outcome {
    let mut report = Report::new();
    let instance = match compile("crates/rsh-daemon/ui/tray.slint", "TrayPopup") {
        Ok(instance) => instance,
        Err(error) => return Outcome::fail(error),
    };
    for (light, want_light) in [(false, false), (true, true)] {
        if let Err(error) = set_bool(&instance, "light-theme", light) {
            return Outcome::fail(error);
        }
        let (rgba, width, height) = match snapshot(&instance) {
            Ok(snap) => snap,
            Err(error) => return Outcome::fail(error),
        };
        let total = (width * height) as usize;
        let (dark_bg, _, _, _, _) = count_color(&rgba, width, (0x11, 0x16, 0x1f), 4);
        let (light_bg, _, _, _, _) = count_color(&rgba, width, (0xff, 0xff, 0xff), 12);
        let dark_pct = dark_bg * 100 / total.max(1);
        let light_pct = light_bg * 100 / total.max(1);
        let name = if light {
            "tray-light-bg-white"
        } else {
            "tray-dark-bg"
        };
        report.check(
            name,
            if want_light {
                light_pct > 40 && dark_pct == 0
            } else {
                // Dark keeps a little white (switch knob, "R" logo,
                // "Launch" labels) — under 2% of the popup.
                dark_pct > 40 && light_pct < 2
            },
            &format!("light-theme={light}: dark-bg {dark_pct}% / light-bg {light_pct}% of pixels"),
        );
    }
    report.finish()
}

/// Panel2-coloured runs along one horizontal pixel row
/// (`Theme.panel2` #1D2533, the `GameTile` background) — the tile
/// runs on a tile row. Mirrors `panel_row_runs` for the tile
/// colour.
#[must_use]
fn panel2_row_runs(rgba: &[u8], width: u32, y: usize) -> Vec<(usize, usize)> {
    let mut xs: Vec<usize> = Vec::new();
    for col in 0..width as usize {
        let idx = (y * width as usize + col) * 4;
        let (red, green, blue) = (
            i32::from(rgba[idx]),
            i32::from(rgba[idx + 1]),
            i32::from(rgba[idx + 2]),
        );
        if (red - 0x1d).abs() <= 6 && (green - 0x25).abs() <= 6 && (blue - 0x33).abs() <= 6 {
            xs.push(col);
        }
    }
    merge_runs(&xs)
}

/// R3-UI-06: measure the games-grid tile gap from a snapshot.
/// The row with the most panel2 pixels is a tile padding strip
/// (two full tile widths — no other row on the page comes close:
/// tile content rows lose the icon + button widths, the wheel
/// card has no wide panel2 run), and the gap is the space between
/// the two tile runs on that row. Returns (gap, y) — the gap
/// includes the two 1 px tile borders, so it reads ~2 px over the
/// `spacing` value.
#[must_use]
fn games_tile_gap(rgba: &[u8], width: u32, height: u32) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for y in 0..height as usize {
        let count = panel2_row_runs(rgba, width, y)
            .iter()
            .map(|(_, len)| len)
            .sum::<usize>();
        if best.is_none_or(|(best_count, _)| count > best_count) {
            best = Some((count, y));
        }
    }
    let (_, y) = best?;
    let runs = panel2_row_runs(rgba, width, y);
    if runs.len() < 2 {
        return None;
    }
    // Runs come back left to right; the two tiles are the first
    // two (a padding strip has no other panel2 run).
    let (start0, len0) = runs[0];
    let (start1, _) = runs[1];
    Some((start1.saturating_sub(start0 + len0), y))
}

/// Gate: games-grid tile gap is responsive (R3-UI-06) — 10 px
/// spacing at the 736 px design width, growing with the window.
/// Pixel proof on the real `OverviewPage`: the measured gap
/// (spacing + the two 1 px tile borders, so ~12 px at design)
/// must be ~12 px at 736 px and ~19 px at 1016 px
/// (`10 + (1016-736) * 0.025 + 2`), i.e. the gap grows ~7 px
/// over the same range the R-4 spacers grow ~7 px.
fn ui_games_grid_gap() -> Outcome {
    let mut report = Report::new();
    let mut gaps: Vec<(u32, usize)> = Vec::new();
    for (component, exp_gap) in [
        ("GateOverviewWindow736", 12_usize),
        ("GateOverviewWindow1016", 19_usize),
    ] {
        let instance = match compile(
            "tools/gate-checks/rsh-gate-ui/ui/overview_probe.slint",
            component,
        ) {
            Ok(instance) => instance,
            Err(error) => return Outcome::fail(error),
        };
        let (rgba, width, height) = match snapshot(&instance) {
            Ok(snap) => snap,
            Err(error) => return Outcome::fail(error),
        };
        let Some((gap, y)) = games_tile_gap(&rgba, width, height) else {
            return Outcome::fail(format!("{component}: no tile row found"));
        };
        gaps.push((width, gap));
        report.check(
            &format!("{component}-gap"),
            gap.abs_diff(exp_gap) <= 3,
            &format!("tile gap {gap} px on row y={y} (expected ~{exp_gap} px)"),
        );
    }
    if gaps.len() == 2 {
        let growth = gaps[1].1.saturating_sub(gaps[0].1);
        report.check(
            "games-gap-grows",
            (5..=9).contains(&growth),
            &format!("gap grows {growth} px from 736 to 1016 px (expected ~7 px)"),
        );
    }
    report.finish()
}

/// Gate: error banner wraps + grows (DS-21, U-03) — the banner was
/// a fixed 36 px with no wrapping, so a 150-char daemon-error was
/// cut off mid-sentence. A short string stays 36 px; the 150-char
/// string (the plan's verification case) must grow the banner past
/// 40 px, proving the text wraps instead of clipping.
/// Render the real `ExtrasScreen` in one on/off state and return the
/// left-column card runs (top, height) from the panel column x = 25,
/// plus the raw snapshot (for the horizontal width scan).
/// A fresh instance per state: a shown Window keeps its size, so
/// reusing one instance would contaminate the readings.
///
/// One Extras render: the card runs plus the raw snapshot bytes and
/// width (the height is only needed to take the snapshot).
type ExtrasSnapshot = (Vec<(usize, usize)>, Vec<u8>, u32);

fn extras_runs(flags: [bool; 4]) -> Result<ExtrasSnapshot, String> {
    let [handbrake, shifter, horn, leds] = flags;
    let instance = compile(
        "tools/gate-checks/rsh-gate-ui/ui/cards_probe.slint",
        "GateExtrasWindow",
    )?;
    set_bool(&instance, "handbrake-on", handbrake)?;
    set_bool(&instance, "shifter-on", shifter)?;
    set_bool(&instance, "horn-on", horn)?;
    set_bool(&instance, "leds-on", leds)?;
    let (rgba, width, height) = snapshot(&instance)?;
    Ok((panel_runs(&rgba, width, height, 25), rgba, width))
}

/// Gate: Extras card size (Bug 2) — the 4 module cards must be the
/// SAME size ON and OFF. Pixel proof on the real `ExtrasScreen`: the
/// row-1 card height and the row-2 top must be identical in the
/// all-ON, one-OFF and all-OFF states (tolerance ±3 px for font
/// metrics). The old `if` branches collapsed a card to ~71 px when
/// off (grid 1030 px → 190 px); the fix keeps the body in the tree
/// with `visible`, so both rows stay at their ON height.
///
/// R3-UI-01 / WS-5b: the row-2 height is asserted too, not just its
/// top. Before this, `runs[1]` (Horn, left column row 2) was read
/// only for its top — a Horn/LEDs-only collapse would have kept
/// row-1 and row-2-top identical and passed silently. Row-2 height
/// now has the same floor + cross-state equality as row-1, so all
/// four cards are covered: row-1 proves Handbrake/Shifter, row-2
/// proves Horn/LEDs (right-column cards share their row height by
/// `HorizontalLayout` cross-axis stretch).
#[allow(clippy::cast_precision_loss)]
fn ui_extras_card_size() -> Outcome {
    let mut report = Report::new();
    let states = [
        ("all-on", true, true, true, true),
        ("handbrake-off", false, true, true, true),
        ("all-off", false, false, false, false),
    ];
    let mut runs_by_state: Vec<(&str, ExtrasSnapshot)> = Vec::new();
    for (name, handbrake, shifter, horn, leds) in states {
        match extras_runs([handbrake, shifter, horn, leds]) {
            Ok(snapshot) => runs_by_state.push((name, snapshot)),
            Err(error) => return Outcome::fail(format!("{name}: {error}")),
        }
    }
    let mut first: Option<(usize, usize)> = None;
    let mut first_top: Option<usize> = None;
    let mut first_row2_h: Option<usize> = None;
    for (name, (runs, rgba, width)) in &runs_by_state {
        if runs.len() != 2 {
            return Outcome::fail(format!(
                "{name}: expected 2 card runs, got {} ({runs:?})",
                runs.len()
            ));
        }
        let (row1_top, row1_h) = runs[0];
        let (row2_top, row2_h) = runs[1];
        report.check(
            &format!("{name}-row1-tall"),
            row1_h > 300,
            &format!("row-1 card height {row1_h} px (ON body present, not collapsed)"),
        );
        // R3-UI-01 / WS-5b: row-2 floor. Row-2 measures 189 px
        // all-ON (Horn/LEDs row), so 150 px separates "body
        // present" from a collapsed ~73 px OFF card with margin
        // on both sides.
        report.check(
            &format!("{name}-row2-tall"),
            row2_h > 150,
            &format!("row-2 card height {row2_h} px (ON body present, not collapsed)"),
        );
        // Card widths (R-10): the two row-1 cards share the row
        // equally — 342 px each at design width — because every
        // Extras card has `min-width: 342px`. Scanned through the
        // top padding strip (y = run start + 6, no content there),
        // so each run is exactly one card.
        let row_runs = panel_row_runs(rgba, *width, row1_top + 6);
        let widths_ok = row_runs.len() == 2 && row_runs.iter().all(|(_, w)| w.abs_diff(342) <= 12);
        report.check(
            &format!("{name}-cards-equal-width"),
            widths_ok,
            &format!("row-1 card widths {row_runs:?} (expected two ~342 px runs)"),
        );
        match first {
            None => {
                first = Some((row1_top, row1_h));
                first_top = Some(row2_top);
                first_row2_h = Some(row2_h);
            }
            Some((top0, h0)) => {
                report.check(
                    &format!("{name}-row1-same-height"),
                    row1_h.abs_diff(h0) <= 3 && row1_top.abs_diff(top0) <= 3,
                    &format!("row-1 {row1_h} px at y={row1_top} vs all-on {h0} px at y={top0}"),
                );
                report.check(
                    &format!("{name}-row2-same-top"),
                    row2_top.abs_diff(first_top.unwrap_or(0)) <= 3,
                    &format!(
                        "row-2 top y={row2_top} vs all-on y={}",
                        first_top.unwrap_or(0)
                    ),
                );
                // R3-UI-01 / WS-5b: row-2 height identical across
                // states — the Horn/LEDs half of the guarantee.
                report.check(
                    &format!("{name}-row2-same-height"),
                    row2_h.abs_diff(first_row2_h.unwrap_or(0)) <= 3,
                    &format!(
                        "row-2 {row2_h} px vs all-on {} px",
                        first_row2_h.unwrap_or(0)
                    ),
                );
            }
        }
    }
    report.finish()
}

/// Gate: Extras OFF card takes no input, header switch stays live
/// (Bug 2 safety). With Horn OFF: a click on the body area (where
/// the "Learn from wheel" button sits when ON) must fire nothing —
/// the invisible body takes no input — while a click on the header
/// Switch must flip `horn-on` back on and fire `horn-toggle`.
#[allow(clippy::cast_precision_loss)]
fn ui_extras_off_no_input() -> Outcome {
    let mut report = Report::new();
    let instance = match compile(
        "tools/gate-checks/rsh-gate-ui/ui/cards_probe.slint",
        "GateExtrasWindow",
    ) {
        Ok(instance) => instance,
        Err(error) => return Outcome::fail(error),
    };
    let last = Rc::new(RefCell::new(String::new()));
    {
        let last = Rc::clone(&last);
        if let Err(error) = instance.set_callback("action", move |args| {
            if let Some(Value::String(value)) = args.first() {
                *last.borrow_mut() = value.to_string();
            }
            Value::Void
        }) {
            return Outcome::fail(format!("set action handler failed: {error}"));
        }
    }
    if let Err(error) = set_bool(&instance, "horn-on", false) {
        return Outcome::fail(error);
    }
    let (rgba, width, height) = match snapshot(&instance) {
        Ok(snap) => snap,
        Err(error) => return Outcome::fail(error),
    };
    let runs = panel_runs(&rgba, width, height, 25);
    if runs.len() != 2 {
        return Outcome::fail(format!(
            "expected 2 card runs, got {} ({runs:?})",
            runs.len()
        ));
    }
    let row2_top = runs[1].0 as f32;
    // Body area: 80 px below the Horn card top — past the header
    // (~34 px) and the "Off, turn on to edit" note, inside the
    // invisible body (the Learn/Test button row when ON).
    click(&instance, 200.0, row2_top + 80.0);
    report.check(
        "extras-off-body-no-input",
        last.borrow().is_empty(),
        &format!(
            "click on the OFF Horn body fired '{}' (expected nothing)",
            last.borrow()
        ),
    );
    // Header Switch, located from its knob (pure white, 14x14 — the
    // only solid white block in the header strip): click the knob
    // centre, which sits inside the switch's TouchArea. Hardcoding
    // the switch position from the card geometry missed (the panel
    // run starts a few px below the true card top — rounded corners
    // — so a geometry-derived y lands outside the 20 px switch).
    let (kx0, ky0, kx1, ky1, kcount) =
        white_bbox(&rgba, width, height, 20, 362, runs[1].0, runs[1].0 + 40);
    let knob_ok = kcount >= 100 && kx1 - kx0 <= 20 && ky1 - ky0 <= 20;
    report.check(
        "extras-off-switch-located",
        knob_ok,
        &format!(
            "Horn switch knob at x={kx0}..{kx1} y={ky0}..{ky1} ({kcount} white px, expected a ~14x14 block)"
        ),
    );
    if knob_ok {
        let cx = (kx0 + kx1) as f32 / 2.0;
        let cy = (ky0 + ky1) as f32 / 2.0;
        click(&instance, cx, cy);
    }
    report.check(
        "extras-off-switch-toggles",
        get_bool(&instance, "horn-on-state") == Some(true),
        "header Switch click flipped the screen's horn-on back to true (read via the probe mirror property)",
    );
    report.check(
        "extras-off-switch-action",
        last.borrow().as_str() == "horn-toggle",
        &format!(
            "header Switch click fired '{}' (expected 'horn-toggle')",
            last.borrow()
        ),
    );
    report.finish()
}

/// Gate: card gaps scale with the window (Bug 1) — pixel proof on
/// the spacer pattern the screens use. At the 736 px design width
/// the gap is exactly 12 px and each card 362 px; at 1016 px the
/// gap grows to ~19 px and the cards to ~498 px (extra space split
/// by stretch: cards 1, spacer 0.05).
fn ui_card_gap_responsive() -> Outcome {
    let mut report = Report::new();
    for (component, exp_left, exp_gap) in [
        ("GateGapWindow736", 362.0_f64, 12.0_f64),
        ("GateGapWindow1016", 498.0_f64, 19.0_f64),
    ] {
        let instance = match compile(
            "tools/gate-checks/rsh-gate-ui/ui/cards_probe.slint",
            component,
        ) {
            Ok(instance) => instance,
            Err(error) => return Outcome::fail(error),
        };
        // Snapshot forces layout; the measurements come off the
        // probe's out properties.
        if let Err(error) = snapshot(&instance) {
            return Outcome::fail(error);
        }
        let left = get_num(&instance, "left-w").unwrap_or(-1.0);
        let gap = get_num(&instance, "gap-w").unwrap_or(-1.0);
        let right = get_num(&instance, "right-w").unwrap_or(-1.0);
        report.check(
            &format!("{component}-card-width"),
            (left - exp_left).abs() <= 3.0 && (right - exp_left).abs() <= 3.0,
            &format!("cards {left:.0}/{right:.0} px (expected {exp_left:.0} px)"),
        );
        report.check(
            &format!("{component}-gap"),
            (gap - exp_gap).abs() <= 3.0,
            &format!("gap {gap:.1} px (expected {exp_gap:.0} px)"),
        );
    }
    report.finish()
}

fn ui_banner_wrap() -> Outcome {
    let mut report = Report::new();
    let instance = match compile(
        "tools/gate-checks/rsh-gate-ui/ui/banner_probe.slint",
        "GateBannerWindow",
    ) {
        Ok(instance) => instance,
        Err(error) => return Outcome::fail(error),
    };
    if let Err(error) = set_str(&instance, "banner-text", "Short text.") {
        return Outcome::fail(error);
    }
    // The height is only laid out once the window is shown — read it
    // off a snapshot, not off the unshown instance.
    if let Err(error) = snapshot(&instance) {
        return Outcome::fail(error);
    }
    let short = get_num(&instance, "banner-height");
    let long_text = "Daemon failed to start: IPC bind failed after 5 attempts — pipe is held by another process or the previous daemon did not exit cleanly. Close the old RSH app and retry.";
    assert!(
        long_text.chars().count() > 150,
        "probe text must exceed 150 chars"
    );
    if let Err(error) = set_str(&instance, "banner-text", long_text) {
        return Outcome::fail(error);
    }
    if let Err(error) = snapshot(&instance) {
        return Outcome::fail(error);
    }
    let long = get_num(&instance, "banner-height");
    report.check(
        "banner-short-36px",
        short == Some(36.0),
        &format!("short text banner height: {short:?} (want 36)"),
    );
    report.check(
        "banner-long-grows",
        long.is_some_and(|h| h > 40.0),
        &format!("150-char text banner height: {long:?} (want > 40)"),
    );
    report.finish()
}

/// Gate: toast size (Bug 2) — the DS-24 toast is a designed
/// 480×~52 px card, but the user PC rendered it at ~70% window
/// width and ~30% window height. The probe hosts the toast markup
/// verbatim from `crates/rsh-ui/ui/main.slint` and measures its
/// rendered height (layout property + pixel bbox of the toast
/// background) for a short body, the flap body, and a long error
/// body. Acceptance: width stays 480 px and height stays ≤ 80 px
/// on every body — a toast is a small card, never a panel.
#[allow(clippy::cast_precision_loss)]
fn ui_toast_size() -> Outcome {
    let mut report = Report::new();
    let instance = match compile(
        "tools/gate-checks/rsh-gate-ui/ui/toast_probe.slint",
        "GateToastWindow",
    ) {
        Ok(instance) => instance,
        Err(error) => return Outcome::fail(error),
    };
    // (title, body, kind, label) cases.
    let cases: [(&str, &str, &str, &str); 3] = [
        ("RSH connected", "Live data is back.", "success", "short"),
        (
            "RSH connected",
            "Live data is back — the connection flapped ×3 before stabilising.",
            "success",
            "flap",
        ),
        (
            "Couldn't complete that",
            // The longest body production can show: set_toast caps
            // bodies at 120 chars (the uncapped error is 200 chars;
            // the full text stays in the drawer). The gate renders
            // the capped form, as the user sees it.
            "Couldn't reach the wheel — the connection timed out after 10 s. Check the RSH-XXXX Wi-Fi is joined (or the USB cable is …",
            "error",
            "long",
        ),
    ];
    for (title, body, kind, label) in cases {
        if let Err(error) = set_str(&instance, "toast-title", title)
            .and_then(|()| set_str(&instance, "toast-text", body))
            .and_then(|()| set_str(&instance, "toast-kind", kind))
        {
            return Outcome::fail(error);
        }
        let (rgba, width, _height) = match snapshot(&instance) {
            Ok(snap) => snap,
            Err(error) => return Outcome::fail(error),
        };
        let height = get_num(&instance, "toast-height");
        let toast_width = get_num(&instance, "toast-width");
        // Pixel bbox of the toast background (ok-bg #1B3635 /
        // err-bg #2E1F26): the rendered height, independent of the
        // layout property read.
        let bg: (i32, i32, i32) = if kind == "success" {
            (0x1b, 0x36, 0x35)
        } else {
            (0x2e, 0x1f, 0x26)
        };
        let (_, _, _, top, bottom) = count_color(&rgba, width, bg, 6);
        let px_height = if bottom >= top { bottom - top + 1 } else { 0 };
        report.check(
            &format!("toast-{label}-width-480px"),
            toast_width == Some(480.0),
            &format!("{label} toast width: {toast_width:?} (want 480)"),
        );
        report.check(
            &format!("toast-{label}-height-compact"),
            height.is_some_and(|h| h <= 80.0),
            &format!(
                "{label} toast height: {height:?} px (property), {px_height} px (pixels) (want <= 80)"
            ),
        );
    }
    report.finish()
}

/// Gate: banner with action button (Bug 3) — the DS-21 probe covers
/// the button-less banner, but the Overview banners carry a
/// "Reconnect" button that eats ~100 px of text width, inside a
/// `ScrollView` at the 736 px content-column width. The probe renders
/// the real `EmptyBanner` in exactly that setting with both exact
/// Overview texts and measures (a) the banner height — it must grow
/// past 36 px for the two-line text — and (b) the wrapped text's
/// rendered extent from the snapshot: the last text row must sit
/// inside the banner (≥ 6 px above its bottom edge), so the second
/// line is never half cut off by the card painted below it.
#[allow(clippy::cast_precision_loss)]
fn ui_banner_action_wrap() -> Outcome {
    let mut report = Report::new();
    let instance = match compile(
        "tools/gate-checks/rsh-gate-ui/ui/banner_probe.slint",
        "GateBannerActionWindow",
    ) {
        Ok(instance) => instance,
        Err(error) => return Outcome::fail(error),
    };
    let texts = [
        (
            "Wheel not found — join the wheel's RSH-XXXX Wi-Fi network or plug in USB.",
            "wheel-not-found",
            false,
        ),
        (
            "Wheel not connected — check the wheel's Wi-Fi is joined or the USB cable is plugged in.",
            "wheel-not-connected",
            false,
        ),
        (
            // The daemon-error banner (overview.slint) carries the
            // same Reconnect button — and this 150-char text wraps
            // to two lines even with the gate's narrow fallback
            // font, so it exercises the wrap path the ~100-char
            // Overview texts only hit under Inter on Windows.
            "Daemon failed to start: IPC bind failed after 5 attempts — pipe is held by another process or the previous daemon did not exit cleanly. Close the old RSH app and retry.",
            "daemon-error-long",
            true,
        ),
    ];
    for (text, label, expect_wrap) in texts {
        if let Err(error) = set_str(&instance, "banner-text", text) {
            return Outcome::fail(error);
        }
        let (rgba, width, _height) = match snapshot(&instance) {
            Ok(snap) => snap,
            Err(error) => return Outcome::fail(error),
        };
        let banner_height = get_num(&instance, "banner-height");
        // Banner interior from its err-bg (#2E1F26) pixels.
        let (_, bg_min_x, bg_max_x, bg_top, bg_bottom) =
            count_color(&rgba, width, (0x2e, 0x1f, 0x26), 6);
        // Text rows from Theme.bad (#FF6B6B) glyph pixels, counted
        // only inside the banner interior (the side borders put bad
        // pixels on every row — without the inset every interior
        // row reads as a text row). The last text row is the bottom
        // of the wrapped text as actually painted.
        let rows = color_row_counts(
            &rgba,
            width,
            (0xff, 0x6b, 0x6b),
            60,
            bg_min_x + 2,
            bg_max_x.saturating_sub(1),
        );
        // (TEMP-DEBUG removed — the row histogram showed the text
        // painting as a single 35..46 band, i.e. the ~100-char
        // Overview texts fit on ONE line with this backend's font;
        // the wrap path needs a longer text — see the third case
        // below.)
        // Text rows carry >= 3 glyph pixels; the banner's rounded
        // corners put exactly 2 border pixels on the first/last
        // interior rows, so the range starts at 3 to exclude them.
        let text_bottom = rows
            .iter()
            .enumerate()
            .filter(|&(_, &count)| (3..=200).contains(&count))
            .map(|(y, _)| y)
            .max();
        let text_top = rows
            .iter()
            .enumerate()
            .filter(|&(_, &count)| (3..=200).contains(&count))
            .map(|(y, _)| y)
            .min();
        let fits = match (text_bottom, bg_bottom.checked_sub(bg_top)) {
            (Some(last), Some(_)) => last + 6 <= bg_bottom,
            _ => false,
        };
        let two_lines = match (text_top, text_bottom) {
            (Some(first), Some(last)) => last - first >= 20,
            _ => false,
        };
        report.check(
            &format!("banner-{label}-grows"),
            // The button is 30 px tall, so the layout is 30 px
            // whether the text is one line or two — the banner is
            // 46 px in both cases. Growth only shows past two
            // lines; the wrap case is judged by the text extent,
            // not the banner height.
            banner_height.is_some_and(|h| h >= 46.0),
            &format!("{label} banner height: {banner_height:?} (want >= 46)"),
        );
        // The wrap assertion only applies where the text is known
        // to wrap in this backend (the long daemon-error text);
        // the ~100-char Overview texts fit on one line here and
        // wrap only under Inter on Windows.
        if expect_wrap {
            report.check(
                &format!("banner-{label}-two-lines"),
                two_lines,
                &format!(
                    "{label} text spans rows {text_top:?}..{text_bottom:?} (want two lines, >= 20 px apart)"
                ),
            );
        }
        report.check(
            &format!("banner-{label}-text-fits"),
            fits,
            &format!(
                "{label} last text row {text_bottom:?}, banner bg bottom {bg_bottom} (want text bottom + 6 <= banner bottom)"
            ),
        );
    }
    report.finish()
}

/// Gate: real `MainWindow` toast (Bug 2) — the verbatim probes above
/// isolate the mechanism (a height-less Rectangle parented by the
/// Window / a 100% `FocusScope` measures window-height), this check
/// proves the shipped window: instantiate the real `MainWindow`,
/// set its toast props, and measure the toast background's pixel
/// bbox. Acceptance: the rendered toast is ≤ 80 px tall on a short
/// body and on a long error body, at 480 px wide.
#[allow(clippy::cast_precision_loss)]
fn ui_toast_real_mainwindow() -> Outcome {
    let mut report = Report::new();
    // FocusScope-parented verbatim copy (MainWindow's parenting):
    // the layout property alone proves the mechanism.
    let focus = match compile(
        "tools/gate-checks/rsh-gate-ui/ui/toast_probe.slint",
        "GateToastFocusWindow",
    ) {
        Ok(instance) => instance,
        Err(error) => return Outcome::fail(error),
    };
    if let Err(error) = snapshot(&focus) {
        return Outcome::fail(error);
    }
    let focus_height = get_num(&focus, "toast-height");
    let focus_width = get_num(&focus, "toast-width");
    report.check(
        "toast-focus-parent-height-compact",
        focus_height.is_some_and(|h| h <= 80.0),
        &format!("FocusScope-parented toast height: {focus_height:?} (want <= 80)"),
    );
    report.check(
        "toast-focus-parent-width-480px",
        focus_width == Some(480.0),
        &format!("FocusScope-parented toast width: {focus_width:?} (want 480)"),
    );
    // The real MainWindow, short + long bodies (pixel bbox of the
    // toast background — the window exposes no toast-height prop).
    // One component per variant: the interpreter cannot set
    // inherited properties on an instance.
    let cases: [(&str, &str, &str); 2] = [
        ("GateMainToastShort", "success", "short"),
        ("GateMainToastLong", "error", "long"),
    ];
    for (component, kind, label) in cases {
        let instance = match compile(
            "tools/gate-checks/rsh-gate-ui/ui/toast_probe.slint",
            component,
        ) {
            Ok(instance) => instance,
            Err(error) => return Outcome::fail(error),
        };
        let (rgba, width, _height) = match snapshot(&instance) {
            Ok(snap) => snap,
            Err(error) => return Outcome::fail(error),
        };
        let bg: (i32, i32, i32) = if kind == "success" {
            (0x1b, 0x36, 0x35)
        } else {
            (0x2e, 0x1f, 0x26)
        };
        // Region-restricted to the toast card (x 312..792, y 64..250):
        // the Overview page behind it has other ok-bg elements.
        let (_, _, _, top, bottom) = count_color_rect(&rgba, width, bg, 6, (312, 792, 64, 250));
        let px_height = if bottom >= top { bottom - top + 1 } else { 0 };
        report.check(
            &format!("toast-real-{label}-height-compact"),
            px_height <= 80,
            &format!("real MainWindow toast ({label}): {px_height} px tall (want <= 80)"),
        );
    }
    report.finish()
}

/// Gate: real Overview banner (Bug 3) — the real `OverviewPage`
/// in the disconnected + unreachable state, i.e. the exact
/// "Wheel not found" banner with its Reconnect button inside the
/// real `ScrollView`. Measures the banner's painted extent (first
/// err-bg row run) against the painted text extent: the last text
/// row must sit inside the banner with padding to spare, and no
/// text row may paint below the banner (that is the user's
/// "second line half cut off"). The replica probe
/// (`ui-banner-action-wrap`) isolates the component; this one
/// proves the shipped page.
#[allow(clippy::cast_precision_loss)]
fn ui_overview_banner_real() -> Outcome {
    let mut report = Report::new();
    let instance = match compile(
        "tools/gate-checks/rsh-gate-ui/ui/overview_probe.slint",
        "GateOverviewDisconnectedWindow",
    ) {
        Ok(instance) => instance,
        Err(error) => return Outcome::fail(error),
    };
    let (rgba, width, _height) = match snapshot(&instance) {
        Ok(snap) => snap,
        Err(error) => return Outcome::fail(error),
    };
    // Banner interior rows: rows with a wide run of err-bg
    // (#2E1F26). The banner is the first such run (top of page).
    let bg_rows = color_row_counts(&rgba, width, (0x2e, 0x1f, 0x26), 6, 0, width as usize);
    let mut banner_top: Option<usize> = None;
    let mut banner_bottom: Option<usize> = None;
    for (y, &count) in bg_rows.iter().enumerate() {
        if count > 100 {
            if banner_top.is_none() {
                banner_top = Some(y);
            }
            banner_bottom = Some(y);
        } else if banner_top.is_some() && banner_bottom.is_some() {
            break;
        }
    }
    let (Some(top), Some(bottom)) = (banner_top, banner_bottom) else {
        return Outcome::fail("no err-bg banner found on the Overview".to_string());
    };
    // Text rows from Theme.bad glyph pixels (rounded-corner border
    // pixels excluded by the >= 3 threshold, as in the replica
    // probe). Any text row below the banner bottom is overflow.
    let (_, bg_min_x, bg_max_x, _, _) = count_color(&rgba, width, (0x2e, 0x1f, 0x26), 6);
    let rows = color_row_counts(
        &rgba,
        width,
        (0xff, 0x6b, 0x6b),
        60,
        bg_min_x + 2,
        bg_max_x.saturating_sub(1),
    );
    let text_rows: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|&(_, &count)| (3..=200).contains(&count))
        .map(|(y, _)| y)
        .filter(|&y| y >= top.saturating_sub(2) && y <= bottom + 25)
        .collect();
    let last_text = text_rows.iter().max().copied();
    let first_text = text_rows.iter().min().copied();
    let overflow = text_rows.iter().any(|&y| y > bottom + 1);
    report.check(
        "overview-banner-text-fits",
        !overflow && last_text.is_some_and(|last| last + 6 <= bottom),
        &format!(
            "real Overview banner bg rows {top}..{bottom}, text rows {first_text:?}..{last_text:?} (want text inside, last + 6 <= bottom)"
        ),
    );
    report.finish()
}

/// Gate: popup surfaces (Bug 5) — the toast measured window-height
/// (580 px) because its Rectangle had no explicit height, and the
/// same defect was gate-measured on every other height-less
/// overlay (plain rect, `GameDialog`, `ProfilePopover`: all 580 px).
/// This check proves the fix: the confirm dialog's structure (as a
/// replica — the real one is inline in `MainWindow`) plus the real
/// `GameDialog` and the real `ProfilePopover`, all with their
/// height bound to their content layout.
/// Acceptance: every surface renders at content height, far below
/// the 580 px window height — a popup is a small card, never a
/// full-height panel.
fn ui_popup_surfaces() -> Outcome {
    let mut report = Report::new();
    let cases = [
        (
            "GatePlainRectWindow",
            "rect-height",
            "rect-width",
            100.0,
            "confirm-replica",
        ),
        (
            "GateGameDialogWindow",
            "dlg-height",
            "dlg-width",
            400.0,
            "game-dialog",
        ),
        (
            "GateProfilePopoverWindow",
            "pop-height",
            "pop-width",
            300.0,
            "profile-popover",
        ),
    ];
    for (component, height_prop, width_prop, max_height, label) in cases {
        let instance = match compile(
            "tools/gate-checks/rsh-gate-ui/ui/popup_probe.slint",
            component,
        ) {
            Ok(instance) => instance,
            Err(error) => return Outcome::fail(error),
        };
        if let Err(error) = snapshot(&instance) {
            return Outcome::fail(error);
        }
        let height = get_num(&instance, height_prop);
        let width = get_num(&instance, width_prop);
        report.check(
            &format!("popup-{label}-height-content"),
            height.is_some_and(|h| h <= max_height),
            &format!(
                "{label} height: {height:?} px, width: {width:?} px (want height <= {max_height}, far below the 580 px window)"
            ),
        );
    }
    report.finish()
}

/// One UI gate check: (id, description, runner).
type UiCheck = (&'static str, &'static str, fn() -> Outcome);

/// Content pixels of a rendered probe snapshot: pixels that are
/// not the window background (`#0d1118`).
fn steering_probe_content_pixels(rgba: &[u8]) -> usize {
    let (chunks, _rest) = rgba.as_chunks::<4>();
    chunks
        .iter()
        .filter(|chunk| chunk[0] != 0x0d || chunk[1] != 0x11 || chunk[2] != 0x18)
        .count()
}

/// The probe's Advanced callbacks exist with the right signatures
/// and fire: invoke each root callback and check the handler
/// receives the value.
fn steering_probe_callbacks(
    instance: &ComponentInstance,
    report: &mut Report,
) -> Result<(), String> {
    let last_bool = Rc::new(RefCell::new(false));
    let last_num = Rc::new(RefCell::new(f64::NAN));
    {
        let last_bool = Rc::clone(&last_bool);
        instance
            .set_callback("invert-changed", move |args| {
                if let Some(Value::Bool(value)) = args.first() {
                    *last_bool.borrow_mut() = *value;
                }
                Value::Void
            })
            .map_err(|error| format!("set invert-changed handler failed: {error}"))?;
    }
    instance
        .invoke("invert-changed", &[Value::Bool(true)])
        .map_err(|error| format!("invoke invert-changed failed: {error}"))?;
    report.check(
        "steering-invert-callback",
        *last_bool.borrow(),
        "invoke invert-changed(true) must reach the handler",
    );
    for (name, value) in [
        ("lock-changed", 0.5_f64),
        ("smoothing-changed", 0.3_f64),
        ("balance-changed", 0.75_f64),
        ("trim-changed", 0.2_f64),
    ] {
        let last_num = Rc::clone(&last_num);
        let last_num_cb = Rc::clone(&last_num);
        instance
            .set_callback(name, move |args| {
                if let Some(Value::Number(n)) = args.first() {
                    *last_num_cb.borrow_mut() = *n;
                }
                Value::Void
            })
            .map_err(|error| format!("set {name} handler failed: {error}"))?;
        instance
            .invoke(name, &[Value::Number(value)])
            .map_err(|error| format!("invoke {name} failed: {error}"))?;
        report.check(
            &format!("steering-{name}-callback"),
            (*last_num.borrow() - value).abs() <= f64::EPSILON,
            &format!("invoke {name}({value}) must reach the handler"),
        );
    }
    // S1: the full-lock capture buttons fire their callbacks.
    for name in ["capture-full-left", "capture-full-right"] {
        let fired = Rc::new(RefCell::new(false));
        let fired_cb = Rc::clone(&fired);
        instance
            .set_callback(name, move |_| {
                *fired_cb.borrow_mut() = true;
                Value::Void
            })
            .map_err(|error| format!("set {name} handler failed: {error}"))?;
        instance
            .invoke(name, &[])
            .map_err(|error| format!("invoke {name} failed: {error}"))?;
        report.check(
            &format!("steering-{name}-callback"),
            *fired.borrow(),
            &format!("invoke {name}() must reach the handler"),
        );
    }
    Ok(())
}

/// Gate: steering Advanced (Bug 7) — the Advanced card holds real
/// tuning controls (invert toggle, lock slider, smoothing slider),
/// not the old read-only rows (centre mV, raw live, curve exponent
/// box, min/max note). Proves: the real `SteeringPage` compiles,
/// Advanced-on renders a whole extra card of content, the new
/// labels are in the source and the old rows are gone, and the
/// Advanced callbacks exist with the right signatures.
/// (S1–S3, per-module tuning: the full-lock capture buttons and the
/// balance / centre-trim sliders are covered the same way.)
fn ui_steering_advanced() -> Outcome {
    let mut report = Report::new();
    // Source assertions: new controls present, old rows gone.
    let src_path = repo_root().join("crates/rsh-ui/ui/screens/steering.slint");
    let src = match std::fs::read_to_string(&src_path) {
        Ok(src) => src,
        Err(error) => return Outcome::fail(format!("read steering.slint failed: {error}")),
    };
    for label in [
        "Invert steering",
        "Steering lock",
        "Smoothing",
        "Left/right balance",
        "Centre trim",
        "Full left",
        "Full right",
    ] {
        report.check(
            &format!("steering-advanced-has-{label}"),
            src.contains(label),
            &format!("steering.slint must contain the {label} control"),
        );
    }
    for gone in [
        "Raw live",
        "Curve exponent",
        "Min/max mV aren't stored",
        "centre-mv-text",
        "raw-mv-text",
    ] {
        report.check(
            &format!("steering-advanced-no-{gone}"),
            !src.contains(gone),
            &format!("steering.slint must not contain the old {gone} row"),
        );
    }
    let instance = match compile(
        "tools/gate-checks/rsh-gate-ui/ui/steering_probe.slint",
        "GateSteeringWindow",
    ) {
        Ok(instance) => instance,
        Err(error) => return Outcome::fail(error),
    };
    // Rendered content: Advanced-on must visibly change the render
    // — the Advanced card (labels + two sliders + switch) paints
    // new pixels. A raw content-pixel total misleads here (the
    // ScrollView's scrollbar appears once the card is added), so
    // the check counts pixels that DIFFER between the two renders.
    if let Err(error) = set_bool(&instance, "advanced", false) {
        return Outcome::fail(error);
    }
    let (basic_rgba, _, _) = match snapshot(&instance) {
        Ok(snap) => snap,
        Err(error) => return Outcome::fail(error),
    };
    let basic = steering_probe_content_pixels(&basic_rgba);
    if let Err(error) = set_bool(&instance, "advanced", true) {
        return Outcome::fail(error);
    }
    let (advanced_rgba, _, _) = match snapshot(&instance) {
        Ok(snap) => snap,
        Err(error) => return Outcome::fail(error),
    };
    let advanced = steering_probe_content_pixels(&advanced_rgba);
    let diff = basic_rgba
        .iter()
        .zip(advanced_rgba.iter())
        .filter(|(a, b)| a != b)
        .count();
    report.check(
        "steering-advanced-adds-content",
        diff > 5000,
        &format!(
            "render diff Basic→Advanced: {diff} px changed (content {basic} → {advanced}, want > 5000 changed)"
        ),
    );
    if let Err(error) = steering_probe_callbacks(&instance, &mut report) {
        return Outcome::fail(error);
    }
    report.finish()
}

fn ui_brake_tuning() -> Outcome {
    let mut report = Report::new();
    // Source assertions: the brake card carries both new controls
    // (B1 brake force + B2 bite point), wired to their own
    // callbacks — never to the clutch bite handler.
    let src_path = repo_root().join("crates/rsh-ui/ui/screens/pedals.slint");
    let src = match std::fs::read_to_string(&src_path) {
        Ok(src) => src,
        Err(error) => return Outcome::fail(format!("read pedals.slint failed: {error}")),
    };
    for label in ["Brake force", "Travel for full brake"] {
        report.check(
            &format!("brake-tuning-has-{label}"),
            src.contains(label),
            &format!("pedals.slint must contain the {label} control"),
        );
    }
    for wiring in [
        "bite: brake-bite;",
        "force: brake-force;",
        "bite-changed(v) => { brake-bite-changed(v); }",
        "force-changed(v) => { brake-force-changed(v); }",
    ] {
        report.check(
            &format!("brake-tuning-wires-{wiring}"),
            src.contains(wiring),
            &format!("pedals.slint must wire the brake card: {wiring}"),
        );
    }
    report.check(
        "brake-tuning-force-only-on-brake",
        src.matches("show-force: true").count() == 1,
        "only the brake card shows the Brake force row (throttle/clutch must not)",
    );
    let instance = match compile(
        "tools/gate-checks/rsh-gate-ui/ui/brake_probe.slint",
        "GateBrakeWindow",
    ) {
        Ok(instance) => instance,
        Err(error) => return Outcome::fail(error),
    };
    // Rendered content: moving the Brake force thumb (1.0 → 0.5)
    // and the Bite thumb (0.0 → 1.0 of 50%) must repaint the
    // slider tracks — the rows exist and the props drive them.
    // (Per-byte diff, like the steering check: a slider row is
    // small, so the bar is 500 changed bytes, not 5000.)
    let diff_for =
        |instance: &ComponentInstance, prop: &str, from: f64, to: f64| -> Result<usize, String> {
            set_num(instance, prop, from)?;
            let (before, _, _) = snapshot(instance)?;
            set_num(instance, prop, to)?;
            let (after, _, _) = snapshot(instance)?;
            Ok(before
                .iter()
                .zip(after.iter())
                .filter(|(a, b)| a != b)
                .count())
        };
    match diff_for(&instance, "brake-force", 1.0, 0.5) {
        Ok(diff) => report.check(
            "brake-force-slider-moves",
            diff > 500,
            &format!("Brake force thumb 100%→50%: {diff} bytes changed (want > 500)"),
        ),
        Err(error) => return Outcome::fail(error),
    }
    match diff_for(&instance, "brake-bite", 0.0, 1.0) {
        Ok(diff) => report.check(
            "brake-bite-slider-moves",
            diff > 500,
            &format!("Brake bite thumb 0%→50%: {diff} bytes changed (want > 500)"),
        ),
        Err(error) => return Outcome::fail(error),
    }
    // Callbacks: the probe's brake callbacks exist with the right
    // signatures and fire (same pattern as the steering check).
    for (name, value) in [
        ("brake-force-changed", 0.75_f64),
        ("brake-bite-changed", 0.5_f64),
    ] {
        let last_num = Rc::new(RefCell::new(f64::NAN));
        let last_num_cb = Rc::clone(&last_num);
        if let Err(error) = instance.set_callback(name, move |args| {
            if let Some(Value::Number(n)) = args.first() {
                *last_num_cb.borrow_mut() = *n;
            }
            Value::Void
        }) {
            return Outcome::fail(format!("set {name} handler failed: {error}"));
        }
        if let Err(error) = instance.invoke(name, &[Value::Number(value)]) {
            return Outcome::fail(format!("invoke {name} failed: {error}"));
        }
        report.check(
            &format!("brake-{name}-callback"),
            (*last_num.borrow() - value).abs() < f64::EPSILON,
            &format!("invoke {name}({value}) must reach the handler"),
        );
    }
    report.finish()
}

fn checks() -> Vec<UiCheck> {
    vec![
        (
            "ui-slider-visual",
            "A6: slider fill runs min → thumb at min/mid/max (pixel proof)",
            ui_slider_visual as fn() -> Outcome,
        ),
        (
            "ui-slider-click",
            "A6/A2: clicking the track at 25%/75% fires changed(0.25/0.75)",
            ui_slider_click as fn() -> Outcome,
        ),
        (
            "ui-wizard-gating",
            "A2: wizard next-enabled per step + Next click-through 0→3 + finish; Skip invokes",
            ui_wizard_gating as fn() -> Outcome,
        ),
        (
            "ui-wizard-no-fake-data",
            "A3: wizard defaults are placeholders, never fake live values",
            ui_wizard_no_fake_data as fn() -> Outcome,
        ),
        (
            "ui-theme-switch",
            "A8: theme Dark/Light/Auto repaints the window",
            ui_theme_switch as fn() -> Outcome,
        ),
        (
            "ui-tray-disconnected",
            "A12: tray popup defaults Disconnected + red dot; live values flip it green",
            ui_tray_disconnected as fn() -> Outcome,
        ),
        (
            "ui-tray-light-theme",
            "DS-22: tray popup light-theme repaints the background white (Dark stays #11161F)",
            ui_tray_light_theme as fn() -> Outcome,
        ),
        (
            "ui-banner-wrap",
            "DS-21: EmptyBanner wraps long text and grows past 36 px (short text stays 36 px)",
            ui_banner_wrap as fn() -> Outcome,
        ),
        (
            "ui-toast-size",
            "Bug 2: toast stays a 480 px wide, <= 80 px tall card on short, flap and long bodies",
            ui_toast_size as fn() -> Outcome,
        ),
        (
            "ui-toast-real-mainwindow",
            "Bug 2: the real MainWindow toast (FocusScope-parented) renders <= 80 px tall, short and long bodies",
            ui_toast_real_mainwindow as fn() -> Outcome,
        ),
        (
            "ui-banner-action-wrap",
            "Bug 3: Overview banner with Reconnect button grows for two-line text and the second line fits inside it",
            ui_banner_action_wrap as fn() -> Outcome,
        ),
        (
            "ui-overview-banner-real",
            "Bug 3: the real OverviewPage in the wheel-not-found state renders its banner text inside the banner, no overflow below it",
            ui_overview_banner_real as fn() -> Outcome,
        ),
        (
            "ui-popup-surfaces",
            "Bug 5: every height-less popup surface (plain rect, GameDialog, ProfilePopover) renders at content height, not window height",
            ui_popup_surfaces as fn() -> Outcome,
        ),
        (
            "ui-extras-card-size",
            "Cards fix Bug 2: Extras card heights identical ON/OFF (all-ON, one-OFF, all-OFF)",
            ui_extras_card_size as fn() -> Outcome,
        ),
        (
            "ui-extras-off-no-input",
            "Cards fix Bug 2 safety: OFF body takes no input, header Switch still toggles",
            ui_extras_off_no_input as fn() -> Outcome,
        ),
        (
            "ui-card-gap-responsive",
            "Cards fix Bug 1: gap 12 px at design width, grows on wider windows; cards grow",
            ui_card_gap_responsive as fn() -> Outcome,
        ),
        (
            "ui-games-grid-gap",
            "R3-UI-06: games-grid tile gap 10 px at design width, grows with the window",
            ui_games_grid_gap as fn() -> Outcome,
        ),
        (
            "ui-steering-advanced",
            "Bug 7 + S1–S3: Advanced card holds invert + lock + smoothing + balance + trim, centre card holds the full-lock capture buttons (not the old read-only rows)",
            ui_steering_advanced as fn() -> Outcome,
        ),
        (
            "ui-brake-tuning",
            "Brake B1+B2: brake card holds Brake force + Bite point sliders (own callbacks, force only on brake)",
            ui_brake_tuning as fn() -> Outcome,
        ),
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut only: Option<String> = None;
    let mut list = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--only" => {
                i += 1;
                only = args.get(i).cloned();
            }
            "--list" => list = true,
            "--help" | "-h" => {
                eprintln!("Usage: rsh-gate-ui [--only <id>] [--list]");
                return;
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let all = checks();
    if list {
        for (id, desc, _) in &all {
            println!("{id} — {desc}");
        }
        return;
    }
    init_platform();
    let selected: Vec<_> = all
        .into_iter()
        .filter(|(id, _, _)| only.as_deref().is_none_or(|want| *id == want))
        .collect();
    if selected.is_empty() {
        eprintln!("no check matches --only {}", only.unwrap_or_default());
        std::process::exit(2);
    }
    let mut passed = 0usize;
    for (id, desc, run) in &selected {
        println!("--- {id} ({desc})");
        let outcome = run();
        println!("    {}", outcome.detail);
        println!("    {} ", if outcome.passed { "PASS" } else { "FAIL" });
        if outcome.passed {
            passed += 1;
        }
    }
    let total = selected.len();
    println!("== rsh-gate-ui: {passed}/{total} PASS");
    if passed != total {
        std::process::exit(1);
    }
}
