//! The dock rail's berth shape, a point-for-point port of
//! `DockBerthShape`.
//!
//! The rail looks like it grew out of the screen's edge: the edge-facing
//! side runs flush for the whole height, and each end sweeps out to meet
//! the screen through a **concave** fillet, tangent-continuous at both
//! junctions. Off the edge it closes into a capsule with fully round ends.
//! The collapsed sliver is this same shape at `openness` 0, which is what
//! lets the two be animated between as one object changing size.
//!
//! Drawn once, facing right, then moved into place: mirrored for the left
//! edge, quarter-turned for the top. The rotation preserves winding, which
//! a reflection would not.

use windows::core::Result;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct2D::Common::D2D1_FILL_MODE_WINDING;

use crate::d2d::{point, D2DEngine, PathBuilder};
use crate::geometry::{dock, Edge, Metrics};

/// Superellipse exponent: 2 would be a circle; 4 lands close to the
/// squircle Apple and Fluent use, keeping the corner full while easing it
/// into the straight edges.
const SQUIRCLE_EXPONENT: f64 = 4.0;
/// Enough segments that the sampled curve stays sub-pixel smooth.
const CORNER_SAMPLES: usize = 48;

/// Where a path's segments go: the real D2D builder, or the recorder that
/// captures the canonical shape for transformation afterwards.
trait PathSink {
    fn begin_at(&mut self, x: f64, y: f64);
    fn line(&mut self, x: f64, y: f64);
    fn curve(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64);
    fn end_closed(&mut self);
}

impl PathSink for PathBuilder {
    fn begin_at(&mut self, x: f64, y: f64) {
        PathBuilder::begin_at(self, x, y);
    }

    fn line(&mut self, x: f64, y: f64) {
        PathBuilder::line(self, x, y);
    }

    fn curve(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
        PathBuilder::curve(self, c1x, c1y, c2x, c2y, x, y);
    }

    fn end_closed(&mut self) {
        PathBuilder::end_closed(self);
    }
}

enum Op {
    Move(f64, f64),
    Line(f64, f64),
    Curve(f64, f64, f64, f64, f64, f64),
    Close,
}

struct Recorder {
    ops: Vec<Op>,
}

impl PathSink for Recorder {
    fn begin_at(&mut self, x: f64, y: f64) {
        self.ops.push(Op::Move(x, y));
    }

    fn line(&mut self, x: f64, y: f64) {
        self.ops.push(Op::Line(x, y));
    }

    fn curve(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
        self.ops.push(Op::Curve(c1x, c1y, c2x, c2y, x, y));
    }

    fn end_closed(&mut self) {
        self.ops.push(Op::Close);
    }
}

/// A quarter superellipse sampled as a polyline, from unit direction
/// `from` to unit direction `to` around `center`.
fn append_corner(
    builder: &mut dyn PathSink,
    center: (f64, f64),
    radius: f64,
    from: (f64, f64),
    to: (f64, f64),
) {
    if radius <= 0.0 {
        return;
    }
    for step in 1..=CORNER_SAMPLES {
        let t = step as f64 / CORNER_SAMPLES as f64 * (std::f64::consts::PI / 2.0);
        let along = t.cos().powf(2.0 / SQUIRCLE_EXPONENT);
        let across = t.sin().powf(2.0 / SQUIRCLE_EXPONENT);
        builder.line(
            center.0 + radius * (from.0 * along + to.0 * across),
            center.1 + radius * (from.1 * along + to.1 * across),
        );
    }
}

/// The canonical shape, facing right, in a rect of (width, height) design
/// units.
fn facing_right(builder: &mut dyn PathSink, w: f64, h: f64, m: &Metrics, openness: f64) {
    let flare_height = dock::FLARE_HEIGHT * m.scale * openness;
    let flare_width = dock::FLARE_WIDTH * m.scale * openness;
    let corner_radius = (dock::COLLAPSED_WIDTH
        + (dock::CORNER_RADIUS - dock::COLLAPSED_WIDTH) * openness)
        * m.scale;

    let f = flare_height.min(h / 2.0);
    // The convex corners live on the body, which spans y in [f, h - f].
    let r = corner_radius.min(w).min((h - f * 2.0) / 2.0).max(0.0);
    // The flare must leave room for that corner: if flare_width + r
    // exceeded the width, the body's flat top edge would run backwards and
    // the path would fold in on itself.
    let fw = flare_width.min(w - r).max(0.0);

    // Pulls each fillet's control points off its endpoints. 0.55 is the
    // usual circular-arc approximation; it keeps the sweep full instead of
    // flattening it into a sliver.
    let k: f64 = 0.55;

    builder.begin_at(r, f);

    // Body's flat top edge, left to right.
    builder.line(w - fw, f);

    // Concave fillet sweeping up into the screen edge.
    builder.curve(w - fw * (1.0 - k), f, w, f * k, w, 0.0);

    // Flush against the screen for the full height.
    builder.line(w, h);

    // Mirrored fillet back down into the body's bottom edge.
    builder.curve(w, h - f * k, w - fw * (1.0 - k), h - f, w - fw, h - f);

    // Body's flat bottom edge, right to left.
    builder.line(r, h - f);

    // Bottom-left corner: starts directly below the corner's center, ends
    // directly to its left.
    append_corner(builder, (r, h - f - r), r, (0.0, 1.0), (-1.0, 0.0));

    // Left edge.
    builder.line(0.0, f + r);

    // Top-left corner: starts to the left of its center, ends above it.
    append_corner(builder, (r, f + r), r, (-1.0, 0.0), (0.0, -1.0));

    builder.end_closed();
}

