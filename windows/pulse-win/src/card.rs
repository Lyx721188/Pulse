//! The detail bubble, ported from `UsageDetailCard` and `UsageBubbleShape`.
//!
//! Card and pointer are **one path**, same winding. The window's name has
//! the top row to itself; spent and reset pair on the line below the bar.
//!
//! Layout units: the bubble **body** is the content width plus its padding;
//! the pointer is a strip beyond the body on the rail-facing side, ending
//! `HORIZONTAL_GAP` short of the rail.

use pulse_core::model::{ProviderUsage, State, Unavailability};
use windows::Win32::Graphics::Direct2D::Common::D2D1_FILL_MODE_WINDING;
use windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL;

use crate::d2d::{point, PathBuilder, Painter, Rgba};
use crate::geometry::{card, Edge, Metrics};
use crate::theme::panel;
use pulse_core::model::usage_tint;

#[derive(Clone)]
pub struct CardData {
    pub usage: ProviderUsage,
    /// What the card calls it: the account's label.
    pub title: String,
    pub monogram: String,
    pub shows_remaining: bool,
    pub shows_forecast: bool,
}

/// Card geometry resolved for a frame, in design units relative to the
/// panel window.
#[derive(Debug, Clone)]
pub struct CardFrame {
    /// The bubble **body** rect (without the pointer strip).
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Where the tip sits **along** the rail-facing side, measured from the
    /// body's own top (or leading) edge.
    pub pointer_center: f64,
    pub edge: Edge,
}

impl CardFrame {
    /// The body's total size for a reading of this shape.
    pub fn body_size(m: &Metrics, windows_count: usize, footnote: bool, forecast: bool) -> (f64, f64) {
        let w = m.s(card::WIDTH) + m.s(card::PADDING) * 2.0;
        let h = m.s(card::PADDING) * 2.0
            + m.s(card::HEADER_HEIGHT)
            + windows_count as f64 * (m.s(card::CONTENT_SPACING) + card::row_height(m, forecast))
            + if footnote {
                m.s(card::CONTENT_SPACING) + m.s(card::ROW_TEXT_LINE_HEIGHT)
            } else {
                0.0
            }
            // Room for a wrapped unavailable message even with no rows.
            .max(m.s(card::PADDING) * 2.0 + m.s(card::HEADER_HEIGHT) + m.s(card::ROW_TEXT_LINE_HEIGHT) * 4.0);
        (w, h)
    }

    /// Lays the card out for a ring centred at `ring_along` along the rail
    /// (or across, for the top edge), inside a window of `window` units.
    pub fn for_ring(
        m: &Metrics,
        edge: Edge,
        window: (f64, f64),
        rail: (f64, f64, f64, f64),
        ring_along: f64,
        windows_count: usize,
        footnote: bool,
        forecast: bool,
    ) -> CardFrame {
        let (w, h) = Self::body_size(m, windows_count.max(1), footnote, forecast);
        let (rx, ry, rw, rh) = rail;
        let (win_w, win_h) = window;
        let gap = m.s(card::HORIZONTAL_GAP);
        let pw = m.s(card::POINTER_WIDTH);
        let margin = 8.0;

        let (x, y, pointer_center) = match edge {
            Edge::Right => {
                let x = rx - gap - pw - w;
                // Centred on the ring where that fits; the pointer follows
                // the ring wherever the card ends up.
                let y = (ry + ring_along - h / 2.0).clamp(margin, (win_h - h - margin).max(margin));
                (x, y, ry + ring_along - y)
            }
            Edge::Left => {
                let x = rx + rw + gap + pw;
                let y = (ry + ring_along - h / 2.0).clamp(margin, (win_h - h - margin).max(margin));
                (x, y, ry + ring_along - y)
            }
            Edge::Top => {
                let y = ry + rh + gap + pw;
                let x = (rx + ring_along - w / 2.0).clamp(margin, (win_w - w - margin).max(margin));
                (x, y, rx + ring_along - x)
            }
        };

        CardFrame {
            x,
            y,
            w,
            h,
            pointer_center: pointer_center.clamp(h * 0.08, h * 0.92),
            edge,
        }
    }

    pub fn pointer_strip(&self, m: &Metrics) -> f64 {
        match self.edge {
            Edge::Top => 0.0,
            _ => m.s(card::POINTER_WIDTH),
        }
    }
}

