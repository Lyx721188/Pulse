//! The floating panel: a transparent, non-activating layered window that
//! docks along a screen edge, shows one ring per account, opens a detail
//! card on hover, collapses to a sliver when idle, and drags with edge
//! fusion — the Windows counterpart of `FloatingPanel` + `FloatingPanelController`.
//!
//! The window owns size, placement and input; everything drawn in it comes
//! from the geometry module, whose numbers the window frame is computed
//! from. Hover is driven by pointer sampling on mouse-move plus
//! WM_MOUSELEAVE for the exit — the same enter-from-tracking,
//! leave-from-sampling split as the macOS app.

use std::sync::mpsc::Sender;

use pulse_core::model::{usage_tint, AccountKey, ProviderUsage, State, UsageWindow};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::berth;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use crate::card::{draw_card, CardData, CardFrame};
use crate::d2d::{global_engine, LayeredCanvas, Painter, Rgba};
use crate::geometry::{self, dock, Edge, Metrics};
use crate::rings::{draw_ring, ring_center, RingModel};
use crate::theme::panel;
use crate::winutil;

/// How long the pointer may be gone before the rail folds, when
/// auto-collapse is on.
const COLLAPSE_DELAY_MS: i64 = 1500;
/// How close a drag must come to an edge to fuse with it, in design units.
const FUSE_DISTANCE: f64 = 28.0;

pub enum PanelEvent {
    RefreshAccount(AccountKey),
    OpenSettings,
    PositionChanged,
}

/// One ring's worth of what the panel draws, snapshotted from the store.
#[derive(Clone)]
pub struct RailEntry {
    pub account: AccountKey,
    pub title: String,
    pub monogram: String,
    pub ring: RingModel,
    pub percent_text: String,
    /// What to draw under the ring when there is no percentage to draw:
    /// an em dash, or money.
    pub figure: String,
    pub headline: Option<UsageWindow>,
    /// The full reading, for the card.
    pub usage: Option<ProviderUsage>,
}

impl RailEntry {
    pub fn from_reading(usage: &ProviderUsage, settings: &pulse_core::settings::AppSettings, shows_remaining: bool) -> RailEntry {
        let account_id = usage.account.id();
        let pinned = settings.pinned_windows.get(&account_id).map(|s| s.as_str());
        let headline = usage.headline_window(pinned).cloned();
        let second = usage.second_window(pinned).cloned();
        let tint = settings.tint_for(&account_id).map(|c| Rgba::rgb(c[0], c[1], c[2]));

        let now = pulse_core::timeutil::now_ms();
        // The clock arc and the second ring are settings; the reading only
        // supplies the fractions.
        let elapsed = if settings.shows_window_clock {
            headline.as_ref().and_then(|w| w.elapsed_fraction(now))
        } else {
            None
        };
        let second = if settings.shows_second_ring {
            second
        } else {
            None
        };

        let (percent_text, figure) = match &headline {
            Some(w) => (w.percent_text(shows_remaining), String::new()),
            None => {
                // An em dash rather than 0% when nothing is known — but a
                // balance is a reading, and money is what it says.
                match &usage.credit_remaining {
                    Some(credit) => (String::new(), credit.rail_text()),
                    None => (String::new(), "—".to_string()),
                }
            }
        };

        // Refresh feedback is transient state set by a click and cleared
        // by the next snapshot, not something a reading carries.
        let refreshing_shown = false;

        let ring = RingModel {
            used_fraction: headline.as_ref().map(|w| w.used_fraction),
            has_reading: headline.is_some() || usage.credit_balance.is_some(),
            tint,
            is_spent: usage_tint::is_spent(headline.as_ref()),
            shows_remaining,
            monogram: usage.provider().monogram().to_string(),
            is_busy: false,
            is_refreshing: refreshing_shown,
            highlight: false,
            elapsed_fraction: elapsed,
            second_fraction: second.as_ref().map(|w| w.used_fraction),
            second_is_spent: usage_tint::is_spent(second.as_ref()),
        };

        RailEntry {
            account: usage.account.clone(),
            title: usage.provider().display_name().to_string(),
            monogram: usage.provider().monogram().to_string(),
            ring,
            percent_text,
            figure,
            headline,
            usage: Some(usage.clone()),
        }
    }

