//! Ring painting, ported from `UsageRingView`.
//!
//! The vocabulary: a track at 18% white; a coloured usage arc from 12
//! o'clock; a white travelling mark inside when the CLI is working; a
//! coloured travelling mark on the ring itself while a reading is being
//! fetched; an optional hairline clock arc outside; an optional thinner
//! second ring inside; the provider's mark in the middle. Colour means
//! **usage**, never brand.

use windows_numerics::Vector2;

use crate::d2d::{arc_geometry, arc_sweep_geometry, circle_geometry, point, Painter, Rgba};
use crate::theme::panel;

/// A travelling mark's share of its circle — short enough to read as a mark
/// rather than a second progress ring.
const BUSY_SWEEP_DEG: f64 = 0.22 * 360.0;
const BUSY_PERIOD_MS: f64 = 1000.0;
const REFRESH_SWEEP_DEG: f64 = 0.16 * 360.0;
const REFRESH_PERIOD_MS: f64 = 850.0;

/// The halo's reach, in design units.
const HALO_RADIUS: f64 = 10.0;

/// The gap between the progress ring and the disc it encircles, and the
/// mark's share of that dark disc.
const CENTRE_GAP: f64 = 4.0;
const ICON_SCALE: f64 = 0.8;
/// How much the disc shrinks per side when the second ring takes the band
/// the activity mark was riding.
const SECOND_RING_SQUEEZE: f64 = 2.0;
const CLOCK_GAP: f64 = 3.0;
const CLOCK_LINE_WIDTH: f64 = 2.0;

#[derive(Clone)]
pub struct RingModel {
    /// How much of the tightest limit is gone, or none when there is no
    /// fraction to draw — an empty track then says "nothing known" rather
    /// than "nothing used".
    pub used_fraction: Option<f64>,
    /// Whether this ring has a reading at all. Not the same question: a
    /// balance-only account has money and no fraction.
    pub has_reading: bool,
    /// A colour chosen for this account, or none to colour by usage. Spent
    /// still uses the spent colour — being blocked is not a matter of taste.
    pub tint: Option<Rgba>,
    pub is_spent: bool,
    /// Draw the arc as what is **left** rather than what is gone. Only the
    /// arc; the colour is worked out from `used_fraction` either way.
    pub shows_remaining: bool,
    /// The provider's monogram, drawn in the disc.
    pub monogram: String,
    pub is_busy: bool,
    pub is_refreshing: bool,
    pub highlight: bool,
    /// How much of the window has gone by, for the hairline clock arc.
    pub elapsed_fraction: Option<f64>,
    /// The next-fullest limit, drawn as a smaller ring inside this one.
    pub second_fraction: Option<f64>,
    pub second_is_spent: bool,
}