/// The card and its pointer as one path, same winding. Walked clockwise
/// from the top-left corner; the rail-facing edge is interrupted by the
/// pointer.
fn bubble_path(
    painter: &Painter,
    m: &Metrics,
    frame: &CardFrame,
) -> windows::core::Result<windows::Win32::Graphics::Direct2D::ID2D1PathGeometry> {
    let x = frame.x;
    let y = frame.y;
    let w = frame.w;
    let h = frame.h;
    let r = m.s(card::CORNER_RADIUS);
    let pw = m.s(card::POINTER_WIDTH);
    let ph = m.s(card::POINTER_HEIGHT) / 2.0;
    let tip = frame.pointer_center;
    let k = r * 0.5523;

    let (mut bx, mut by, mut bw) = (x, y, w);
    let mut tip_point = (0.0f64, 0.0f64);
    let mut base_a = (0.0f64, 0.0f64);
    let mut base_b = (0.0f64, 0.0f64);

    match frame.edge {
        Edge::Right => {
            // Body on the left, pointer strip on the right pointing right.
            bx = x;
            tip_point = (x + w + pw, y + tip);
            base_a = (x + w, y + tip - ph);
            base_b = (x + w, y + tip + ph);
        }
        Edge::Left => {
            bw = w;
            tip_point = (x - pw, y + tip);
            base_a = (x, y + tip - ph);
            base_b = (x, y + tip + ph);
        }
        Edge::Top => {
            by = y;
            tip_point = (x + tip, y - pw);
            base_a = (x + tip - ph, y);
            base_b = (x + tip + ph, y);
        }
    }

    let _ = bw;
    let right = bx + w;
    let bottom = by + h;

    let mut builder = PathBuilder::new(painter.engine, D2D1_FILL_MODE_WINDING)?;

    // Walk the rounded body clockwise; insert the pointer where the
    // rail-facing edge runs.
    builder.begin_at(bx + r, by);
    match frame.edge {
        Edge::Top => {
            builder.line((tip_point.0 - ph).max(bx + r), by);
            builder.line(tip_point.0, tip_point.1);
            builder.line((tip_point.0 + ph).min(right - r), by);
        }
        _ => builder.line(right - r, by),
    }
    builder.curve(right - r + k, by, right, by + r - k, right, by + r);
    match frame.edge {
        Edge::Right => {
            builder.line(right, (base_a.1).max(by + r));
            builder.line(tip_point.0, tip_point.1);
            builder.line(right, (base_b.1).min(bottom - r));
        }
        _ => builder.line(right, bottom - r),
    }
    builder.curve(right, bottom - r + k, right - r + k, bottom, right - r, bottom);
    builder.line(bx + r, bottom);
    builder.curve(bx + r - k, bottom, bx, bottom - r + k, bx, bottom - r);
    match frame.edge {
        Edge::Left => {
            builder.line(bx, (base_b.1).min(bottom - r));
            builder.line(tip_point.0, tip_point.1);
            builder.line(bx, (base_a.1).max(by + r));
        }
        _ => builder.line(bx, by + r),
    }
    builder.curve(bx, by + r - k, bx + r - k, by, bx + r, by);
    builder.end_closed();

    builder.finish()
}