    pub fn placeholder(provider: pulse_core::model::Provider) -> RailEntry {
        RailEntry {
            account: AccountKey::primary(provider),
            title: provider.display_name().to_string(),
            monogram: provider.monogram().to_string(),
            ring: RingModel::unavailable(provider.monogram()),
            percent_text: String::new(),
            figure: "—".to_string(),
            headline: None,
            usage: None,
        }
    }
}

struct DragState {
    /// Where inside the window the grab happened, in units.
    grab: (f64, f64),
    moved: bool,
}

pub struct PanelWindow {
    pub hwnd: HWND,
    canvas: Option<LayeredCanvas>,
    pub entries: Vec<RailEntry>,
    m: Metrics,
    edge: Edge,
    docked: bool,
    /// 0 = sliver, 1 = full rail, animated between.
    openness: f64,
    expanded: bool,
    pointer_inside: bool,
    leave_at: Option<i64>,
    hover_slot: Option<usize>,
    card_slot: Option<usize>,
    drag: Option<DragState>,
    window_units: (f64, f64),
    dpi: f64,
    events: Sender<PanelEvent>,
    last_frame: i64,
    tracking_mouse: bool,
}

impl PanelWindow {
    pub fn new(events: Sender<PanelEvent>) -> Box<PanelWindow> {
        let class_name = w!("PulsePanel");
        unsafe {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(panel_wndproc),
                lpszClassName: class_name,
                hInstance: winutil::hinstance(),
                hCursor: winutil::arrow_cursor(),
                ..Default::default()
            };
            RegisterClassW(&wc);
        }

        let mut panel = Box::new(PanelWindow {
            hwnd: HWND::default(),
            canvas: None,
            entries: Vec::new(),
            m: Metrics::from_settings(&pulse_core::settings::with(|s| s.clone()), 1),
            edge: Edge::Right,
            docked: true,
            openness: 1.0,
            expanded: true,
            pointer_inside: false,
            leave_at: None,
            hover_slot: None,
            card_slot: None,
            drag: None,
            window_units: (0.0, 0.0),
            dpi: 1.0,
            events,
            last_frame: 0,
            tracking_mouse: false,
        });

