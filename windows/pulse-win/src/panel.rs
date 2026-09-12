//! The floating panel: a dock bar that hugs a screen edge with a margin of
//! empty desktop around it — the Windows counterpart of `FloatingPanel` +
//! `FloatingPanelController`.
//!
//! There is no painted surface in this window. It is a real Win11 material
//! window: DWM draws Mica behind the whole frame and rounds its corners,
//! and everything this module renders is ink on that material — one accent
//! ring per account, its percent beneath. The detail card is a separate
//! window (`flyout`), so its Mica is its own.
//!
//! Hover is driven by pointer sampling on mouse-move plus WM_MOUSELEAVE
//! for the exit; the card follows the ring the pointer is over, and lingers
//! briefly on the way out so a trip from ring to card does not flicker it.

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use pulse_core::model::{usage_tint, AccountKey, ProviderUsage, State, UsageWindow};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::card::{body_size as card_body_size, CardData};
use crate::d2d::{global_engine, Painter, SwapchainCanvas};
use crate::flyout::Flyout;
use crate::geometry::{self, card, dock, Edge, Metrics};
use crate::rings::{draw_ring, ring_center, RingModel};
use crate::theme::panel as theme_panel;
use crate::winutil;

/// How long the card lingers after the pointer leaves the bar, so a trip
/// from ring to card does not flicker it away.
const CARD_LINGER_MS: i64 = 220;
/// How long the pointer may be gone before the dock retreats off-screen —
/// the macOS app's collapse delay.
const RETREAT_DELAY_MS: i64 = 1500;
/// How much of the bar stays on screen while it is retreated, in physical
/// pixels: the sliver the pointer lands on to summon it back.
const PEEK_PX: f64 = 5.0;
/// How close a drag must come to an edge to fuse with it, in design units.
const FUSE_DISTANCE: f64 = 28.0;
/// The timer hands out 30 ms frames; the springs step in the same units.
const TICK_SECONDS: f64 = 0.03;

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
    pub fn from_reading(
        usage: &ProviderUsage,
        settings: &pulse_core::settings::AppSettings,
        shows_remaining: bool,
    ) -> RailEntry {
        let account_id = usage.account.id();
        let pinned = settings.pinned_windows.get(&account_id).map(|s| s.as_str());
        let headline = usage.headline_window(pinned).cloned();
        let second = usage.second_window(pinned).cloned();

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
            is_spent: usage_tint::is_spent(headline.as_ref()),
            shows_remaining,
            monogram: usage.provider().monogram().to_string(),
            icon: Some(usage.provider().raw().to_string()),
            is_busy: false,
            is_refreshing: refreshing_shown,
            halo: 0.0,
            elapsed_fraction: elapsed,
            second_fraction: second.as_ref().map(|w| w.used_fraction),
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
        let mut ring = RingModel::unavailable(provider.monogram());
        ring.icon = Some(provider.raw().to_string());
        RailEntry {
            account: AccountKey::primary(provider),
            title: provider.display_name().to_string(),
            monogram: provider.monogram().to_string(),
            ring,
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
    canvas: Option<SwapchainCanvas>,
    /// The hover card's window. Created after this window exists, because
    /// it needs the bar's HWND to report pointer traffic to.
    card: Option<Box<Flyout>>,
    pub entries: Vec<RailEntry>,
    m: Metrics,
    edge: Edge,
    docked: bool,
    /// The rings' arrival spring, 0 → 1 and a little past it — the port of
    /// the macOS app's `.spring(response: 0.32, dampingFraction: 0.86)`.
    presence: f64,
    presence_v: f64,
    /// The hover halo's spring, riding the same family of curves as the
    /// panel's selection spring on the macOS side.
    hover_spring: f64,
    hover_v: f64,
    /// The arcs' springs, per account: the displayed fraction trails the
    /// reading the way the macOS ring's arc animates
    /// (`.spring(response: 0.5, dampingFraction: 0.85)`), so a refreshed
    /// figure bounces to its new value instead of snapping.
    arc_springs: HashMap<String, (f64, f64, f64, f64)>,
    hover_slot: Option<usize>,
    card_slot: Option<usize>,
    /// The flyout's slide, in physical pixels — the port of the macOS
    /// panel's selection spring (`.spring(response: 0.34, dampingFraction:
    /// 0.82)`), which is what makes the card glide between rings instead
    /// of teleporting.
    card_x: f64,
    card_vx: f64,
    card_y: f64,
    card_vy: f64,
    card_target: Option<(i32, i32)>,
    /// The card content's entrance fade, 0 → 1 on the same spring family.
    card_alpha: f64,
    card_alpha_v: f64,
    /// The idle retreat: false docked out in the open, true slid almost
    /// fully off-screen. `recede` is the spring between the two — 0 at
    /// rest, 1 hidden, allowed to overshoot a little on the way.
    hidden: bool,
    recede: f64,
    recede_v: f64,
    hide_at: Option<i64>,
    base_pos: (i32, i32),
    hidden_pos: (i32, i32),
    phys_size: (i32, i32),
    /// When the pointer left the bar and the card should follow it out,
    /// unless it has moved onto the card first.
    leave_at: Option<i64>,
    drag: Option<DragState>,
    window_units: (f64, f64),
    dpi: f64,
    events: Sender<PanelEvent>,
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

        theme_panel::sync_theme();

        let mut panel = Box::new(PanelWindow {
            hwnd: HWND::default(),
            canvas: None,
            card: None,
            entries: Vec::new(),
            m: Metrics::from_settings(&pulse_core::settings::with(|s| s.clone()), 1),
            edge: Edge::Right,
            docked: true,
            presence: 0.0,
            presence_v: 0.0,
            hover_spring: 0.0,
            hover_v: 0.0,
            arc_springs: HashMap::new(),
            hover_slot: None,
            card_slot: None,
            card_x: 0.0,
            card_vx: 0.0,
            card_y: 0.0,
            card_vy: 0.0,
            card_target: None,
            card_alpha: 0.0,
            card_alpha_v: 0.0,
            hidden: false,
            recede: 0.0,
            recede_v: 0.0,
            hide_at: None,
            base_pos: (0, 0),
            hidden_pos: (0, 0),
            phys_size: (1, 1),
            leave_at: None,
            drag: None,
            window_units: (0.0, 0.0),
            dpi: 1.0,
            events,
            tracking_mouse: false,
        });

        // WS_CAPTION | WS_THICKFRAME stay even though the client area will
        // swallow the whole frame: DWM only draws a system backdrop on a
        // window it believes has a frame. `WM_NCCALCSIZE` returning 0
        // removes the frame's client-area cost, so the window is still one
        // borderless Mica surface — with Windows' own border and corners.
        let ex = WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
        let style = WS_CAPTION | WS_THICKFRAME;
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
            // Mica, Windows' own corner radius, and the dark/light variant
            // that matches the system — all DWM, none of it drawn here.
            winutil::apply_system_backdrop(hwnd, theme_panel::is_dark());
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        panel.card = Some(Flyout::new(panel.hwnd));
        panel.reload_settings();
        panel
    }

    fn card_shown(&self) -> bool {
        self.card.as_ref().map(|c| c.is_shown()).unwrap_or(false)
    }

    /// Recomputes everything a settings change can move: metrics, edge,
    /// dock, window size, placement.
    pub fn reload_settings(&mut self) {
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
        // A setting that forbids retreating also summons a retreated bar
        // straight back out.
        if !self.can_retreat() {
            self.set_hidden(false);
        }
        self.compute_window_size();
        self.place();
        self.redraw();
    }

    fn forecast_enabled(&self) -> bool {
        pulse_core::settings::with(|s| s.shows_forecast)
    }

    fn compute_window_size(&mut self) {
        let count = self.entries.len().max(1);
        self.window_units = dock::size(&self.m, count, self.edge);
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
                SwapchainCanvas::new(
                    self.hwnd,
                    global_engine(),
                    physical.0.max(1),
                    physical.1.max(1),
                )
                .expect("panel canvas"),
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

        // The bar sits `EDGE_MARGIN` inside the work area on the edge it
        // docks to — a dock, not a strip glued to the glass. Floating: the
        // stored ratio of the display, clamped inside it with the same
        // margin.
        let margin = (dock::EDGE_MARGIN * dpi_scale) as i32;
        let (mut px, mut py) = match (self.docked, self.edge) {
            (true, Edge::Right) => (
                work.right - physical.0 - margin,
                work.top + (work.height() - physical.1) / 2,
            ),
            (true, Edge::Left) => (
                work.left + margin,
                work.top + (work.height() - physical.1) / 2,
            ),
            (true, Edge::Top) => (
                work.left + (work.width() - physical.0) / 2,
                work.top + margin,
            ),
            (false, _) => {
                let x = settings.float_x.clamp(0.0, 1.0);
                let y = settings.float_y.clamp(0.0, 1.0);
                (
                    work.left
                        + margin
                        + ((work.width() - physical.0 - margin * 2) as f64 * x) as i32,
                    work.top
                        + margin
                        + ((work.height() - physical.1 - margin * 2) as f64 * y) as i32,
                )
            }
        };
        // Clamp into the work area whatever happened.
        px = px.clamp(
            work.left + margin,
            (work.right - physical.0 - margin).max(work.left + margin),
        );
        py = py.clamp(
            work.top + margin,
            (work.bottom - physical.1 - margin).max(work.top + margin),
        );

        // Where the bar rests, and where it retreats to when idle: slid
        // almost fully off the screen, with a sliver of itself left as the
        // target for the pointer's return.
        let peek = (PEEK_PX as f64 * dpi_scale) as i32;
        self.base_pos = (px, py);
        self.phys_size = physical;
        self.hidden_pos = match self.edge {
            Edge::Right => (work.right - peek, py),
            Edge::Left => (work.left, py),
            Edge::Top => (px, work.top),
        };
        self.apply_position();
    }

    /// Sets the window's rect for the retreat spring's current value — a
    /// plain interpolation between the resting place and the hidden one.
    fn apply_position(&mut self) {
        let t = self.recede;
        let x = self.base_pos.0 as f64 + (self.hidden_pos.0 - self.base_pos.0) as f64 * t;
        let y = self.base_pos.1 as f64 + (self.hidden_pos.1 - self.base_pos.1) as f64 * t;
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x as i32,
                y as i32,
                self.phys_size.0,
                self.phys_size.1,
                SWP_NOACTIVATE,
            );
        }
    }

    /// Whether the dock should ever retreat: the setting says so, and only
    /// a docked bar retreats — a freely floating panel keeps its place.
    fn can_retreat(&self) -> bool {
        self.docked && pulse_core::settings::with(|s| s.auto_collapse)
    }

    /// The retreat state machine's switch. Retreating closes the card: a
    /// bar that has left the screen has nothing to point at.
    fn set_hidden(&mut self, hidden: bool) {
        if self.hidden == hidden {
            return;
        }
        self.hidden = hidden;
        if hidden {
            self.hide_card();
        }
    }

    pub fn show(&mut self) {
        unsafe {
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        // Re-summoned from the tray: the rings bounce in again, which is
        // the arrival the launch eased through.
        self.presence = 0.0;
        self.presence_v = 0.0;
        self.place();
        self.redraw();
    }

    pub fn set_entries(&mut self, entries: Vec<RailEntry>) {
        let count_changed = entries.len() != self.entries.len();
        // Springs survive snapshot swaps for the accounts that stay; a
        // fresh account starts its arc at zero, so its reading lands with
        // the arrival bounce.
        let old = std::mem::take(&mut self.arc_springs);
        self.arc_springs = entries
            .iter()
            .map(|e| {
                let id = e.account.id();
                let seed = old.get(&id).copied().unwrap_or((0.0, 0.0, 0.0, 0.0));
                (id, seed)
            })
            .collect();
        self.entries = entries;
        if count_changed {
            self.compute_window_size();
            self.place();
        }
        if self
            .card_slot
            .map(|s| s >= self.entries.len())
            .unwrap_or(false)
        {
            self.hide_card();
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
        let now_ms = pulse_core::timeutil::now_ms() as f64;

        // The rail is the window: Mica behind it all, ink only here.
        let rail = (0.0, 0.0, self.window_units.0, self.window_units.1);
        // The arrival spring drives both the fade and the ring's size, so
        // the overshoot reads as a bounce, not as a flicker. The hover
        // spring is the focus gesture — and it belongs to the **pointed-at
        // ring alone**: halo, track brightness and a step of growth, all
        // riding one spring.
        let arrive = self.presence.clamp(0.0, 1.0);
        let ring_scale = 0.55 + 0.45 * self.presence;
        let halo = self.hover_spring;
        let focus_grow = 1.0 + 0.1 * halo;
        if arrive > 0.0 && count > 0 {
            let label_shows = if self.edge.is_vertical() {
                self.m.side_percentages
            } else {
                self.m.top_percentages
            };
            let ink = theme_panel::palette().text_primary;
            for (index, entry) in self.entries.iter().enumerate() {
                let is_hover = self.hover_slot == Some(index);
                let mut model = entry.ring.clone();
                model.halo = if is_hover { halo } else { 0.0 };
                let center = ring_center(&self.m, index, rail, self.edge);
                let _ = draw_ring(
                    &painter,
                    center,
                    self.m.s(dock::RING_DIAMETER)
                        * ring_scale
                        * if is_hover { focus_grow } else { 1.0 },
                    self.m.s(dock::RING_LINE_WIDTH),
                    self.m.scale,
                    &model,
                    now_ms,
                );

                if label_shows {
                    let text = if !entry.percent_text.is_empty() {
                        entry.percent_text.clone()
                    } else {
                        entry.figure.clone()
                    };
                    let color = if entry.headline.is_none() && entry.figure == "—" {
                        theme_panel::palette().text_disabled
                    } else {
                        ink
                    }
                    .with_alpha((arrive) as f32);
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
            }
        }

        canvas.present();

        // Keep the flyout's content in step: a refresh finishing while the
        // card is open re-draws it from the new snapshot.
        if self.card_shown() {
            if let Some(slot) = self.card_slot {
                if let Some((data, _)) = self.card_payload(slot) {
                    if let Some(flyout) = self.card.as_mut() {
                        flyout.draw(&data, &self.m, self.card_alpha.min(1.0) as f32);
                    }
                }
            }
        }
    }

    /// What the card for `slot` draws and how big it is.
    fn card_payload(&self, slot: usize) -> Option<(CardData, (f64, f64))> {
        let entry = self.entries.get(slot)?;
        let usage = entry.usage.as_ref()?;
        let forecast = self.forecast_enabled();
        let footnote = matches!(usage.state, State::Stale);
        let data = CardData {
            usage: usage.clone(),
            title: entry.title.clone(),
            monogram: entry.monogram.clone(),
            icon: Some(usage.provider().raw().to_string()),
            shows_remaining: pulse_core::settings::with(|s| s.shows_remaining),
            shows_forecast: forecast,
        };
        Some((
            data,
            card_body_size(&self.m, usage.windows.len().max(1), footnote, forecast),
        ))
    }

    /// Puts the card beside the ring it points at — on the desktop side of
    /// the bar, vertically centred on the ring, clamped into the monitor.
    /// First appearance lands in place and fades up on the spring; a move
    /// between rings slides on the position spring.
    fn place_card(&mut self) {
        let Some(slot) = self.card_slot else {
            self.hide_card();
            return;
        };
        let Some((data, (cw, ch))) = self.card_payload(slot) else {
            self.hide_card();
            return;
        };

        let mut rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut rect);
        }
        let (_, work) =
            winutil::monitor_work_at((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
        let dpi = self.dpi;
        let rail = (0.0, 0.0, self.window_units.0, self.window_units.1);
        let center = ring_center(&self.m, slot, rail, self.edge);
        let ring_x = rect.left as f64 + center.X as f64 * dpi;
        let ring_y = rect.top as f64 + center.Y as f64 * dpi;
        let gap = self.m.s(card::HORIZONTAL_GAP) * dpi;
        let w = cw * dpi;
        let h = ch * dpi;

        let (mut tx, mut ty) = match self.edge {
            Edge::Right => (rect.left as f64 - gap - w, ring_y - h / 2.0),
            Edge::Left => (rect.right as f64 + gap, ring_y - h / 2.0),
            Edge::Top => (ring_x - w / 2.0, rect.bottom as f64 + gap),
        };
        tx = tx.clamp(
            work.left as f64,
            (work.right as f64 - w).max(work.left as f64),
        );
        ty = ty.clamp(
            work.top as f64,
            (work.bottom as f64 - h).max(work.top as f64),
        );

        if self.card_shown() {
            // A move between rings: hand the destination to the spring and
            // resize in place at wherever the slide has reached.
            self.card_target = Some((tx as i32, ty as i32));
            let flyout = self.card.as_mut().expect("panel flyout");
            flyout.show_at(self.card_x as i32, self.card_y as i32, (cw, ch), dpi);
        } else {
            // First appearance lands where it belongs, empty, and the
            // content fades up as the entrance.
            self.card_x = tx;
            self.card_y = ty;
            self.card_vx = 0.0;
            self.card_vy = 0.0;
            self.card_alpha = 0.0;
            self.card_alpha_v = 0.0;
            self.card_target = Some((tx as i32, ty as i32));
            let flyout = self.card.as_mut().expect("panel flyout");
            flyout.show_at(tx as i32, ty as i32, (cw, ch), dpi);
        }
        let flyout = self.card.as_mut().expect("panel flyout");
        flyout.draw(&data, &self.m, self.card_alpha.min(1.0) as f32);
    }

    fn hide_card(&mut self) {
        if let Some(flyout) = self.card.as_mut() {
            flyout.hide();
        }
        self.card_slot = None;
        self.hover_slot = None;
        self.card_target = None;
    }

    /// The animation tick: steps the springs (arrival, hover halo, arcs)
    /// and settles the card's linger. Returns true while something still
    /// moves, so the app knows to keep the frame clock alive.
    pub fn tick(&mut self) -> bool {
        let now = pulse_core::timeutil::now_ms();
        let dt = TICK_SECONDS;

        // The springs: each one steps toward its target with the same
        // fixed dt the timer hands out.
        spring_step(
            &mut self.presence,
            &mut self.presence_v,
            1.0,
            0.32,
            0.86,
            dt,
        );
        let hover_target = if self.hover_slot.is_some() { 1.0 } else { 0.0 };
        spring_step(
            &mut self.hover_spring,
            &mut self.hover_v,
            hover_target,
            0.34,
            0.82,
            dt,
        );
        for entry in &self.entries {
            let Some(s) = self.arc_springs.get_mut(&entry.account.id()) else {
                continue;
            };
            if let Some(target) = entry.ring.used_fraction {
                spring_step(&mut s.0, &mut s.1, target, 0.5, 0.85, dt);
            } else {
                s.0 = 0.0;
                s.1 = 0.0;
            }
            if let Some(target) = entry.ring.second_fraction {
                spring_step(&mut s.2, &mut s.3, target, 0.5, 0.85, dt);
            } else {
                s.2 = 0.0;
                s.3 = 0.0;
            }
        }
        // The springs write their displayed values straight onto the
        // models, so drawing stays a read.
        for entry in &mut self.entries {
            if let Some(s) = self.arc_springs.get(&entry.account.id()) {
                if entry.ring.used_fraction.is_some() {
                    entry.ring.used_fraction = Some(s.0.clamp(0.0, 1.0));
                }
                if entry.ring.second_fraction.is_some() {
                    entry.ring.second_fraction = Some(s.2.clamp(0.0, 1.0));
                }
            }
        }

        let moving = !(settled(self.presence, self.presence_v, 1.0)
            && settled(self.hover_spring, self.hover_v, hover_target))
            || self.entries.iter().any(|e| {
                let Some(s) = self.arc_springs.get(&e.account.id()) else {
                    return false;
                };
                e.ring.used_fraction.is_some_and(|t| !settled(s.0, s.1, t))
                    || e.ring
                        .second_fraction
                        .is_some_and(|t| !settled(s.2, s.3, t))
            });
        let mut moving = moving;

        // The card's slide toward its ring.
        if let (Some((tx, ty)), true) = (self.card_target, self.card_shown()) {
            spring_step(
                &mut self.card_x,
                &mut self.card_vx,
                tx as f64,
                0.34,
                0.82,
                dt,
            );
            spring_step(
                &mut self.card_y,
                &mut self.card_vy,
                ty as f64,
                0.34,
                0.82,
                dt,
            );
            if !(settled(self.card_x, self.card_vx, tx as f64)
                && settled(self.card_y, self.card_vy, ty as f64))
            {
                if let Some(flyout) = self.card.as_mut() {
                    flyout.move_to(self.card_x as i32, self.card_y as i32);
                }
                moving = true;
            }
            // The content's fade-up after (or during) the slide.
            if self.card_alpha < 1.0 {
                spring_step(
                    &mut self.card_alpha,
                    &mut self.card_alpha_v,
                    1.0,
                    0.32,
                    0.86,
                    dt,
                );
                moving = true;
            }
        }

        // The linger: the pointer left the bar, the card waits a beat for
        // it to arrive — on the card, or on another ring.
        let mut card_changed = false;
        if let Some(at) = self.leave_at {
            if now >= at {
                self.leave_at = None;
                let inside = self
                    .card
                    .as_ref()
                    .map(|c| c.pointer_inside)
                    .unwrap_or(false);
                if !inside {
                    self.hide_card();
                    card_changed = true;
                }
            }
        }

        // The idle retreat: the pointer has been gone long enough, and the
        // bar slides off-screen to a sliver of itself.
        if let Some(at) = self.hide_at {
            if now >= at {
                self.hide_at = None;
                self.set_hidden(true);
            }
        }

        // The retreat spring: the bar's own travel between its resting
        // place and the screen edge, the "sinking in" the dock does when
        // nothing needs it.
        let recede_target = if self.hidden { 1.0 } else { 0.0 };
        if !settled(self.recede, self.recede_v, recede_target) {
            spring_step(
                &mut self.recede,
                &mut self.recede_v,
                recede_target,
                0.45,
                0.8,
                dt,
            );
            self.apply_position();
            moving = true;
        }

        // Any busy ring keeps the frame clock alive.
        let busy = self
            .entries
            .iter()
            .any(|e| e.ring.is_busy || e.ring.is_refreshing);
        if moving || busy || card_changed {
            self.redraw();
        }
        moving || busy
    }

    pub fn on_mouse_move(&mut self, x: i32, y: i32) {
        if self.drag.is_some() {
            self.drag_move(x, y);
            return;
        }
        self.leave_at = None;
        // The pointer found the bar — wherever it was, the dock comes back
        // out. This is the retreated sliver's summons.
        if self.hidden {
            self.set_hidden(false);
        }
        self.hide_at = None;

        let (ux, uy) = (x as f64 / self.dpi, y as f64 / self.dpi);
        let count = self.entries.len();
        let rail = (0.0, 0.0, self.window_units.0, self.window_units.1);
        let along = match self.edge {
            Edge::Top => ux - rail.0,
            _ => uy - rail.1,
        };
        let slot = geometry::dock::slot_at(&self.m, along, self.edge.axis(), count);
        if slot != self.hover_slot {
            self.hover_slot = slot;
            self.card_slot = slot;
            if slot.is_some() {
                self.place_card();
            } else {
                self.hide_card();
            }
            self.redraw();
        }
        self.track_mouse();
    }

    pub fn on_mouse_leave(&mut self) {
        self.tracking_mouse = false;
        if self.drag.is_some() {
            return;
        }
        // Not immediate: the pointer may be on its way to the card.
        self.leave_at = Some(pulse_core::timeutil::now_ms() + CARD_LINGER_MS);
        // And if it stays gone, the dock sinks back off-screen — unless a
        // setting or a free-floating panel says it has nowhere to go.
        if self.can_retreat() {
            self.hide_at = Some(pulse_core::timeutil::now_ms() + RETREAT_DELAY_MS);
        }
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

    /// The flyout reports pointer traffic: `entered` when it arrived on
    /// the card, false when it left. Leaving the card schedules the same
    /// linger as leaving the bar, in case the pointer is heading back.
    pub fn on_card_pointer(&mut self, entered: bool) {
        if entered {
            self.leave_at = None;
        } else {
            self.leave_at = Some(pulse_core::timeutil::now_ms() + CARD_LINGER_MS);
        }
    }

    pub fn on_l_button_down(&mut self, x: i32, y: i32) {
        // A press on the retreated sliver is a summons, not a drag handle.
        if self.hidden {
            self.set_hidden(false);
            self.hide_at = None;
            return;
        }
        let (ux, uy) = (x as f64 / self.dpi, y as f64 / self.dpi);
        let count = self.entries.len();

        // A click on a ring refreshes that account — the ring is a button.
        let rail = (0.0, 0.0, self.window_units.0, self.window_units.1);
        let along = match self.edge {
            Edge::Top => ux - rail.0,
            _ => uy - rail.1,
        };
        if let Some(slot) = geometry::dock::slot_at(&self.m, along, self.edge.axis(), count) {
            if let Some(entry) = self.entries.get(slot) {
                let _ = self
                    .events
                    .send(PanelEvent::RefreshAccount(entry.account.clone()));
                // Immediate visible feedback; the store's answer lands
                // through the usual channel.
                if let Some(e) = self.entries.get_mut(slot) {
                    e.ring.is_refreshing = true;
                }
                self.redraw();
                return;
            }
        }

        // Anything else is the drag handle: the bar's padding claims the
        // press.
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

    /// Re-runs placement without resetting the arrival ease.
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
        let margin = (dock::EDGE_MARGIN * dpi_scale) as i32;

        let want_x = cursor.x - (gx * dpi_scale) as i32;
        let want_y = cursor.y - (gy * dpi_scale) as i32;

        // Fuse to a side **during** the drag, not on mouse-up. The edge is
        // decided from the cursor first, then the size is recomputed for
        // it, then the bar is placed against that size.
        let fuse = self.m.s(FUSE_DISTANCE) as i32;
        let dist_right = (work.right - margin - (want_x + phys_w)).abs();
        let dist_left = (want_x - margin - work.left).abs();
        let dist_top = (want_y - margin - work.top).abs();

        let new_edge = if dist_right <= fuse {
            Some(Edge::Right)
        } else if dist_left <= fuse {
            Some(Edge::Left)
        } else if dist_top <= fuse {
            Some(Edge::Top)
        } else {
            None
        };
        let new_docked = new_edge.is_some() || self.docked;
        let new_edge = new_edge.unwrap_or(self.edge);

        if new_edge != self.edge || new_docked != self.docked {
            self.edge = new_edge;
            self.docked = new_docked;
            self.compute_window_size();
        }
        let phys_w = (self.window_units.0 * dpi_scale) as i32;
        let phys_h = (self.window_units.1 * dpi_scale) as i32;

        let (nx, ny) = match (self.docked, self.edge) {
            (true, Edge::Right) => (
                work.right - phys_w - margin,
                work.top + (work.height() - phys_h) / 2,
            ),
            (true, Edge::Left) => (work.left + margin, work.top + (work.height() - phys_h) / 2),
            (true, Edge::Top) => {
                let x = want_x.clamp(
                    work.left + margin,
                    (work.right - phys_w - margin).max(work.left + margin),
                );
                (x, work.top + margin)
            }
            _ => {
                let x = want_x.clamp(
                    work.left + margin,
                    (work.right - phys_w - margin).max(work.left + margin),
                );
                let y = want_y.clamp(
                    work.top + margin,
                    (work.bottom - phys_h - margin).max(work.top + margin),
                );
                (x, y)
            }
        };

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
        // The window **is** the rail now, so its own rect is the position.
        let work_w = (work.width() - rect.width()).max(1) as f64;
        let work_h = (work.height() - rect.height()).max(1) as f64;
        let x = ((rect.left - work.left) as f64 / work_w).clamp(0.0, 1.0);
        let y = ((rect.top - work.top) as f64 / work_h).clamp(0.0, 1.0);

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
        let (enabled, current_display) =
            pulse_core::settings::with(|s| (s.follows_active_display, s.display.clone()));
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

/// One step of the damped spring the macOS app rides — SwiftUI's
/// `.spring(response:dampingFraction:)` in fixed-step form: ω from the
/// response, damping from the fraction, integrated semi-implicitly so the
/// bounce is stable at the timer's 30 ms.
fn spring_step(x: &mut f64, v: &mut f64, target: f64, response: f64, damping: f64, dt: f64) {
    let w = std::f64::consts::TAU / response;
    let c = 2.0 * damping * w;
    *v += (w * w * (target - *x) - c * *v) * dt;
    *x += *v * dt;
}

/// Whether a spring has come to rest on its target.
fn settled(x: f64, v: f64, target: f64) -> bool {
    (x - target).abs() < 0.002 && v.abs() < 0.01
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
    let monitor = unsafe {
        windows::Win32::Graphics::Gdi::MonitorFromPoint(
            POINT { x: 0, y: 0 },
            windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTOPRIMARY,
        )
    };
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
unsafe extern "system" fn panel_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
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
        // WM_MOUSELEAVE lives in UI::Controls in the windows metadata (and
        // must be named by path — a bare identifier starting with an
        // underscore, or any bare name, would bind instead of match).
        windows::Win32::UI::Controls::WM_MOUSELEAVE => {
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
            panel.tick();
            LRESULT(0)
        }
        // The flyout's pointer traffic, folded into the same linger logic.
        winutil::WM_APP_CARD => {
            panel.on_card_pointer(wparam.0 != 0);
            LRESULT(0)
        }
        // The system appearance changed: re-read it, re-tint the backdrops,
        // re-ink the rail (and the icons, whose tint bakes in at load).
        WM_SETTINGCHANGE => {
            if setting_change_name(lparam).as_deref() == Some("ImmersiveColorSet") {
                theme_panel::sync_theme();
                crate::assets::clear_cache();
                let dark = theme_panel::is_dark();
                winutil::apply_system_backdrop(hwnd, dark);
                if let Some(flyout) = panel.card.as_ref() {
                    winutil::apply_system_backdrop(flyout.hwnd, dark);
                }
                panel.redraw();
            }
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        // The frame is all client: no title bar, no resize borders — the
        // styles exist only so DWM treats the window as framed and paints
        // its backdrop, border and corners.
        WM_NCCALCSIZE if wparam.0 != 0 => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// WM_SETTINGCHANGE's LPARAM names what changed, as a wide string.
unsafe fn setting_change_name(lparam: LPARAM) -> Option<String> {
    let ptr = lparam.0 as *const u16;
    if ptr.is_null() {
        return None;
    }
    let mut len = 0usize;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    Some(String::from_utf16_lossy(std::slice::from_raw_parts(
        ptr, len,
    )))
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
