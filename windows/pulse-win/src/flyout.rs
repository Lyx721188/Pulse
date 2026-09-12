//! The detail flyout: the hover card as its own topmost window, the way a
//! WinUI flyout is its own surface rather than a layer of the window it
//! points at.
//!
//! It exists as a separate window because the dock bar is real Mica, and a
//! DWM backdrop fills the whole window rectangle — the only way to have
//! Mica on the bar and more Mica on the card without a slab of it between
//! is to make the card a window of its own. The card draws no surface of
//! its own; its backdrop is the flyout's.
//!
//! The panel owns the flyout's lifetime; the flyout only reports pointer
//! traffic back through `WM_APP_CARD`, so the panel can decide when the
//! card ought to go away.

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::card::{draw_card, CardData};
use crate::d2d::{global_engine, scale_matrix, SwapchainCanvas};
use crate::geometry::Metrics;
use crate::theme::panel;
use crate::winutil::{self, WM_APP_CARD};

pub struct Flyout {
    pub hwnd: HWND,
    canvas: Option<SwapchainCanvas>,
    /// Whether the pointer is over the card right now — the panel checks
    /// it before hiding the card on a bar mouse-leave.
    pub pointer_inside: bool,
    bar: HWND,
    dpi: f64,
    units: (f64, f64),
    tracking_mouse: bool,
}

impl Flyout {
    /// Builds the flyout. Returns a box because the window procedure holds
    /// a pointer into it from the first message onwards.
    pub fn new(bar: HWND) -> Box<Flyout> {
        let class_name = w!("PulseFlyout");
        unsafe {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(flyout_wndproc),
                lpszClassName: class_name,
                hInstance: winutil::hinstance(),
                hCursor: winutil::arrow_cursor(),
                ..Default::default()
            };
            RegisterClassW(&wc);
        }

        let mut flyout = Box::new(Flyout {
            hwnd: HWND::default(),
            canvas: None,
            pointer_inside: false,
            bar,
            dpi: 1.0,
            units: (0.0, 0.0),
            tracking_mouse: false,
        });
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                class_name,
                w!("Pulse"),
                // Framed enough for DWM to paint its backdrop on; the
                // frame itself is swallowed by WM_NCCALCSIZE below.
                WS_CAPTION | WS_THICKFRAME,
                0,
                0,
                10,
                10,
                None,
                None,
                Some(winutil::hinstance()),
                None,
            )
            .expect("CreateWindowExW for flyout");
            flyout.hwnd = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &*flyout as *const Flyout as isize);
            winutil::apply_system_backdrop(hwnd, panel::is_dark());
        }
        flyout
    }

    /// Moves, sizes, shows. `units` is the card's design size; the canvas
    /// is kept in step with it and the monitor's DPI.
    pub fn show_at(&mut self, x: i32, y: i32, units: (f64, f64), dpi: f64) {
        let px_w = (units.0 * dpi).ceil() as i32;
        let px_h = (units.1 * dpi).ceil() as i32;
        let resized = self.units != units || self.dpi != dpi;
        if self.canvas.is_none() {
            self.canvas = Some(
                SwapchainCanvas::new(self.hwnd, global_engine(), px_w.max(1), px_h.max(1))
                    .expect("flyout canvas"),
            );
        } else if resized {
            self.canvas
                .as_mut()
                .expect("flyout canvas")
                .resize(px_w.max(1), px_h.max(1))
                .expect("flyout canvas resize");
        }
        self.units = units;
        self.dpi = dpi;
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                px_w.max(1),
                px_h.max(1),
                SWP_NOACTIVATE,
            );
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }

    pub fn hide(&mut self) {
        if self.is_shown() {
            self.pointer_inside = false;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    /// Slides the window without touching its size — the panel's spring
    /// calls this while the card glides between rings.
    pub fn move_to(&mut self, x: i32, y: i32) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOSIZE,
            );
        }
    }

    pub fn is_shown(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd).as_bool() }
    }

    /// Draws the card's content over its Mica. `alpha` is the entrance
    /// fade, driven by the panel's spring.
    pub fn draw(&mut self, data: &CardData, m: &Metrics, alpha: f32) {
        let Some(canvas) = self.canvas.as_ref() else {
            return;
        };
        canvas.begin();
        unsafe {
            let _ = canvas
                .rt
                .SetTransform(&scale_matrix(self.dpi as f32, self.dpi as f32));
        }
        let painter = crate::d2d::Painter {
            rt: &canvas.rt,
            engine: global_engine(),
        };
        let _ = draw_card(&painter, m, (0.0, 0.0), data, alpha);
        canvas.present();
    }
}

unsafe extern "system" fn flyout_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    if state == 0 {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let flyout = &mut *(state as *mut Flyout);

    match msg {
        WM_MOUSEMOVE => {
            flyout.pointer_inside = true;
            flyout.track_mouse();
            // The panel owns the show/hide decision.
            let _ = SendMessageW(flyout.bar, WM_APP_CARD, Some(WPARAM(1)), Some(LPARAM(0)));
            LRESULT(0)
        }
        // WM_MOUSELEAVE lives in UI::Controls in the windows metadata.
        windows::Win32::UI::Controls::WM_MOUSELEAVE => {
            flyout.pointer_inside = false;
            flyout.tracking_mouse = false;
            let _ = SendMessageW(flyout.bar, WM_APP_CARD, Some(WPARAM(0)), Some(LPARAM(0)));
            LRESULT(0)
        }
        // The card is read-only, but it still takes the click — a press on
        // the card must not fall through to whatever sits beneath it.
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK => LRESULT(0),
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        // All frame, all client — see the panel's proc.
        WM_NCCALCSIZE if wparam.0 != 0 => LRESULT(0),
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

impl Flyout {
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
}