        let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
        let style = WS_POPUP;
        unsafe {
            let hwnd = CreateWindowExW(
                ex,
                class_name,
                w!("Pulse"),
                style,
                0,
                0,
                10,
                10,
                None,
                None,
                Some(winutil::hinstance()),
                None,
            )
            .expect("CreateWindowExW for panel");
            panel.hwnd = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &*panel as *const PanelWindow as isize);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        panel.reload_settings();
        panel
    }

    /// Recomputes everything a settings change can move: metrics, edge,
    /// dock, window size, placement.
    pub fn reload_settings(&mut self) {
        let (scale, spacing, label_leads, side_pct, top_pct, floating, side, shows_remaining, shows_forecast) =
            pulse_core::settings::with(|s| {
                (
                    s.scale(),
                    s.spacing(),
                    s.label_above_ring,
                    s.side_rail_shows_percentages,
                    s.top_rail_shows_percentages,
                    s.floating,
                    s.dock_side.clone(),
                    s.shows_remaining,
                    s.shows_forecast,
                )
            });
        let _ = (shows_remaining, shows_forecast);

        self.m = Metrics {
            scale,
            spacing,
            label_leads,
            side_percentages: side_pct,
            top_percentages: top_pct,
            capacity: 17,
        };
        self.edge = Edge::from_name(&side);
        self.docked = !floating;
        self.expanded = true;
        self.openness = 1.0;
        self.compute_window_size();
        self.place();
        self.redraw();
    }

    fn forecast_enabled(&self) -> bool {
        pulse_core::settings::with(|s| s.shows_forecast)
    }

    fn auto_collapse(&self) -> bool {
        pulse_core::settings::with(|s| s.auto_collapse)
    }

    fn compute_window_size(&mut self) {
        let count = self.entries.len().max(1);
        let forecast = self.forecast_enabled();
        let (w, h) = geometry::window_size(&self.m, count, self.edge, self.docked, forecast);
        self.window_units = (w, h);
    }

    /// Sizes the canvas for the current monitor's DPI and resizes the
    /// window. The window is only ever resized here, never while a card
    /// opens — the rule the macOS app learned the hard way.
    pub fn place(&mut self) {
        let settings = pulse_core::settings::with(|s| s.clone());
        let (monitor_rect, work) = monitor_for_settings(&settings);

        // Per-monitor DPI: a panel drawn at the wrong scale is a panel
        // drawn wrong.
        let dpi_scale = work_dpi(&monitor_rect).max(0.5);
        let physical = (
            (self.window_units.0 * dpi_scale).ceil() as i32,
            (self.window_units.1 * dpi_scale).ceil() as i32,
        );

        if self.canvas.is_none() {
            self.dpi = dpi_scale;
            self.canvas = Some(
                LayeredCanvas::new(self.hwnd, global_engine(), physical.0.max(1), physical.1.max(1))
                    .expect("layered canvas"),
            );
        } else {
            let size_changed = self
                .canvas
                .as_ref()
                .map(|c| c.width != physical.0 || c.height != physical.1)
                .unwrap_or(false);
            if size_changed || (self.dpi - dpi_scale).abs() > 0.01 {
                self.dpi = dpi_scale;
                if let Some(canvas) = self.canvas.as_mut() {
                    canvas
                        .resize(physical.0.max(1), physical.1.max(1))
                        .expect("canvas resize");
                }
            }
        }

        // The frame, in physical pixels. Docked: flush to the edge of the
        // work area, rail centred. Floating: the stored ratio of the
        // display, clamped inside it.
        let (mut px, mut py) = match (self.docked, self.edge) {
            (true, Edge::Right) => (
                work.right - physical.0,
                work.top + (work.height() - physical.1) / 2,
            ),
            (true, Edge::Left) => (work.left, work.top + (work.height() - physical.1) / 2),
            (true, Edge::Top) => (
                work.left + (work.width() - physical.0) / 2,
                work.top,
            ),
            (false, _) => {
                let x = settings.float_x.clamp(0.0, 1.0);
                let y = settings.float_y.clamp(0.0, 1.0);
                (
                    work.left + ((work.width() - physical.0) as f64 * x) as i32,
                    work.top + ((work.height() - physical.1) as f64 * y) as i32,
                )
            }
        };
        // Clamp into the work area whatever happened.
        px = px.clamp(work.left, (work.right - physical.0).max(work.left));
        py = py.clamp(work.top, (work.bottom - physical.1).max(work.top));

        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                px,
                py,
                physical.0,
                physical.1,
                SWP_NOACTIVATE,
            );
        }
    }

    pub fn show(&mut self) {
        unsafe {
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        // Places, orders front, and places again: the first call puts the
        // window roughly right, the second measures against what it
        // actually got — the lesson `RailOffsetTests` pins on the macOS
        // side.
        self.place();
        self.redraw();
    }

    pub fn set_entries(&mut self, entries: Vec<RailEntry>) {
        let count_changed = entries.len() != self.entries.len();
        self.entries = entries;
        if count_changed {
            self.compute_window_size();
            self.place();
        }
        self.redraw();
    }

    /// The ring's model, updated in place so an animation state survives
    /// the snapshot swap.
    pub fn entries_mut(&mut self) -> &mut Vec<RailEntry> {
        &mut self.entries
    }

    pub fn redraw(&mut self) {
        let Some(canvas) = self.canvas.as_ref() else {
            return;
        };
        self.last_frame = pulse_core::timeutil::now_ms();

        canvas.begin();
        let engine = global_engine();
        let painter = Painter {
            rt: &canvas.rt,
            engine,
        };
        unsafe {
            painter
                .rt
                .SetTransform(&crate::d2d::scale_matrix(self.dpi as f32, self.dpi as f32));
        }

        let count = self.entries.len();
        let forecast = self.forecast_enabled();
        let now_ms = pulse_core::timeutil::now_ms() as f64;

        let openness = self.openness;
        if openness > 0.001 {
            // The berth's rect shrinks toward the docked edge as it closes;
            // the shape's own flare and corners wind down with it, which is
            // what makes sliver and rail one object.
            let rail = berth::rail_frame(&self.m, count.max(1), self.edge, self.docked, self.window_units);
            let sliver = berth::sliver_rect(&self.m, self.edge, self.window_units);
            let t = ease(openness);
            let rect = (
                lerp(sliver.0 as f64, rail.0, t),
                lerp(sliver.1 as f64, rail.1, t),
                lerp(sliver.2 as f64, rail.2, t),
                lerp(sliver.3 as f64, rail.3, t),
            );

            let surface = painter.brush(self.berth_color()).unwrap();
            let stroke = painter.brush(panel::SURFACE_STROKE).unwrap();
            if let Ok(path) = berth::berth_path(
                engine,
                rect.0 as f32,
                rect.1 as f32,
                rect.2 as f32,
                rect.3 as f32,
                self.edge,
                self.docked,
                openness,
                &self.m,
            ) {
                painter.fill_geometry(&path, &surface);
                painter.draw_geometry(&path, &stroke, 1.0, false);
            }

            // Rings fade up once the berth has opened enough to hold them.
            let alpha = ((openness - 0.55) / 0.35).clamp(0.0, 1.0);
            if alpha > 0.0 && count > 0 {
                let label_shows = if self.edge.is_vertical() {
                    self.m.side_percentages
                } else {
                    self.m.top_percentages
                };
                let ring_offset = dock::ring_offset_in_item(&self.m, self.edge.axis());
                for (index, entry) in self.entries.iter().enumerate() {
                    let mut model = entry.ring.clone();
                    model.highlight = self.hover_slot == Some(index);
                    let center = ring_center(&self.m, index, rail, self.edge, self.docked);
                    let _ = draw_ring(&painter, center, self.m.s(dock::RING_DIAMETER), self.m.s(dock::RING_LINE_WIDTH), self.m.scale, &model, now_ms);

                    if label_shows {
                        let text = if !entry.percent_text.is_empty() {
                            entry.percent_text.clone()
                        } else {
                            entry.figure.clone()
                        };
                        let spent = model.is_spent;
                        let color = if spent {
                            Rgba::from(usage_tint::EXHAUSTED)
                        } else if entry.headline.is_none() && entry.figure == "—" {
                            panel::TEXT_DISABLED
                        } else {
                            panel::TEXT_PRIMARY
                        }
                        .with_alpha(alpha as f32);
                        let brush = painter.brush(color).unwrap();
                        let text_w = self.m.s(dock::PERCENT_TEXT_WIDTH);
                        let text_h = self.m.s(dock::PERCENT_TEXT_HEIGHT);
                        let gap = self.m.s(dock::RING_TO_TEXT);
                        let (tx, ty) = if self.m.label_leads {
                            (
                                center.X - text_w as f32 / 2.0,
                                center.Y - (self.m.s(dock::RING_DIAMETER) / 2.0 + gap + text_h) as f32,
                            )
                        } else {
                            (
                                center.X - text_w as f32 / 2.0,
                                center.Y + (self.m.s(dock::RING_DIAMETER) / 2.0 + gap) as f32,
                            )
                        };
                        painter.text(
                            &text,
                            crate::d2d::rect(tx, ty, text_w as f32, text_h as f32),
                            self.m.s(dock::PERCENT_FONT) as f32,
                            windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_MEDIUM,
                            &brush,
                            1,
                            1,
                        );
                    }
                    let _ = ring_offset;
                }
            }
        } else {
            // The sliver, alone. It takes usage colour past the warning
            // threshold: hiding the rail must not hide something worth
            // seeing.
            let sliver = berth::sliver_rect(&self.m, self.edge, self.window_units);
            let surface = painter.brush(self.berth_color()).unwrap();
            let stroke = painter.brush(panel::SURFACE_STROKE).unwrap();
            if let Ok(path) = berth::berth_path(
                engine,
                sliver.0,
                sliver.1,
                sliver.2,
                sliver.3,
                self.edge,
                self.docked,
                0.0,
                &self.m,
            ) {
                painter.fill_geometry(&path, &surface);
                painter.draw_geometry(&path, &stroke, 1.0, false);
            }
        }

        // The card, when a ring is pointed at. The window does not resize
        // for it — the card is an overlay, not a stack sibling.
        if let Some(slot) = self.card_slot {
            if let Some(entry) = self.entries.get(slot) {
                if let Some(usage) = &entry.usage {
                    let rail = berth::rail_frame(&self.m, count.max(1), self.edge, self.docked, self.window_units);
                    let ring_along = dock::ring_centre_along(&self.m, slot, self.edge.axis(), self.docked);
                    let footnote = matches!(usage.state, State::Stale);
                    let frame = CardFrame::for_ring(
                        &self.m,
                        self.edge,
                        self.window_units,
                        rail,
                        ring_along,
                        usage.windows.len().max(1),
                        footnote,
                        forecast,
                    );
                    let data = CardData {
                        usage: usage.clone(),
                        title: entry.title.clone(),
                        monogram: entry.monogram.clone(),
                        shows_remaining: pulse_core::settings::with(|s| s.shows_remaining),
                        shows_forecast: forecast,
                    };
                    let _ = draw_card(&painter, &self.m, &frame, &data);
                }
            }
        }

        canvas.present();
    }

    /// The berth's fill: obsidian, or the warning colour on the collapsed
    /// sliver when a limit is close.
    fn berth_color(&self) -> Rgba {
        if self.openness < 0.5 || !self.expanded {
            if self.docked {
                if let Some(color) = self.alert_color() {
                    return color;
                }
            }
        }
        panel::SURFACE
    }

    fn alert_color(&self) -> Option<Rgba> {
        let threshold = pulse_core::settings::with(|s| s.alert_threshold as f64 / 100.0);
        self.entries.iter().filter_map(|e| e.headline.as_ref()).find_map(|w| {
            if w.is_exhausted || w.used_fraction >= 1.0 {
                Some(Rgba::from(usage_tint::EXHAUSTED))
            } else if w.used_fraction >= threshold {
                Some(Rgba::from(usage_tint::WARNING))
            } else {
                None
            }
        })
    }

    /// The animation tick: eases openness, refreshes marks, and collapses
    /// on schedule. Returns true while something still moves, so the app
    /// knows to keep the timer alive.
    pub fn tick(&mut self) -> bool {
        let now = pulse_core::timeutil::now_ms();

        // Collapse after the pointer has been gone long enough, docked
        // only: off the edge there is nothing to hide against.
        if self.auto_collapse() && self.docked && !self.pointer_inside && self.expanded {
            if let Some(leave_at) = self.leave_at {
                if now - leave_at >= COLLAPSE_DELAY_MS {
                    self.expanded = false;
                    self.card_slot = None;
                    self.hover_slot = None;
                }
            }
        }

        let target = if self.expanded { 1.0 } else { 0.0 };
        let speed = 0.14;
        self.openness += (target - self.openness) * speed;
        if (self.openness - target).abs() < 0.005 {
            self.openness = target;
        }
        let moving = (self.openness - target).abs() > 0.0005;

        // Any busy ring or open card keeps the frame clock alive.
        let busy = self.entries.iter().any(|e| e.ring.is_busy || e.ring.is_refreshing);
        let card = self.card_slot.is_some() && self.expanded;

        let needs_frame = moving || busy || card;
        if needs_frame {
            self.redraw();
        }
        needs_frame
    }

    pub fn on_mouse_move(&mut self, x: i32, y: i32) {
        if self.drag.is_some() {
            self.drag_move(x, y);
            return;
        }
        self.pointer_inside = true;
        self.leave_at = None;

        let (ux, uy) = (x as f64 / self.dpi, y as f64 / self.dpi);
        let count = self.entries.len();

        // Collapsed: a hit band wider than the sliver opens it — throwing
        // the pointer at the edge always lands on it.
        if !self.expanded {
            let sliver = berth::sliver_rect(&self.m, self.edge, self.window_units);
            let hit = self.m.s(dock::COLLAPSED_HIT_WIDTH) as f32;
            let inside = match self.edge {
                Edge::Right => ux as f32 >= sliver.0 - hit,
                Edge::Left => ux as f32 <= sliver.0 + sliver.2 + hit,
                Edge::Top => uy as f32 <= sliver.1 + sliver.3 + hit,
            };
            if inside {
                self.expanded = true;
                self.openness = self.openness.max(0.02);
            }
            self.track_mouse();
            return;
        }

        // Expanded: which ring, if any.
        let rail = berth::rail_frame(&self.m, count.max(1), self.edge, self.docked, self.window_units);
        let along = match self.edge {
            Edge::Top => ux - rail.0,
            _ => uy - rail.1,
        };
        let slot = geometry::dock::slot_at(&self.m, along, self.edge.axis(), self.docked, count);
        if slot != self.hover_slot {
            self.hover_slot = slot;
            self.card_slot = slot;
        }
        self.track_mouse();
    }

    pub fn on_mouse_leave(&mut self) {
        self.tracking_mouse = false;
        self.pointer_inside = false;
        if self.drag.is_some() {
            return;
        }
        self.leave_at = Some(pulse_core::timeutil::now_ms());
        self.hover_slot = None;
        self.card_slot = None;
        self.redraw();
    }

    fn track_mouse(&mut self) {
        if self.tracking_mouse {
            return;
        }
        self.tracking_mouse = true;
        unsafe {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut tme);
        }
    }

    pub fn on_l_button_down(&mut self, x: i32, y: i32) {
        let (ux, uy) = (x as f64 / self.dpi, y as f64 / self.dpi);
        let count = self.entries.len();

        // A click on a ring refreshes that account — the ring is a button.
        if self.expanded {
            let rail = berth::rail_frame(&self.m, count.max(1), self.edge, self.docked, self.window_units);
            let along = match self.edge {
                Edge::Top => ux - rail.0,
                _ => uy - rail.1,
            };
            if let Some(slot) = geometry::dock::slot_at(&self.m, along, self.edge.axis(), self.docked, count) {
                if let Some(entry) = self.entries.get(slot) {
                    let _ = self.events.send(PanelEvent::RefreshAccount(entry.account.clone()));
                    // Immediate visible feedback; the store's answer lands
                    // through the usual channel.
                    if let Some(e) = self.entries.get_mut(slot) {
                        e.ring.is_refreshing = true;
                    }
                    self.redraw();
                    return;
                }
            }
        }

        // Anything else is the drag handle: the berth area between the
        // rings claims the press.
        self.drag = Some(DragState {
            grab: (ux, uy),
            moved: false,
        });
        unsafe {
            let _ = SetCapture(self.hwnd);
        }
    }

    pub fn on_l_button_up(&mut self, _x: i32, _y: i32) {
        if let Some(drag) = self.drag.take() {
            unsafe {
                let _ = ReleaseCapture();
            }
            if drag.moved {
                self.persist_position();
                let _ = self.events.send(PanelEvent::PositionChanged);
            }
            // Re-place through the settings so a fused edge lands exactly
            // flush, measured against what the window was granted.
            self.reload_settings_preserving_openness();
        }
    }

    /// Re-runs placement without resetting the collapse animation.
    fn reload_settings_preserving_openness(&mut self) {
        let (scale, spacing, label_leads, side_pct, top_pct, floating, side) =
            pulse_core::settings::with(|s| {
                (
                    s.scale(),
                    s.spacing(),
                    s.label_above_ring,
                    s.side_rail_shows_percentages,
                    s.top_rail_shows_percentages,
                    s.floating,
                    s.dock_side.clone(),
                )
            });
        self.m = Metrics {
            scale,
            spacing,
            label_leads,
            side_percentages: side_pct,
            top_percentages: top_pct,
            capacity: 17,
        };
        self.edge = Edge::from_name(&side);
        self.docked = !floating;
        self.expanded = true;
        self.compute_window_size();
        self.place();
        self.redraw();
    }

    fn drag_move(&mut self, x: i32, y: i32) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        drag.moved = true;
        let (gx, gy) = drag.grab;

        // The pointer's monitor is the one that matters: clamping against
        // the window's own screen would make a second monitor unreachable.
        let cursor = POINT { x, y };
        let (monitor_rect, work) = winutil::monitor_work_at(cursor.x, cursor.y);
        let dpi_scale = work_dpi(&monitor_rect).max(0.5);
        let phys_w = (self.window_units.0 * dpi_scale) as i32;
        let phys_h = (self.window_units.1 * dpi_scale) as i32;

        let want_x = cursor.x - (gx * dpi_scale) as i32;
        let want_y = cursor.y - (gy * dpi_scale) as i32;

        // Fuse to a side **during** the drag, not on mouse-up.
        let fuse = self.m.s(FUSE_DISTANCE) as i32;
        let dist_right = (work.right - (want_x + phys_w)).abs();
        let dist_left = (want_x - work.left).abs();
        let dist_top = (want_y - work.top).abs();

        let (nx, ny, new_edge, new_docked) = if dist_right <= fuse {
            (work.right - phys_w, work.top + (work.height() - phys_h) / 2, Edge::Right, true)
        } else if dist_left <= fuse {
            (work.left, work.top + (work.height() - phys_h) / 2, Edge::Left, true)
        } else if dist_top <= fuse {
            (
                want_x.clamp(work.left, (work.right - phys_w).max(work.left)),
                work.top,
                Edge::Top,
                true,
            )
        } else {
            let x = want_x.clamp(work.left, (work.right - phys_w).max(work.left));
            let y = want_y.clamp(work.top, (work.bottom - phys_h).max(work.top));
            (x, y, self.edge, false)
        };

        if new_edge != self.edge || new_docked != self.docked {
            self.edge = new_edge;
            self.docked = new_docked;
            self.compute_window_size();
        }

        // Keep the DPI canvas in step when the drag crossed to another
        // monitor.
        if (self.dpi - dpi_scale).abs() > 0.01 {
            self.dpi = dpi_scale;
            if let Some(canvas) = self.canvas.as_mut() {
                let _ = canvas.resize(
                    ((self.window_units.0 * dpi_scale).ceil() as i32).max(1),
                    ((self.window_units.1 * dpi_scale).ceil() as i32).max(1),
                );
            }
        }

        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                nx,
                ny,
                ((self.window_units.0 * dpi_scale) as i32).max(1),
                ((self.window_units.1 * dpi_scale) as i32).max(1),
                SWP_NOACTIVATE,
            );
            let _ = monitor_rect;
        }
        self.redraw();
    }

    fn persist_position(&self) {
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        let (_, work) = winutil::monitor_work_at(pt.x, pt.y);
        let mut rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut rect);
        }
        // Store the **rail's** position, never the window's: the window is
        // much wider than the rail, and which side the rail sits on flips
        // at screen mid.
        let rail = berth::rail_frame(&self.m, self.entries.len().max(1), self.edge, self.docked, self.window_units);
        let rail_px_x = (rect.left as f64 + rail.0 * self.dpi) as i32;
        let rail_px_y = (rect.top as f64 + rail.1 * self.dpi) as i32;
        let rail_px_w = (rail.2 * self.dpi) as i32;
        let rail_px_h = (rail.3 * self.dpi) as i32;

        let work_w = (work.width() - rail_px_w).max(1) as f64;
        let work_h = (work.height() - rail_px_h).max(1) as f64;
        let x = ((rail_px_x - work.left) as f64 / work_w).clamp(0.0, 1.0);
        let y = ((rail_px_y - work.top) as f64 / work_h).clamp(0.0, 1.0);

        let name = winutil::monitor_name(winutil::monitor_from_hwnd(self.hwnd));
        pulse_core::settings::mutate(|s| {
            s.floating = !self.docked;
            s.dock_side = match self.edge {
                Edge::Left => "left".into(),
                Edge::Right => "right".into(),
                Edge::Top => "top".into(),
            };
            s.float_x = x;
            s.float_y = y;
            s.display = name;
        });
    }

    /// Follow the active display: the one holding the **pointer** — not the
    /// key window, not the frontmost app's frame. One panel, moved; the
    /// ratios carry across unchanged, so the rail keeps its place on a
    /// display whatever its size.
    pub fn follow_pointer_if_enabled(&mut self) -> bool {
        let (enabled, current_display) = pulse_core::settings::with(|s| (s.follows_active_display, s.display.clone()));
        if !enabled || self.drag.is_some() {
            return false;
        }
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        let monitor = unsafe {
            windows::Win32::Graphics::Gdi::MonitorFromPoint(
                pt,
                windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST,
            )
        };
        let name = winutil::monitor_name(monitor);
        if name == current_display {
            return false;
        }
        pulse_core::settings::mutate(|s| s.display = name);
        self.place();
        self.redraw();
        true
    }

    pub fn is_visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn ease(t: f64) -> f64 {
    // A gentle ease-out; the berth's own animation is the spring the macOS
    // side rides.
    1.0 - (1.0 - t) * (1.0 - t)
}

