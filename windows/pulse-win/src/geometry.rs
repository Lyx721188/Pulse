//! Panel geometry, a straight port of `DockLayout`, `DetailCardLayout` and
//! `PanelMetrics`. All numbers are **design units** — the macOS app's
//! points — and the renderer scales them to physical pixels with one
//! transform, so window sizing and drawing cannot drift apart.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
}

impl Edge {
    pub fn axis(self) -> Axis {
        match self {
            Edge::Top => Axis::Horizontal,
            _ => Axis::Vertical,
        }
    }

    pub fn is_vertical(self) -> bool {
        self.axis() == Axis::Vertical
    }

    pub fn from_name(name: &str) -> Edge {
        match name {
            "left" => Edge::Left,
            "top" => Edge::Top,
            _ => Edge::Right,
        }
    }
}

/// The scale every measurement is multiplied by: the panel size setting
/// times the rail spacing setting where the two apply.
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    pub scale: f64,
    pub spacing: f64,
    /// Whether the percent label sits above its ring rather than below.
    pub label_leads: bool,
    /// Whether the ring carries a percent label, per axis.
    pub side_percentages: bool,
    pub top_percentages: bool,
    pub capacity: usize,
}

impl Metrics {
    pub fn from_settings(settings: &pulse_core::settings::AppSettings, capacity: usize) -> Metrics {
        Metrics {
            scale: settings.scale(),
            spacing: settings.spacing(),
            label_leads: settings.label_above_ring,
            side_percentages: settings.side_rail_shows_percentages,
            top_percentages: settings.top_rail_shows_percentages,
            capacity: capacity.max(1),
        }
    }

    pub fn s(&self, units: f64) -> f64 {
        units * self.scale
    }

    fn shows_percentages(&self, axis: Axis) -> bool {
        match axis {
            Axis::Vertical => self.side_percentages,
            Axis::Horizontal => self.top_percentages,
        }
    }
}

/// The dock rail's constants, in design units — each one a **budget**, not a
/// taste: the window frame is computed from them before anything is drawn.
pub mod dock {
    use super::{Axis, Edge};

    use super::Metrics;

    pub const WIDTH: f64 = 64.0;
    pub const VERTICAL_PADDING: f64 = 46.0;
    pub const HORIZONTAL_PADDING: f64 = 10.0;
    pub const RING_DIAMETER: f64 = 36.0;
    pub const RING_LINE_WIDTH: f64 = 4.0;
    pub const RING_TO_TEXT: f64 = 6.0;
    pub const PERCENT_FONT: f64 = 13.0;
    pub const PERCENT_TEXT_HEIGHT: f64 = 16.0;
    pub const PERCENT_TEXT_WIDTH: f64 = 38.0;
    /// Reach of the two convex corners on the rail's inner side.
    pub const CORNER_RADIUS: f64 = 26.0;
    /// How far the concave flare rises above the rail body's flat top.
    pub const FLARE_HEIGHT: f64 = 24.0;
    /// How far in from the screen edge the flare starts sweeping.
    pub const FLARE_WIDTH: f64 = 38.0;
    pub const SECOND_RING_DIAMETER: f64 = 26.0;
    pub const SECOND_RING_LINE_WIDTH: f64 = 2.5;
    /// The sliver the rail hides down to.
    pub const COLLAPSED_WIDTH: f64 = 6.0;
    pub const COLLAPSED_HEIGHT: f64 = 96.0;
    pub const COLLAPSED_HIT_WIDTH: f64 = 20.0;

    /// Height of one ring + its percent label.
    pub fn item_height(m: &Metrics) -> f64 {
        m.s(RING_DIAMETER) + m.s(RING_TO_TEXT) + m.s(PERCENT_TEXT_HEIGHT)
    }

    /// The room left at each end of the rail before the first ring: less
    /// when floating, so the *visible* breathing room matches docked, where
    /// the flare carves `FLARE_HEIGHT` out of each end.
    pub fn end_padding(m: &Metrics, docked: bool) -> f64 {
        if docked {
            m.s(VERTICAL_PADDING)
        } else {
            m.s(VERTICAL_PADDING) - m.s(FLARE_HEIGHT)
        }
    }