/// A true capsule: the ends are half circles, not rounded-off corners.
/// Deliberately circular rather than squircle — at this width the two
/// corners of an end meet with no straight edge between them, so there is
/// nothing to ease into.
fn floating(builder: &mut PathBuilder, w: f64, h: f64) {
    let radius = w.min(h) / 2.0;
    builder.begin_at(radius, 0.0);
    builder.line(w - radius, 0.0);
    builder.arc_to(point(w as f32, radius as f32), radius as f32, false, true);
    builder.line(w, h - radius);
    builder.arc_to(
        point((w - radius) as f32, h as f32),
        radius as f32,
        false,
        true,
    );
    builder.line(radius, h);
    builder.arc_to(point(0.0, (h - radius) as f32), radius as f32, false, true);
    builder.line(0.0, radius);
    builder.arc_to(point(radius as f32, 0.0), radius as f32, false, true);
    builder.end_closed();
}

/// Builds the berth in **pixel** coordinates for the rail rect at
/// `(x, y, w, h)`, facing `edge`, at the given openness (0 = sliver).
pub fn berth_path(
    engine: &D2DEngine,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    edge: Edge,
    docked: bool,
    openness: f64,
    m: &Metrics,
) -> Result<ID2D1PathGeometry> {
    if !docked {
        let mut builder = PathBuilder::new(engine, D2D1_FILL_MODE_WINDING)?;
        floating(&mut builder, w as f64, h as f64);
        return builder.finish();
    }

    // The canonical rect is the rail laid on its side for the top edge, so
    // the drawing is unchanged and only its placement differs.
    let (cw, ch) = match edge {
        Edge::Top => (h as f64, w as f64),
        _ => (w as f64, h as f64),
    };

    // Record the canonical shape, then emit it transformed. The shape is
    // built from lines, béziers and superellipse samples; transforming
    // their points directly is exact and preserves winding.
    let mut recorder = Recorder { ops: Vec::new() };
    facing_right(&mut recorder, cw, ch, m, openness);

    let transform = |px: f64, py: f64| -> (f32, f32) {
        let (tx, ty) = match edge {
            Edge::Right => (px, py),
            Edge::Left => (cw - px, py),
            Edge::Top => (py, ch - px),
        };
        ((x as f64 + tx) as f32, (y as f64 + ty) as f32)
    };

    let mut builder = PathBuilder::new(engine, D2D1_FILL_MODE_WINDING)?;
    for op in &recorder.ops {
        match op {
            Op::Move(px, py) => {
                let (tx, ty) = transform(*px, *py);
                builder.begin_at(tx as f64, ty as f64);
            }
            Op::Line(px, py) => {
                let (tx, ty) = transform(*px, *py);
                builder.line(tx as f64, ty as f64);
            }
            Op::Curve(c1x, c1y, c2x, c2y, ex, ey) => {
                let (a, b) = transform(*c1x, *c1y);
                let (c, d) = transform(*c2x, *c2y);
                let (e, f) = transform(*ex, *ey);
                builder.curve(
                    a as f64, b as f64, c as f64, d as f64, e as f64, f as f64,
                );
            }
            Op::Close => builder.end_closed(),
        }
    }
    builder.finish()
}

/// Where the rail sits inside the panel window, in design units, for the
/// given state. The single source of truth the drawing and the hit testing
/// both step through — the same rule `PanelPlacement.layout` keeps on the
/// macOS side.
pub fn rail_frame(
    m: &Metrics,
    item_count: usize,
    edge: Edge,
    docked: bool,
    window: (f64, f64),
) -> (f64, f64, f64, f64) {
    let (rail_w, rail_h) = dock::size(m, item_count, edge, docked);
    let (win_w, win_h) = window;
    match edge {
        Edge::Right => (win_w - rail_w, (win_h - rail_h) / 2.0, rail_w, rail_h),
        Edge::Left => (0.0, (win_h - rail_h) / 2.0, rail_w, rail_h),
        Edge::Top => ((win_w - rail_w) / 2.0, 0.0, rail_w, rail_h),
    }
}

/// The collapsed sliver's rect: `COLLAPSED_WIDTH` against the screen edge,
/// `COLLAPSED_HEIGHT` along it, centred the way the berth is.
pub fn sliver_rect(m: &Metrics, edge: Edge, window: (f64, f64)) -> (f32, f32, f32, f32) {
    let (win_w, win_h) = window;
    let cw = m.s(dock::COLLAPSED_WIDTH);
    let chh = m.s(dock::COLLAPSED_HEIGHT);
    match edge {
        Edge::Right => ((win_w - cw) as f32, ((win_h - chh) / 2.0) as f32, cw as f32, chh as f32),
        Edge::Left => (0.0, ((win_h - chh) / 2.0) as f32, cw as f32, chh as f32),
        Edge::Top => (((win_w - chh) / 2.0) as f32, 0.0, chh as f32, cw as f32),
    }
}