fn work_dpi(monitor_rect: &RECT) -> f64 {
    // The monitor's DPI, read through its handle when we have one; the
    // primary assumption is only a fallback for a rect nobody can name.
    let _ = monitor_rect;
    unsafe {
        let hdc = windows::Win32::Graphics::Gdi::GetDC(None);
        let dpi = windows::Win32::Graphics::Gdi::GetDeviceCaps(
            Some(hdc),
            windows::Win32::Graphics::Gdi::LOGPIXELSX,
        );
        let _ = windows::Win32::Graphics::Gdi::ReleaseDC(None, hdc);
        if dpi > 0 {
            return dpi as f64 / 96.0;
        }
    }
    1.0
}

fn monitor_for_settings(settings: &pulse_core::settings::AppSettings) -> (RECT, RECT) {
    // The remembered display, by device name; missing → primary.
    if !settings.display.is_empty() {
        let mut ctx = MonitorEnumCtx {
            wanted: settings.display.clone(),
            found: None,
        };
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::EnumDisplayMonitors(
                None,
                None,
                Some(enum_monitor_cb),
                LPARAM(&mut ctx as *mut MonitorEnumCtx as isize),
            );
        }
        if let Some(monitor) = ctx.found {
            return winutil::monitor_rects(monitor);
        }
    }
    let (monitor, _) = (
        unsafe {
            windows::Win32::Graphics::Gdi::MonitorFromPoint(
                POINT { x: 0, y: 0 },
                windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTOPRIMARY,
            )
        },
        (),
    );
    winutil::monitor_rects(monitor)
}