    /// Whether an item carries its percent label, on either axis.
    pub fn shows_percentages(m: &Metrics, axis: Axis) -> bool {
        m.shows_percentages(axis)
    }

    /// How far into its item a ring starts — nothing at all unless the
    /// label is above it. Drawing and hit testing must both add this.
    pub fn ring_offset_in_item(m: &Metrics, axis: Axis) -> f64 {
        if !m.label_leads || !shows_percentages(m, axis) {
            return 0.0;
        }
        m.s(PERCENT_TEXT_HEIGHT) + m.s(RING_TO_TEXT)
    }

    /// One item's extent **along** the rail.
    pub fn item_length(m: &Metrics, axis: Axis) -> f64 {
        if !shows_percentages(m, axis) {
            return m.s(RING_DIAMETER);
        }
        match axis {
            Axis::Vertical => item_height(m),
            Axis::Horizontal => m.s(RING_DIAMETER).max(m.s(PERCENT_TEXT_WIDTH)),
        }
    }

    /// The rail's extent **across** its run: its width down a side, its
    /// height across the top.
    pub fn thickness(m: &Metrics, axis: Axis) -> f64 {
        if axis == Axis::Horizontal && shows_percentages(m, axis) {
            return item_height(m) + m.s(HORIZONTAL_PADDING) * 2.0;
        }
        m.s(WIDTH)
    }

    /// Rail length for a count of items: the padding at each end + the
    /// items + the gaps between them.
    pub fn length(m: &Metrics, item_count: usize, axis: Axis, docked: bool) -> f64 {
        let count = item_count.max(1) as f64;
        end_padding(m, docked) * 2.0
            + item_length(m, axis) * count
            + m.s(item_spacing(m)) * (count - 1.0)
    }

    /// The gap between the ring+label items, along the rail.
    pub fn item_spacing(_m: &Metrics) -> f64 {
        30.0
    }

    /// The rail's full size, laid the way `edge` lays it.
    pub fn size(m: &Metrics, item_count: usize, edge: Edge, docked: bool) -> (f64, f64) {
        let along = length(m, item_count, edge.axis(), docked);
        let across = thickness(m, edge.axis());
        if edge.is_vertical() {
            (across, along)
        } else {
            (along, across)
        }
    }

    /// Where a ring's centre sits **across** the rail.
    pub fn ring_centre_across(m: &Metrics, axis: Axis) -> f64 {
        let item = match axis {
            Axis::Vertical => m.s(RING_DIAMETER),
            Axis::Horizontal => {
                if shows_percentages(m, axis) {
                    item_height(m)
                } else {
                    m.s(RING_DIAMETER)
                }
            }
        };
        let lead = if axis == Axis::Horizontal {
            ring_offset_in_item(m, axis)
        } else {
            0.0
        };
        (thickness(m, axis) - item) / 2.0 + lead + m.s(RING_DIAMETER) / 2.0
    }

    /// How far along the rail the first ring's centre sits.
    pub fn first_ring_along(m: &Metrics, axis: Axis, docked: bool) -> f64 {
        let into_item = match axis {
            Axis::Vertical => ring_offset_in_item(m, axis) + m.s(RING_DIAMETER) / 2.0,
            Axis::Horizontal => item_length(m, axis) / 2.0,
        };
        end_padding(m, docked) + into_item
    }

    /// The step from one ring's centre to the next.
    pub fn ring_step(m: &Metrics, axis: Axis) -> f64 {
        item_length(m, axis) + m.s(item_spacing(m))
    }

    /// Ring centre along the rail for slot `index`.
    pub fn ring_centre_along(m: &Metrics, index: usize, axis: Axis, docked: bool) -> f64 {
        first_ring_along(m, axis, docked) + ring_step(m, axis) * index as f64
    }

    /// The rail at its longest, which is what the panel has to leave room
    /// for — measured docked, the longer of the two.
    pub fn maximum_length(m: &Metrics, axis: Axis) -> f64 {
        length(m, m.capacity, axis, true)
    }