/// Draws the card.
pub fn draw_card(painter: &Painter, m: &Metrics, frame: &CardFrame, data: &CardData) -> windows::core::Result<()> {
    let surface = painter.brush(panel::CARD)?;
    let stroke = painter.brush(panel::CARD_STROKE)?;

    let bubble = bubble_path(painter, m, frame)?;
    painter.fill_geometry(&bubble, &surface);
    painter.draw_geometry(&bubble, &stroke, 1.0, false);

    // Content insets: the pointer strip lives outside the body on the
    // rail-facing side; the body carries the padding on all sides.
    let inset_x = frame.x + m.s(card::PADDING);
    let inset_w = frame.w - m.s(card::PADDING) * 2.0;
    let mut cy = frame.y + m.s(card::PADDING);

    // Header: the mark and the title, one line, always — the card's height
    // is budgeted, and a wrapped header would slice off against the edge.
    let title_brush = painter.brush(panel::TEXT_PRIMARY)?;
    painter.text(
        &data.monogram,
        crate::d2d::rect(
            inset_x as f32,
            cy as f32,
            (m.s(card::HEADER_ICON) + 4.0) as f32,
            m.s(card::HEADER_HEIGHT) as f32,
        ),
        (m.s(card::HEADER_ICON) * 0.8) as f32,
        windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_SEMI_BOLD,
        &title_brush,
        0,
        1,
    );
    painter.text(
        &format!("{} Usage", data.title),
        crate::d2d::rect(
            (inset_x + m.s(card::HEADER_ICON) + 8.0) as f32,
            cy as f32,
            (inset_w - m.s(card::HEADER_ICON) - 8.0) as f32,
            m.s(card::HEADER_HEIGHT) as f32,
        ),
        m.s(card::TITLE_FONT) as f32,
        windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_SEMI_BOLD,
        &title_brush,
        0,
        1,
    );
    cy += m.s(card::HEADER_HEIGHT);

    match &data.usage.state {
        State::Unavailable(reason) => {
            cy += m.s(card::CONTENT_SPACING);
            let message = if *reason == Unavailability::NotOnWindows {
                data.usage
                    .account
                    .provider
                    .windows_gap()
                    .unwrap_or(reason.message())
                    .to_string()
            } else {
                reason.message().to_string()
            };
            let message_brush = painter.brush(panel::TEXT_SECONDARY)?;
            draw_wrapped(
                painter,
                &message,
                inset_x,
                &mut cy,
                inset_w,
                m.s(card::MESSAGE_FONT),
                &message_brush,
            );
        }
        State::Live | State::Stale => {
            for window in &data.usage.windows {
                cy += m.s(card::CONTENT_SPACING);
                draw_progress_row(painter, m, inset_x, &mut cy, inset_w, window, data)?;
            }

            // A card with only a title reads as a card that failed to load:
            // DeepSeek on "balance only" reports money and no limits by
            // design, and the money is then the whole reading.
            if data.usage.windows.is_empty() {
                if let Some(balance) = &data.usage.credit_balance {
                    cy += m.s(card::CONTENT_SPACING);
                    draw_value_row(
                        painter,
                        m,
                        inset_x,
                        &mut cy,
                        inset_w,
                        &pulse_core::localization::t("Credit balance").to_string(),
                        balance,
                    );
                } else {
                    cy += m.s(card::CONTENT_SPACING);
                    let message_brush = painter.brush(panel::TEXT_SECONDARY)?;
                    draw_wrapped(
                        painter,
                        pulse_core::localization::t("No limits reported."),
                        inset_x,
                        &mut cy,
                        inset_w,
                        m.s(card::MESSAGE_FONT),
                        &message_brush,
                    );
                }
            }
        }
    }

    // The "as of" line: how much to trust the figures.
    if let State::Stale = data.usage.state {
        let stamp = data
            .usage
            .observed_at
            .map(|at| {
                pulse_core::localization::t_fmt(
                    "As of {time}",
                    &[&pulse_core::timeutil::relative_text(at)],
                )
            })
            .unwrap_or_else(|| pulse_core::localization::t("Reading may be out of date").to_string());
        let footnote_brush = painter.brush(panel::TEXT_DISABLED)?;
        cy += m.s(card::CONTENT_SPACING);
        painter.text(
            &stamp,
            crate::d2d::rect(
                inset_x as f32,
                cy as f32,
                inset_w as f32,
                m.s(card::ROW_TEXT_LINE_HEIGHT) as f32,
            ),
            m.s(card::FOOTNOTE_FONT) as f32,
            DWRITE_FONT_WEIGHT_NORMAL,
            &footnote_brush,
            0,
            0,
        );
    }

    Ok(())
}