unsafe extern "system" fn enum_monitor_cb(
    monitor: windows::Win32::Graphics::Gdi::HMONITOR,
    _hdc: windows::Win32::Graphics::Gdi::HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> windows::core::BOOL {
    let ctx = &mut *(data.0 as *mut MonitorEnumCtx);
    let name = winutil::monitor_name(monitor);
    if name == ctx.wanted {
        ctx.found = Some(monitor);
        return false.into();
    }
    true.into()
}

struct MonitorEnumCtx {
    wanted: String,
    found: Option<windows::Win32::Graphics::Gdi::HMONITOR>,
}

/// The panel's window procedure. Everything arrives here first; drag and
/// ring clicks belong to the window, not to any view inside it.
unsafe extern "system" fn panel_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // Messages that arrive during CreateWindowExW precede the userdata
    // assignment; the state pointer is the ownership handover point.
    let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    if state == 0 {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let panel = &mut *(state as *mut PanelWindow);

    match msg {
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as u16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i32;
            panel.on_mouse_move(x, y);
            LRESULT(0)
        }
        _WM_MOUSELEAVE => {
            panel.on_mouse_leave();
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as u16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i32;
            panel.on_l_button_down(x, y);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let x = (lparam.0 & 0xFFFF) as u16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i32;
            panel.on_l_button_up(x, y);
            LRESULT(0)
        }
        WM_TIMER => {
            let alive = panel.tick();
            if !alive {
                // Keep a slow tick for collapse scheduling; the app drives
                // the fast one only while something moves.
                LRESULT(0)
            } else {
                LRESULT(0)
            }
        }
        WM_DESTROY => LRESULT(0),
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Forces a repaint from outside the window procedure.
pub fn invalidate(panel: &PanelWindow) {
    unsafe {
        let _ = InvalidateRect(Some(panel.hwnd), None, false);
    }
}

/// Small helper on RECT the window code uses everywhere.
trait RectExt {
    fn width(&self) -> i32;
    fn height(&self) -> i32;
}

impl RectExt for RECT {
    fn width(&self) -> i32 {
        self.right - self.left
    }

    fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

/// The work-area helper used by `place`; exposed for the settings window's
/// own clamp logic.
pub fn primary_work_area() -> RECT {
    let (_, work) = winutil::monitor_work_at(0, 0);
    work
}

/// Re-exported so `app.rs` can pass settings into panel metrics without
/// depending on the settings module's shape.
pub fn metrics_from_settings(capacity: usize) -> Metrics {
    pulse_core::settings::with(|s| Metrics::from_settings(s, capacity))
}

// GetMonitorInfoW is imported for the settings window's monitor lookup.
#[allow(unused_imports)]
use windows::Win32::Graphics::Gdi::GetMonitorInfoW as _;