    /// Which slot a point along the rail lands on, or none between rings.
    pub fn slot_at(m: &Metrics, along: f64, axis: Axis, docked: bool, count: usize) -> Option<usize> {
        if count == 0 {
            return None;
        }
        let first = first_ring_along(m, axis, docked);
        let step = ring_step(m, axis);
        let item = item_length(m, axis);
        let relative = along - first;
        if relative < -m.s(RING_DIAMETER) / 2.0 {
            return None;
        }
        let index = (relative / step).round() as i64;
        if index < 0 || index as usize >= count {
            return None;
        }
        let centre = index as f64 * step;
        // Within half a ring's diameter of the centre, or the item's own
        // span — whichever the labels make wider.
        let half = (item / 2.0).max(m.s(RING_DIAMETER) / 2.0 + 2.0);
        ((relative - centre).abs() <= half).then_some(index as usize)
    }
}

/// The detail bubble's constants, also budgets: the window frame is derived
/// from them before anything draws.
pub mod card {
    

    use super::Metrics;

    pub const WIDTH: f64 = 250.0;
    pub const PADDING: f64 = 18.0;
    pub const CORNER_RADIUS: f64 = 20.0;
    pub const POINTER_WIDTH: f64 = 20.0;
    pub const POINTER_HEIGHT: f64 = 40.0;
    /// Gap between the pointer's tip and the dock rail.
    pub const HORIZONTAL_GAP: f64 = 8.0;
    pub const CONTENT_SPACING: f64 = 14.0;
    pub const ROW_INTERNAL_SPACING: f64 = 7.0;
    pub const PROGRESS_BAR_HEIGHT: f64 = 6.0;
    pub const HEADER_HEIGHT: f64 = 19.0;
    pub const TITLE_FONT: f64 = 14.0;
    pub const ROW_FONT: f64 = 11.5;
    pub const MESSAGE_FONT: f64 = 12.0;
    pub const FOOTNOTE_FONT: f64 = 11.0;
    pub const HEADER_ICON: f64 = 16.0;
    pub const ROW_TEXT_LINE_HEIGHT: f64 = 14.0;

    /// One limit row: title, bar, percent + reset, and the forecast line
    /// when it is shown.
    pub fn row_height(m: &Metrics, with_forecast: bool) -> f64 {
        let mut height = m.s(ROW_TEXT_LINE_HEIGHT)
            + m.s(ROW_INTERNAL_SPACING)
            + m.s(PROGRESS_BAR_HEIGHT)
            + m.s(ROW_INTERNAL_SPACING)
            + m.s(ROW_TEXT_LINE_HEIGHT);
        if with_forecast {
            height += m.s(ROW_INTERNAL_SPACING) + m.s(ROW_TEXT_LINE_HEIGHT);
        }
        height
    }

    pub fn content_height(m: &Metrics, windows: usize, footnote: bool, with_forecast: bool) -> f64 {
        m.s(PADDING) * 2.0
            + m.s(HEADER_HEIGHT)
            + windows as f64 * (m.s(CONTENT_SPACING) + row_height(m, with_forecast))
            + (footnote as i64 as f64) * (m.s(CONTENT_SPACING) + m.s(ROW_TEXT_LINE_HEIGHT))
    }

    /// The tallest card the panel has to hold — more limits than are on
    /// screen today, because a card sliced flat against the window's edge
    /// looks like a rendering bug.
    pub fn maximum_height(m: &Metrics, with_forecast: bool) -> f64 {
        content_height(m, 5, true, with_forecast)
    }
}

/// The whole panel window's size for a given state, in design units: the
/// rail **or** the card, whichever is bigger, plus room for the pointer.
pub fn window_size(
    m: &Metrics,
    item_count: usize,
    edge: Edge,
    docked: bool,
    with_forecast: bool,
) -> (f64, f64) {
    let (rail_w, rail_h) = dock::size(m, item_count, edge, docked);
    let card_total_w = dock::WIDTH + card::HORIZONTAL_GAP + card::POINTER_WIDTH + card::WIDTH + card::PADDING * 2.0;
    let card_h = card::maximum_height(m, with_forecast);

    match edge {
        Edge::Right | Edge::Left => {
            let width = rail_w.max(card_total_w);
            let height = rail_h.max(card_h);
            (width, height)
        }
        Edge::Top => {
            // Across the top the card hangs below the rail.
            let width = rail_w.max(card_total_w);
            let height = (rail_h + card::HORIZONTAL_GAP + card::POINTER_WIDTH + card_h).max(rail_h);
            (width, height)
        }
    }
}