/// One limit row: the name on its own line, the bar, spent + reset paired
/// below, and the forecast when it is shown.
fn draw_progress_row(
    painter: &Painter,
    m: &Metrics,
    x: f64,
    cy: &mut f64,
    width: f64,
    window: &pulse_core::model::UsageWindow,
    data: &CardData,
) -> windows::core::Result<()> {
    let spent_color = usage_tint::is_spent(Some(window));
    let accent = Rgba::from(usage_tint::color(window.used_fraction, window.is_exhausted));

    // The name gets the row to itself; a scoped name plus a reset time does
    // not fit one line, and the name is the half that says which limit
    // this is.
    let title_brush = painter.brush(panel::TEXT_PRIMARY)?;
    painter.text(
        &window.display_name(),
        crate::d2d::rect(
            x as f32,
            *cy as f32,
            width as f32,
            m.s(card::ROW_TEXT_LINE_HEIGHT) as f32,
        ),
        m.s(card::ROW_FONT) as f32,
        DWRITE_FONT_WEIGHT_NORMAL,
        &title_brush,
        0,
        1,
    );
    *cy += m.s(card::ROW_TEXT_LINE_HEIGHT) + m.s(card::ROW_INTERNAL_SPACING);

    // The bar: a capsule filled to the fraction. The arc follows the
    // figure; the colour still means closeness to the limit.
    let progress = if data.shows_remaining {
        window.remaining_fraction()
    } else {
        window.used_fraction.clamp(0.0, 1.0)
    };
    let bar_h = m.s(card::PROGRESS_BAR_HEIGHT) as f32;
    let bar_y = (*cy + m.s(card::PROGRESS_BAR_HEIGHT) / 2.0) as f32;
    let track_brush = painter.brush(panel::BAR_TRACK)?;
    draw_capsule(painter, x as f32, bar_y - bar_h / 2.0, width as f32, bar_h, &track_brush);

    let fill_w = (width * progress) as f32;
    if progress > 0.0 {
        let fill_brush = painter.brush(accent)?;
        // The smallest non-zero reading still puts a dot of colour on
        // screen — the same rule the ring's round cap follows.
        let fill_w = fill_w.max(bar_h).min(width as f32);
        draw_capsule(painter, x as f32, bar_y - bar_h / 2.0, fill_w, bar_h, &fill_brush);
    }
    *cy += m.s(card::PROGRESS_BAR_HEIGHT) + m.s(card::ROW_INTERNAL_SPACING);

    // The two short facts pair off: what is gone, and when it comes back.
    // The word has to follow the figure.
    let percent_text = window.percent_text(data.shows_remaining);
    let figure_label = if data.shows_remaining {
        pulse_core::localization::t_fmt("{p} Left", &[&percent_text])
    } else {
        pulse_core::localization::t_fmt("{p} Used", &[&percent_text])
    };
    let figure_brush = painter.brush(if spent_color {
        Rgba::from(usage_tint::EXHAUSTED)
    } else {
        panel::TEXT_SECONDARY
    })?;
    painter.text(
        &figure_label,
        crate::d2d::rect(x as f32, *cy as f32, (width * 0.45) as f32, m.s(card::ROW_TEXT_LINE_HEIGHT) as f32),
        m.s(card::ROW_FONT) as f32,
        windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_MEDIUM,
        &figure_brush,
        0,
        1,
    );

    let reset = reset_text(window);
    if !reset.is_empty() {
        let reset_brush = painter.brush(panel::TEXT_DISABLED)?;
        painter.text(
            &reset,
            crate::d2d::rect(
                (x + width * 0.4) as f32,
                *cy as f32,
                (width * 0.6) as f32,
                m.s(card::ROW_TEXT_LINE_HEIGHT) as f32,
            ),
            m.s(card::ROW_FONT) as f32,
            DWRITE_FONT_WEIGHT_NORMAL,
            &reset_brush,
            2,
            1,
        );
    }
    *cy += m.s(card::ROW_TEXT_LINE_HEIGHT);

    // The forecast: the one line the provider did not say, dimmer than the
    // figures above it, absent far more often than present — and the
    // verdict without the time when the time is past the horizon.
    if data.shows_forecast && !spent_color {
        if let Some(burn) = pulse_core::model::burn_rate::reading(window, pulse_core::timeutil::now_ms()) {
            *cy += m.s(card::ROW_INTERNAL_SPACING);
            let (text, color) = if let Some(ms) = burn.time_to_exhaustion_ms {
                (
                    pulse_core::localization::t_fmt(
                        "Runs out in {t}",
                        &[&pulse_core::model::burn_rate::approximate(ms)],
                    ),
                    Rgba::from(usage_tint::WARNING).with_alpha(0.9),
                )
            } else if burn.exhausts_before_reset {
                (
                    pulse_core::localization::t("Won't last the window").to_string(),
                    Rgba::from(usage_tint::WARNING).with_alpha(0.9),
                )
            } else {
                (
                    pulse_core::localization::t("Expected to last the window").to_string(),
                    panel::TEXT_DISABLED,
                )
            };
            let burn_brush = painter.brush(color)?;
            painter.text(
                &text,
                crate::d2d::rect(x as f32, *cy as f32, width as f32, m.s(card::ROW_TEXT_LINE_HEIGHT) as f32),
                m.s(card::ROW_FONT) as f32,
                DWRITE_FONT_WEIGHT_NORMAL,
                &burn_brush,
                0,
                1,
            );
            *cy += m.s(card::ROW_TEXT_LINE_HEIGHT);
        }
    }

    Ok(())
}

