//! The detail card, ported from `UsageDetailCard`.
//!
//! The card is no longer a self-painted bubble inside the panel window —
//! it is the content of its own Mica flyout, the way a WinUI flyout sits on
//! the surface beneath it. So this module draws **content only**: the
//! header, the limit rows, the footnote. The window's name has the top row
//! to itself; spent and reset pair on the line below the bar.
//!
//! Layout units: everything is inset from the flyout's origin by
//! `card::PADDING` on all sides.

use pulse_core::model::{ProviderUsage, State, Unavailability};
use windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_NORMAL;

use crate::d2d::{point, Painter, Rgba};
use crate::geometry::{card, Metrics};
use crate::theme::panel;
use pulse_core::model::usage_tint;

#[derive(Clone)]
pub struct CardData {
    pub usage: ProviderUsage,
    /// What the card calls it: the account's label.
    pub title: String,
    pub monogram: String,
    /// The provider's mark, keyed like the ring's — the header draws it in
    /// place of the monogram when known.
    pub icon: Option<String>,
    pub shows_remaining: bool,
    pub shows_forecast: bool,
}

/// The card's total size for a reading of this shape — what the flyout
/// window has to be, in design units.
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
        .max(
            m.s(card::PADDING) * 2.0
                + m.s(card::HEADER_HEIGHT)
                + m.s(card::ROW_TEXT_LINE_HEIGHT) * 4.0,
        );
    (w, h)
}

/// Draws the card's content with its padding inset from `origin`.
pub fn draw_card(
    painter: &Painter,
    m: &Metrics,
    origin: (f64, f64),
    data: &CardData,
) -> windows::core::Result<()> {
    let palette = panel::palette();
    let inset_x = origin.0 + m.s(card::PADDING);
    let inset_w = m.s(card::WIDTH);
    let mut cy = origin.1 + m.s(card::PADDING);

    // Header: the mark and the title, one line, always — the card's height
    // is budgeted, and a wrapped header would slice off against the edge.
    let title_brush = painter.brush(palette.text_primary)?;
    let mark_size = m.s(card::HEADER_ICON) as f32;
    let mark_y = (cy + (m.s(card::HEADER_HEIGHT) - m.s(card::HEADER_ICON)) / 2.0) as f32;
    let mut drew_mark = false;
    if let Some(key) = &data.icon {
        if let Some(bitmap) = crate::assets::bitmap_for(painter.engine, key) {
            let dest = crate::d2d::rect(inset_x as f32, mark_y, mark_size, mark_size);
            if let Ok(ctx) = windows::core::Interface::cast::<
                windows::Win32::Graphics::Direct2D::ID2D1DeviceContext,
            >(painter.rt)
            {
                unsafe {
                    ctx.DrawBitmap(
                        &bitmap,
                        Some(&dest),
                        1.0,
                        windows::Win32::Graphics::Direct2D::D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                        None,
                        None,
                    );
                }
                drew_mark = true;
            }
        }
    }
    if !drew_mark {
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
    }
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
            let message_brush = painter.brush(palette.text_secondary)?;
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
                    let message_brush = painter.brush(palette.text_secondary)?;
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
            .unwrap_or_else(|| {
                pulse_core::localization::t("Reading may be out of date").to_string()
            });
        let footnote_brush = painter.brush(palette.text_disabled)?;
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
    let palette = panel::palette();
    let spent_color = usage_tint::is_spent(Some(window));

    // The name gets the row to itself; a scoped name plus a reset time does
    // not fit one line, and the name is the half that says which limit
    // this is.
    let title_brush = painter.brush(palette.text_primary)?;
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

    // The bar: a capsule filled to the fraction, in the system accent —
    // the whole surface speaks one colour; the figure below it carries the
    // meaning.
    let progress = if data.shows_remaining {
        window.remaining_fraction()
    } else {
        window.used_fraction.clamp(0.0, 1.0)
    };
    let bar_h = m.s(card::PROGRESS_BAR_HEIGHT) as f32;
    let bar_y = (*cy + m.s(card::PROGRESS_BAR_HEIGHT) / 2.0) as f32;
    let track_brush = painter.brush(palette.bar_track)?;
    draw_capsule(
        painter,
        x as f32,
        bar_y - bar_h / 2.0,
        width as f32,
        bar_h,
        &track_brush,
    );

    let fill_w = (width * progress) as f32;
    if progress > 0.0 {
        let fill_brush = painter.brush(panel::accent())?;
        // The smallest non-zero reading still puts a dot of colour on
        // screen — the same rule the ring's round cap follows.
        let fill_w = fill_w.max(bar_h).min(width as f32);
        draw_capsule(
            painter,
            x as f32,
            bar_y - bar_h / 2.0,
            fill_w,
            bar_h,
            &fill_brush,
        );
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
        palette.text_secondary
    })?;
    painter.text(
        &figure_label,
        crate::d2d::rect(
            x as f32,
            *cy as f32,
            (width * 0.45) as f32,
            m.s(card::ROW_TEXT_LINE_HEIGHT) as f32,
        ),
        m.s(card::ROW_FONT) as f32,
        windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_MEDIUM,
        &figure_brush,
        0,
        1,
    );

    let reset = reset_text(window);
    if !reset.is_empty() {
        let reset_brush = painter.brush(palette.text_disabled)?;
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
        if let Some(burn) =
            pulse_core::model::burn_rate::reading(window, pulse_core::timeutil::now_ms())
        {
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
                    palette.text_disabled,
                )
            };
            let burn_brush = painter.brush(color)?;
            painter.text(
                &text,
                crate::d2d::rect(
                    x as f32,
                    *cy as f32,
                    width as f32,
                    m.s(card::ROW_TEXT_LINE_HEIGHT) as f32,
                ),
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
        crate::d2d::rect(
            (x + r) as f32,
            y as f32,
            (width - height) as f32,
            height as f32,
        ),
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
    let palette = panel::palette();
    let title_brush = painter.brush(palette.text_primary)?;
    painter.text(
        title,
        crate::d2d::rect(
            x as f32,
            *cy as f32,
            (width * 0.5) as f32,
            m.s(card::ROW_TEXT_LINE_HEIGHT) as f32,
        ),
        m.s(card::ROW_FONT) as f32,
        DWRITE_FONT_WEIGHT_NORMAL,
        &title_brush,
        0,
        1,
    );
    let value_brush = painter.brush(palette.text_secondary)?;
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
        Some(at) => pulse_core::localization::t_fmt(
            "Resets {time}",
            &[&pulse_core::timeutil::reset_text(at)],
        ),
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
            crate::d2d::rect(
                x as f32,
                (*cy + lh * i as f64) as f32,
                width as f32,
                lh as f32,
            ),
            font as f32,
            DWRITE_FONT_WEIGHT_NORMAL,
            brush,
            0,
            0,
        );
    }
    *cy += lh * lines.len() as f64;
}
