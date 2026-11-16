//! Response-curve math for the Steering page (preview `Wheel.html`).
//!
//! Ported from the Phase 1 spike (`v2-spike/src/main.rs`): the viewBox is
//! 330x232, plot x 30..290, y 20..190. Slint `Path` has no dash support,
//! so dashed lines are generated as path segments here.

use std::fmt::Write as _;

/// View-box geometry of the steering curve.
pub const VX0: f32 = 30.0;
/// View-box geometry of the steering curve.
pub const VX1: f32 = 290.0;
/// View-box geometry of the steering curve.
pub const VY0: f32 = 20.0;
/// View-box geometry of the steering curve.
pub const VY1: f32 = 190.0;

/// Power-curve output (0..100) for a wheel input (0..100) with a flat
/// deadzone head and the given exponent.
#[must_use]
pub fn curve_output(input: f32, deadzone_pct: f32, exponent: f32) -> f32 {
    if input <= deadzone_pct {
        0.0
    } else {
        let x = (input - deadzone_pct) / (100.0 - deadzone_pct);
        x.powf(exponent) * 100.0
    }
}

/// Polyline path commands for the response curve, in viewBox coords.
#[must_use]
pub fn curve_commands(deadzone_pct: f32, exponent: f32) -> String {
    let mut s = String::from("M 30 190");
    let flat_x = VX0 + deadzone_pct / 100.0 * (VX1 - VX0);
    let _ = write!(s, " L {flat_x:.1} 190");
    let mut x = deadzone_pct;
    while x < 100.0 {
        x = (x + 4.0).min(100.0);
        let out = curve_output(x, deadzone_pct, exponent);
        let vx = VX0 + x / 100.0 * (VX1 - VX0);
        let vy = VY1 - out / 100.0 * (VY1 - VY0);
        let _ = write!(s, " L {vx:.1} {vy:.1}");
    }
    s
}

/// Dashed straight line (linear response) from (30,190) to (290,20),
/// dash 4 / gap 4 in viewBox units — the preview's "Straight line".
#[must_use]
pub fn dashed_line_commands() -> String {
    dashed_segment(30.0, 190.0, 290.0, 20.0, 4.0, 4.0)
}

/// Dashed crosshair from the live marker (mx,my): vertical down to y=190
/// and horizontal left to x=30, dash 3 / gap 3.
#[must_use]
pub fn crosshair_commands(mx: f32, my: f32) -> String {
    let mut s = dashed_segment(mx, my, mx, 190.0, 3.0, 3.0);
    s.push(' ');
    s.push_str(&dashed_segment(30.0, my, mx, my, 3.0, 3.0));
    s
}

/// One dashed straight segment as `M x y L x y ...` commands.
#[must_use]
pub fn dashed_segment(x1: f32, y1: f32, x2: f32, y2: f32, dash: f32, gap: f32) -> String {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.001 {
        return String::new();
    }
    let ux = dx / len;
    let uy = dy / len;
    let mut s = String::new();
    let mut d = 0.0;
    while d < len {
        let a = d;
        let b = (d + dash).min(len);
        let _ = write!(
            s,
            "M {:.1} {:.1} L {:.1} {:.1} ",
            x1 + ux * a,
            y1 + uy * a,
            x1 + ux * b,
            y1 + uy * b
        );
        d += dash + gap;
    }
    s.trim_end().to_string()
}