/// A capsule: two half-circle ends and a body. Cheaper than a path per bar,
/// and exactly the shape a progress capsule is.
fn draw_capsule(
    painter: &Painter,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    brush: &windows::Win32::Graphics::Direct2D::ID2D1SolidColorBrush,
) {
    let r = height / 2.0;
    if width <= height {
        painter.fill_ellipse(point(x + width / 2.0, y + r), width / 2.0, brush);
        return;
    }
    painter.fill_ellipse(point(x + r, y + r), r, brush);
    painter.fill_ellipse(point(x + width - r, y + r), r, brush);
    painter.fill_rounded_rect(
        crate::d2d::rect((x + r) as f32, y as f32, (width - height) as f32, height as f32),
        (height / 2.0) as f32,
        brush,
    );
}

fn draw_value_row(
    painter: &Painter,
    m: &Metrics,
    x: f64,
    cy: &mut f64,
    width: f64,
    title: &str,
    value: &str,
) -> windows::core::Result<()> {
    let title_brush = painter.brush(panel::TEXT_PRIMARY)?;
    painter.text(
        title,
        crate::d2d::rect(x as f32, *cy as f32, (width * 0.5) as f32, m.s(card::ROW_TEXT_LINE_HEIGHT) as f32),
        m.s(card::ROW_FONT) as f32,
        DWRITE_FONT_WEIGHT_NORMAL,
        &title_brush,
        0,
        1,
    );
    let value_brush = painter.brush(panel::TEXT_SECONDARY)?;
    painter.text(
        value,
        crate::d2d::rect(
            (x + width * 0.4) as f32,
            *cy as f32,
            (width * 0.6) as f32,
            m.s(card::ROW_TEXT_LINE_HEIGHT) as f32,
        ),
        m.s(card::ROW_FONT) as f32,
        windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_MEDIUM,
        &value_brush,
        2,
        1,
    );
    *cy += m.s(card::ROW_TEXT_LINE_HEIGHT);
    Ok(())
}

/// The reset line, or the length when the provider actually stated one —
/// never a sort key dressed as a measurement.
fn reset_text(window: &pulse_core::model::UsageWindow) -> String {
    match window.resets_at {
        Some(at) => pulse_core::localization::t_fmt("Resets {time}", &[&pulse_core::timeutil::reset_text(at)]),
        None => {
            if window.reports_length {
                window.length_text()
            } else {
                String::new()
            }
        }
    }
}

/// Wrapped text: breaks on spaces into lines that fit, up to four. The
/// card's messages are short; a full text layout engine is not needed for
/// them, and the row heights above are budgets, not measurements.
fn draw_wrapped(
    painter: &Painter,
    text: &str,
    x: f64,
    cy: &mut f64,
    width: f64,
    font: f64,
    brush: &windows::Win32::Graphics::Direct2D::ID2D1SolidColorBrush,
) {
    let mut line = String::new();
    let mut lines: Vec<String> = Vec::new();
    // Rough width estimate: a little over half the font size per character,
    // conservative for the CJK-heavy strings this table carries.
    let chars_per_line = ((width / (font * 0.62)).floor() as usize).max(8);
    for word in text.split_whitespace() {
        let candidate_len = line.chars().count() + word.chars().count() + 1;
        if candidate_len > chars_per_line && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines.truncate(4);
    let lh = font * 1.3;
    for (i, text) in lines.iter().enumerate() {
        painter.text(
            text,
            crate::d2d::rect(x as f32, (*cy + lh * i as f64) as f32, width as f32, lh as f32),
            font as f32,
            DWRITE_FONT_WEIGHT_NORMAL,
            brush,
            0,
            0,
        );
    }
    *cy += lh * lines.len() as f64;
}
