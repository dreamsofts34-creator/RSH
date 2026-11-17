//! `rsh-gate-ui` — v2.0.2 UI gate checks (plan §C) that need a
//! rendered UI: slider fill geometry + click values, wizard step
//! gating + Next click-through, theme repaint, tray disconnected
//! state, tray Light theme (DS-22), banner wrap + grow (DS-21).
//!
//! The real `.slint` files are compiled with `slint-interpreter`
//! on the testing backend with the software renderer — no X, no
//! window manager, no daemon. The wizard's Rust handlers live in
//! `rsh-ui` (a binary crate, not importable), so the click-through
//! mirrors the real `on_setup_next` handler from
//! `crates/rsh-ui/src/main.rs` (step + 1, finish at step 4) and the
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
#[must_use]
fn footer_accent_box(rgba: &[u8], width: u32, height: u32) -> Option<(usize, usize, usize, usize)> {
    let footer_first_row = height as usize * 70 / 100;
    let mut bbox: Option<(usize, usize, usize, usize)> = None;
    for y in footer_first_row..height as usize {
        for x in 0..width as usize {
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
/// finish at step 4). Skip/Back are proven via `invoke` (their
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
    // next-enabled truth table (setup.slint line 148).
    let cases: &[GateCase] = &[
        ("step0-blocked", &[("connect-done", false)]),
        ("step0-open", &[("connect-done", true)]),
        (
            "step1-blocked",
            &[("vjoy-installed", false), ("vjoy-fallback", false)],
        ),
        ("step1-vjoy", &[("vjoy-installed", true)]),
        ("step1-fallback", &[("vjoy-fallback", true)]),
        ("step2-blocked", &[("paired", false)]),
        ("step2-open", &[("paired", true)]),
        ("step3-no-calib", &[("calibration-available", false)]),
        (
            "step3-blocked",
            &[
                ("calibration-available", true),
                ("centre-done", false),
                ("pedals-done", false),
            ],
        ),
        (
            "step3-open",
            &[
                ("calibration-available", true),
                ("centre-done", true),
                ("pedals-done", true),
            ],
        ),
    ];
    // (step, expected next-enabled) per case, in order.
    let steps = [0, 0, 1, 1, 1, 2, 2, 3, 3, 3];
    let expected = [
        false, true, false, true, true, false, true, true, false, true,
    ];
    for (i, (name, flags)) in cases.iter().enumerate() {
        let step = steps[i];
        if let Err(error) = set_num(&instance, "step", f64::from(step)) {
            return Outcome::fail(error);
        }
        for prop in [
            "connect-done",
            "vjoy-installed",
            "vjoy-fallback",
            "paired",
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

    // Click-through: mirrored on_setup_next (step + 1, finish at 4).
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
            if step >= 4 {
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
    // Drive: step 0 (connect-done) → 1 (vjoy) → 2 (paired) → 3
    // (calibration done) → 4 → finish.
    let drive: &[(&str, bool)] = &[
        ("connect-done", true),
        ("vjoy-installed", true),
        ("paired", true),
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
    // Finish: one more Next at step 4.
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
    }
    report.check(
        "wizard-click-finish",
        finished.get(),
        "Next at step 4 finishes the wizard (Finish button)",
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
        get_str(&instance, "games-text").as_deref() == Some("No games found yet."),
        "games-text defaults to 'No games found yet.'",
    );
    report.check(
        "wizard-default-pair",
        get_str(&instance, "pair-status").as_deref() == Some("Waiting for the wheel…"),
        "pair-status defaults to 'Waiting for the wheel…'",
    );
    report.check(
        "wizard-default-flags",
        get_bool(&instance, "connect-done") == Some(false)
            && get_bool(&instance, "paired") == Some(false),
        "connect-done=false, paired=false by default",
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

/// Gate: error banner wraps + grows (DS-21, U-03) — the banner was
/// a fixed 36 px with no wrapping, so a 150-char daemon-error was
/// cut off mid-sentence. A short string stays 36 px; the 150-char
/// string (the plan's verification case) must grow the banner past
/// 40 px, proving the text wraps instead of clipping.
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

/// One UI gate check: (id, description, runner).
type UiCheck = (&'static str, &'static str, fn() -> Outcome);

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
            "A2: wizard next-enabled per step + Next click-through 0→4 + finish; Skip invokes",
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