impl RingModel {
    pub fn unavailable(monogram: &str) -> RingModel {
        RingModel {
            used_fraction: None,
            has_reading: false,
            tint: None,
            is_spent: false,
            shows_remaining: false,
            monogram: monogram.to_string(),
            is_busy: false,
            is_refreshing: false,
            highlight: false,
            elapsed_fraction: None,
            second_fraction: None,
            second_is_spent: false,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_ring(
    painter: &Painter,
    center: Vector2,
    diameter: f64,
    line_width: f64,
    scale: f64,
    model: &RingModel,
    now_ms: f64,
) -> windows::core::Result<()> {
    let radius = (diameter / 2.0) as f32;
    let lw = line_width as f32;

    // The disc the mark sits in. It gives up two points a side to the
    // second ring when that ring is drawn.
    let squeeze = if model.second_fraction.is_some() {
        SECOND_RING_SQUEEZE * scale
    } else {
        0.0
    };
    let centre_diameter = (diameter - (line_width + CENTRE_GAP * scale) * 2.0 - squeeze * 2.0).max(0.0);

    // The busy mark rides the empty ring between the disc and the usage
    // ring — or just outside the disc when the second ring is there.
    let busy_radius = if model.second_fraction.is_some() {
        ((centre_diameter + SECOND_RING_SQUEEZE * scale) / 2.0) as f32
    } else {
        ((diameter - line_width * 1.5 - CENTRE_GAP * scale) / 2.0) as f32
    };

    // The clock arc's own circle, measured out from the usage ring's outer
    // edge.
    let clock_radius =
        ((diameter + line_width + (CLOCK_GAP + CLOCK_LINE_WIDTH / 2.0) * 2.0 * scale) / 2.0) as f32;

    let arc_color = arc_colour(model);

    // The pointed-at halo: a soft disc beneath everything, in the arc's
    // colour. On the macOS side this is a shadow on the arc, masked inward;
    // here a gradient disc beneath the ring reads the same and has no
    // colour to get wrong.
    if model.highlight {
        painter.draw_halo(center, radius + HALO_RADIUS as f32 * scale as f32, arc_color)?;
    }

    // Track.
    let track = circle_geometry(painter.engine, center, radius)?;
    let brush = painter.brush(panel::TRACK)?;
    painter.draw_geometry(&track, &brush, lw, true);

    // Usage arc.
    let fraction = arc_fraction(model);
    if let Some(arc) = arc_geometry(painter.engine, center, radius, fraction)? {
        let mut c = arc_color;
        if model.is_refreshing {
            c = quiet(c);
        }
        let brush = painter.brush(c)?;
        painter.draw_geometry(&arc, &brush, lw, true);
    }

    // The refresh mark: a short segment of the ring, in the usage colour,
    // travelling the whole circle. It runs over the track and the usage arc
    // alike, which is the point — spinning the arc itself would make the
    // feedback depend on the number it is showing.
    if model.is_refreshing {
        let angle = (now_ms % REFRESH_PERIOD_MS) / REFRESH_PERIOD_MS * 360.0;
        if let Some(mark) =
            arc_sweep_geometry(painter.engine, center, radius, angle - 90.0, REFRESH_SWEEP_DEG)?
        {
            let brush = painter.brush(arc_color)?;
            painter.draw_geometry(&mark, &brush, lw, true);
        }
    }

    // The provider's monogram in the disc. Dimmed while there is no
    // reading, so the rail shows at a glance which providers it has data
    // for.
    let disc_brush = painter.brush(panel::SURFACE)?;
    painter.fill_ellipse(center, centre_diameter as f32 / 2.0, &disc_brush);
    let text_alpha = if model.has_reading { 1.0 } else { 0.35 };
    let text_brush = painter.brush(panel::TEXT_PRIMARY.with_alpha(text_alpha as f32))?;
    let font_size = (centre_diameter * ICON_SCALE * 0.62) as f32;
    let half = centre_diameter as f32 / 2.0;
    painter.text(
        &model.monogram,
        crate::d2d::rect(center.X - half, center.Y - half, half * 2.0, half * 2.0),
        font_size,
        windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_SEMI_BOLD,
        &text_brush,
        1,
        1,
    );

    // The second ring: same colour language, thinner and inside — two arcs
    // measuring the same kind of thing must read the same way.
    if let Some(second) = model.second_fraction {
        let second_radius = (diameter / 2.0 - line_width - SECOND_RING_SQUEEZE * scale) as f32;
        let second_lw = (2.5 * scale) as f32;
        let used = second.clamp(0.0, 1.0);
        let spent = model.second_is_spent || used >= 1.0;
        let shown = if model.shows_remaining && !spent { 1.0 - used } else { used };
        let track = circle_geometry(painter.engine, center, second_radius)?;
        let brush = painter.brush(panel::TRACK)?;
        painter.draw_geometry(&track, &brush, second_lw, true);
        if shown > 0.0 {
            if let Some(arc) = arc_geometry(painter.engine, center, second_radius, shown as f64)? {
                let color = match model.tint {
                    Some(t) if !spent => t,
                    _ => pulse_core::model::usage_tint::color(used, spent).into(),
                };
                let brush = painter.brush(if spent { Rgba::from(pulse_core::model::usage_tint::EXHAUSTED) } else { color })?;
                painter.draw_geometry(&arc, &brush, second_lw, true);
            }
        }
    }

    // The busy mark: white, on the empty circle, driven by the frame clock.
    if model.is_busy {
        let angle = (now_ms % BUSY_PERIOD_MS) / BUSY_PERIOD_MS * 360.0;
        if let Some(mark) =
            arc_sweep_geometry(painter.engine, center, busy_radius, angle - 90.0, BUSY_SWEEP_DEG)?
        {
            let brush = painter.brush(panel::TEXT_PRIMARY)?;
            painter.draw_geometry(&mark, &brush, (line_width * 0.5).max(1.5) as f32, true);
        }
    }

    // The window clock: a hairline outside the usage ring, in a neutral
    // rather than a second hue.
    if let Some(elapsed) = model.elapsed_fraction {
        let track = circle_geometry(painter.engine, center, clock_radius)?;
        let brush = painter.brush(panel::TRACK.with_alpha(0.16))?;
        painter.draw_geometry(&track, &brush, (2.0 * scale) as f32, true);
        if elapsed > 0.0 {
            if let Some(arc) = arc_geometry(painter.engine, center, clock_radius, elapsed)? {
                let brush = painter.brush(panel::TEXT_PRIMARY.with_alpha(0.7))?;
                painter.draw_geometry(&arc, &brush, (2.0 * scale) as f32, true);
            }
        }
    }

    Ok(())
}

impl From<[f32; 3]> for Rgba {
    fn from(c: [f32; 3]) -> Rgba {
        Rgba::rgb(c[0], c[1], c[2])
    }
}

/// What the arc and the halo are drawn in.
fn arc_colour(model: &RingModel) -> Rgba {
    let spent = model.is_spent || model.used_fraction.unwrap_or(0.0) >= 1.0;
    let automatic =
        Rgba::from(pulse_core::model::usage_tint::color(model.used_fraction.unwrap_or(0.0), spent));
    // Spent is the one state a chosen colour does not get to hide.
    match model.tint {
        Some(t) if !spent => t,
        _ => automatic,
    }
}

/// How much of the circle the coloured arc covers. **No reading draws
/// nothing, either way round** — `?? 0` inverted once drew a complete green
/// ring reading "all fine" for an account that had not answered. And spent
/// fills the ring whichever way it counts: nothing left must not be the
/// state with the least ink.
fn arc_fraction(model: &RingModel) -> f64 {
    let Some(used_fraction) = model.used_fraction else {
        return 0.0;
    };
    let used = used_fraction.clamp(0.0, 1.0);
    if model.is_spent || used >= 1.0 {
        return 1.0;
    }
    if model.shows_remaining {
        1.0 - used
    } else {
        used
    }
}

fn quiet(c: Rgba) -> Rgba {
    // Quietened while a reading is being fetched, never moved.
    c.with_alpha(c.a * 0.3)
}

/// The ring's centre point for slot `index`, given the rail's frame and
/// metrics. Shared by drawing and hit testing.
pub fn ring_center(m: &crate::geometry::Metrics, index: usize, rail: (f64, f64, f64, f64), edge: crate::geometry::Edge, docked: bool) -> Vector2 {
    use crate::geometry::{dock, Axis};
    let axis = edge.axis();
    let along = dock::ring_centre_along(m, index, axis, docked);
    let across = dock::ring_centre_across(m, axis);
    let (rx, ry, rw, _rh) = rail;
    if axis == Axis::Vertical {
        let x = match edge {
            crate::geometry::Edge::Right => rx + rw - across,
            _ => rx + across,
        };
        point(x as f32, (ry + along) as f32)
    } else {
        point((rx + along) as f32, (ry + across) as f32)
    }
}
