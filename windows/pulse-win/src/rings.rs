//! Ring painting, the Win11 clock's ring taken as the vocabulary: a thin
//! arc in the **system accent colour**, a barely-there track, the
//! provider's mark in the middle. One arc per limit; a hairline clock arc
//! outside when it is asked for; a thinner second ring inside.

use windows::core::Interface as _;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC;
use windows_numerics::Vector2;

use crate::d2d::{arc_geometry, arc_sweep_geometry, circle_geometry, point, Painter, Rgba};
use crate::theme::panel;

/// A travelling mark's share of its circle — short enough to read as a mark
/// rather than a second progress ring.
const BUSY_SWEEP_DEG: f64 = 0.22 * 360.0;
const BUSY_PERIOD_MS: f64 = 1000.0;
const REFRESH_SWEEP_DEG: f64 = 0.16 * 360.0;
const REFRESH_PERIOD_MS: f64 = 850.0;

/// The halo's reach beyond a ring, in design units. Lives with the panel
/// now — one glow chases the cursor beneath all the rings.
pub const HALO_RADIUS: f64 = 14.0;

/// The gap between the progress ring and the mark it encircles, and the
/// mark's share of the middle.
const CENTRE_GAP: f64 = 4.0;
const ICON_SCALE: f64 = 0.8;
/// How much the middle shrinks per side when the second ring takes the
/// band the activity mark was riding.
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
    pub is_spent: bool,
    /// Draw the arc as what is **left** rather than what is gone.
    pub shows_remaining: bool,
    /// The provider's monogram, drawn in the middle when no icon is known.
    pub monogram: String,
    /// The provider's mark — the Lobe icon the macOS app draws — keyed by
    /// provider raw name; None falls back to the monogram.
    pub icon: Option<String>,
    pub is_busy: bool,
    pub is_refreshing: bool,
    /// The pointed-at halo's strength, 0..1 — a spring value, not a flag,
    /// so the glow lands with a bounce.
    pub halo: f64,
    /// How much of the window has gone by, for the hairline clock arc.
    pub elapsed_fraction: Option<f64>,
    /// The next-fullest limit, drawn as a smaller ring inside this one.
    pub second_fraction: Option<f64>,
}

impl RingModel {
    pub fn unavailable(monogram: &str) -> RingModel {
        RingModel {
            used_fraction: None,
            has_reading: false,
            is_spent: false,
            shows_remaining: false,
            monogram: monogram.to_string(),
            icon: None,
            is_busy: false,
            is_refreshing: false,
            halo: 0.0,
            elapsed_fraction: None,
            second_fraction: None,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_ring(
    painter: &Painter,
    center: Vector2,
    mark: Vector2,
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
    let centre_diameter =
        (diameter - (line_width + CENTRE_GAP * scale) * 2.0 - squeeze * 2.0).max(0.0);

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

    let arc_color = panel::accent();

    // Track. The ring's track brightens with its emphasis, so a ring near
    // the pointer wakes up — arc, track and glow together, in proportion.
    let track = circle_geometry(painter.engine, center, radius)?;
    let track_color = {
        let base = panel::palette().track;
        base.with_alpha((base.a + 0.28 * model.halo as f32).min(1.0))
    };
    let brush = painter.brush(track_color)?;
    painter.draw_geometry(&track, &brush, lw, true);

    // Usage arc: always the accent. What it measures is written under it;
    // the colour is the system's, not a verdict.
    let fraction = arc_fraction(model);
    if let Some(arc) = arc_geometry(painter.engine, center, radius, fraction)? {
        let mut c = arc_color;
        if model.is_refreshing {
            c = quiet(c);
        }
        let brush = painter.brush(c)?;
        painter.draw_geometry(&arc, &brush, lw, true);
    }

    // The refresh mark: a short segment of the ring, travelling the whole
    // circle. It runs over the track and the usage arc alike, which is the
    // point — spinning the arc itself would make the feedback depend on the
    // number it is showing.
    if model.is_refreshing {
        let angle = (now_ms % REFRESH_PERIOD_MS) / REFRESH_PERIOD_MS * 360.0;
        if let Some(mark) = arc_sweep_geometry(
            painter.engine,
            center,
            radius,
            angle - 90.0,
            REFRESH_SWEEP_DEG,
        )? {
            let brush = painter.brush(arc_color)?;
            painter.draw_geometry(&mark, &brush, lw, true);
        }
    }

    // The provider's mark in the middle — the icon when one is known, the
    // monogram otherwise. It rides `mark`, its own point: the magnetic
    // lean tips it a little further than the ring, so the mark reads as
    // the attracted thing inside the attracted circle. Dimmed while there
    // is no reading, so the rail shows at a glance which providers it has
    // data for.
    let mark_alpha = if model.has_reading { 1.0 } else { 0.35 };
    let mut drew_icon = false;
    if let Some(key) = &model.icon {
        if let Some(bitmap) = crate::assets::bitmap_for(painter.engine, key) {
            let size = (centre_diameter * 0.58) as f32;
            let dest = D2D_RECT_F {
                left: mark.X - size / 2.0,
                top: mark.Y - size / 2.0,
                right: mark.X + size / 2.0,
                bottom: mark.Y + size / 2.0,
            };
            // Downscaling from the 256-px masters deserves the better
            // filter, which only the device-context DrawBitmap carries.
            if let Ok(ctx) = painter
                .rt
                .clone()
                .cast::<windows::Win32::Graphics::Direct2D::ID2D1DeviceContext>()
            {
                unsafe {
                    ctx.DrawBitmap(
                        &bitmap,
                        Some(&dest),
                        mark_alpha as f32,
                        D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                        None,
                        None,
                    );
                }
            }
            drew_icon = true;
        }
    }
    if !drew_icon {
        let text_brush =
            painter.brush(panel::palette().text_primary.with_alpha(mark_alpha as f32))?;
        let font_size = (centre_diameter * ICON_SCALE * 0.62) as f32;
        let half = centre_diameter as f32 / 2.0;
        painter.text(
            &model.monogram,
            crate::d2d::rect(mark.X - half, mark.Y - half, half * 2.0, half * 2.0),
            font_size,
            windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_SEMI_BOLD,
            &text_brush,
            1,
            1,
        );
    }

    // The second ring: the same accent, thinner and inside — two arcs
    // measuring the same kind of thing must read the same way.
    if let Some(second) = model.second_fraction {
        let second_radius = (diameter / 2.0 - line_width - SECOND_RING_SQUEEZE * scale) as f32;
        let second_lw = (2.5 * scale) as f32;
        let used = second.clamp(0.0, 1.0);
        let shown = if model.shows_remaining {
            1.0 - used
        } else {
            used
        };
        let track = circle_geometry(painter.engine, center, second_radius)?;
        let brush = painter.brush(panel::palette().track)?;
        painter.draw_geometry(&track, &brush, second_lw, true);
        if shown > 0.0 {
            if let Some(arc) = arc_geometry(painter.engine, center, second_radius, shown as f64)? {
                let brush = painter.brush(quiet(arc_color))?;
                painter.draw_geometry(&arc, &brush, second_lw, true);
            }
        }
    }

    // The busy mark: in the theme's own ink, on the empty circle, driven by
    // the frame clock.
    if model.is_busy {
        let angle = (now_ms % BUSY_PERIOD_MS) / BUSY_PERIOD_MS * 360.0;
        if let Some(mark) = arc_sweep_geometry(
            painter.engine,
            center,
            busy_radius,
            angle - 90.0,
            BUSY_SWEEP_DEG,
        )? {
            let brush = painter.brush(panel::palette().text_primary)?;
            painter.draw_geometry(&mark, &brush, (line_width * 0.5).max(1.5) as f32, true);
        }
    }

    // The window clock: a hairline outside the usage ring, in a neutral
    // rather than a second hue.
    if let Some(elapsed) = model.elapsed_fraction {
        let track = circle_geometry(painter.engine, center, clock_radius)?;
        let brush = painter.brush(panel::palette().track.with_alpha(0.16))?;
        painter.draw_geometry(&track, &brush, (2.0 * scale) as f32, true);
        if elapsed > 0.0 {
            if let Some(arc) = arc_geometry(painter.engine, center, clock_radius, elapsed)? {
                let brush = painter.brush(panel::palette().text_primary.with_alpha(0.7))?;
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
pub fn ring_center(
    m: &crate::geometry::Metrics,
    index: usize,
    rail: (f64, f64, f64, f64),
    edge: crate::geometry::Edge,
) -> Vector2 {
    use crate::geometry::{dock, Axis};
    let axis = edge.axis();
    let along = dock::ring_centre_along(m, index, axis);
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
