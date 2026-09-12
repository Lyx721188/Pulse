//! The Direct2D / DirectWrite engine: one factory pair for the process, a
//! flip-model swap-chain render target per window, and the small drawing
//! vocabulary the panel and its detail flyout share.
//!
//! Every window here is a real DWM backdrop window — Mica behind the whole
//! frame — so the render target must hand DWM premultiplied alpha to
//! composite over the backdrop with. That is what a flip-model swap chain on
//! a `WS_EX_NOREDIRECTIONBITMAP` window is for; the old layered-window
//! recipe cannot see a system backdrop at all.

use std::collections::HashMap;
use std::mem::ManuallyDrop;

use windows::core::{Interface, Result};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_BEZIER_SEGMENT, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED,
    D2D1_FIGURE_END_CLOSED, D2D1_FILL_MODE, D2D1_FILL_MODE_ALTERNATE, D2D1_GRADIENT_STOP,
    D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_F,
};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1, DXGI_PRESENT, DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows_numerics::{Matrix3x2, Vector2};

pub use crate::theme::Rgba;

pub struct D2DEngine {
    pub factory: ID2D1Factory,
    pub dwrite: IDWriteFactory,
    /// The one D2D device every window draws through — shared, so icon
    /// bitmaps and other device resources serve the dock and the flyout
    /// alike.
    pub device: ID2D1Device,
    /// The D3D device backing the D2D device; swap chains are created on it.
    pub d3d: ID3D11Device,
    /// The DXGI factory the swap chains are created on.
    pub dxgi_factory: IDXGIFactory2,
    pub round_stroke: ID2D1StrokeStyle,
    pub flat_stroke: ID2D1StrokeStyle,
    text_formats: std::sync::Mutex<HashMap<(u32, u32, u32, u32), IDWriteTextFormat>>,
}

/// A colour as D2D wants it.
pub fn color(c: Rgba) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

pub fn point(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

pub fn rect(x: f32, y: f32, w: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}

pub fn identity_matrix() -> Matrix3x2 {
    Matrix3x2::identity()
}

pub fn scale_matrix(sx: f32, sy: f32) -> Matrix3x2 {
    Matrix3x2::scale(sx, sy)
}

pub fn translation_matrix(dx: f32, dy: f32) -> Matrix3x2 {
    Matrix3x2::translation(dx, dy)
}

impl D2DEngine {
    pub fn new() -> Result<D2DEngine> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let factory1: ID2D1Factory1 = factory.cast()?;

            let round_props = D2D1_STROKE_STYLE_PROPERTIES1 {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_FLAT,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_SOLID,
                dashOffset: 0.0,
                transformType: D2D1_STROKE_TRANSFORM_TYPE_NORMAL,
            };
            let round_stroke: ID2D1StrokeStyle =
                factory1.CreateStrokeStyle(&round_props, None)?.cast()?;
            let flat_props = D2D1_STROKE_STYLE_PROPERTIES1 {
                startCap: D2D1_CAP_STYLE_FLAT,
                endCap: D2D1_CAP_STYLE_FLAT,
                ..round_props
            };
            let flat_stroke: ID2D1StrokeStyle =
                factory1.CreateStrokeStyle(&flat_props, None)?.cast()?;

            // The device every render target is created against.
            let (d3d_device, _context) = make_d3d_device()?;
            let dxgi_device: IDXGIDevice = d3d_device.cast()?;
            let device: ID2D1Device = factory1.CreateDevice(&dxgi_device)?;
            let adapter = dxgi_device.GetAdapter()?;
            let dxgi_factory: IDXGIFactory2 = adapter.GetParent()?;

            Ok(D2DEngine {
                factory,
                dwrite,
                d3d: d3d_device,
                device,
                dxgi_factory,
                round_stroke,
                flat_stroke,
                text_formats: std::sync::Mutex::new(HashMap::new()),
            })
        }
    }

    /// A text format at a pixel size, cached — the panel redraws the same
    /// handful of faces many times a second while animating.
    pub fn text_format(
        &self,
        size_px: f32,
        weight: DWRITE_FONT_WEIGHT,
        centered: bool,
        glyph_font: bool,
    ) -> Result<IDWriteTextFormat> {
        let key = (
            (size_px * 4.0).round() as u32,
            weight.0 as u32,
            centered as u32,
            glyph_font as u32,
        );
        if let Some(existing) = self.text_formats.lock().unwrap().get(&key) {
            return Ok(existing.clone());
        }
        unsafe {
            // Fluent's icons live in a private-use area the text face
            // does not carry; they need the icon font.
            let face = if glyph_font {
                windows::core::w!("Segoe Fluent Icons")
            } else {
                windows::core::w!("Segoe UI Variable Display")
            };
            let format = self.dwrite.CreateTextFormat(
                face,
                None::<&IDWriteFontCollection>,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size_px,
                windows::core::w!("en-US"),
            )?;
            if centered {
                let _ = format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
                let _ = format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            }
            self.text_formats
                .lock()
                .unwrap()
                .insert(key, format.clone());
            Ok(format)
        }
    }
}
/// A per-window canvas over a flip-model swap chain. The window carries
/// `WS_EX_NOREDIRECTIONBITMAP`, so DWM composites the swap chain's
/// premultiplied-alpha output straight over its backdrop — that is the
/// whole trick that lets transparent pixels be Mica and painted pixels be
/// ink.
pub struct SwapchainCanvas {
    #[allow(dead_code)]
    hwnd: HWND,
    swap_chain: IDXGISwapChain1,
    /// The composition hand-over. A plain HWND swap chain makes DWM treat
    /// the buffer as the window's final layer and skip the backdrop; a
    /// composition swap chain behind a DComp visual composites
    /// premultiplied alpha **over** the backdrop — the only route where
    /// Mica and transparency both survive.
    _dcomp: Option<(
        IDCompositionDevice,
        IDCompositionTarget,
        IDCompositionVisual,
    )>,
    pub rt: ID2D1DeviceContext,
    pub width: i32,
    pub height: i32,
}

impl SwapchainCanvas {
    pub fn new(hwnd: HWND, engine: &D2DEngine, width: i32, height: i32) -> Result<SwapchainCanvas> {
        unsafe {
            let dxgi_device: IDXGIDevice = engine.d3d.cast()?;
            let rt = engine
                .device
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let _ = rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let _ = rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let _ = rt.SetTransform(&identity_matrix());

            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width.max(1) as u32,
                Height: height.max(1) as u32,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                // Composition swap chains are the one kind where
                // premultiplied alpha is asked for by name.
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap_chain =
                engine
                    .dxgi_factory
                    .CreateSwapChainForComposition(&engine.d3d, &desc, None)?;

            // Hand the swap chain to DWM as a composition visual rooted on
            // this window — Windows Terminal's route to a transparent
            // surface over a system backdrop.
            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)?;
            let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = dcomp.CreateVisual()?;
            visual.SetContent(&swap_chain)?;
            target.SetRoot(&visual)?;
            dcomp.Commit()?;
            let dcomp = Some((dcomp, target, visual));

            let mut canvas = SwapchainCanvas {
                hwnd,
                swap_chain,
                _dcomp: dcomp,
                rt,
                width: width.max(1),
                height: height.max(1),
            };
            canvas.refresh_target()?;
            Ok(canvas)
        }
    }

    /// Resizes the swap chain and re-binds the back buffer. The window is
    /// only ever resized on a DPI change, an edge change, or a rail-length
    /// change.
    pub fn resize(&mut self, width: i32, height: i32) -> Result<()> {
        if width == self.width && height == self.height {
            return Ok(());
        }
        self.width = width.max(1);
        self.height = height.max(1);
        unsafe {
            // The old target holds a reference to a back buffer; it has to
            // be gone before the buffers can be resized.
            self.rt.SetTarget(None);
            self.swap_chain.ResizeBuffers(
                0,
                self.width as u32,
                self.height as u32,
                DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
            self.refresh_target()?;
        }
        Ok(())
    }

    /// Points the device context at the current back buffer.
    fn refresh_target(&mut self) -> Result<()> {
        unsafe {
            let surface: IDXGISurface = self.swap_chain.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: ManuallyDrop::new(None),
            };
            let bitmap = self
                .rt
                .CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            self.rt.SetTarget(&bitmap);
        }
        Ok(())
    }

    /// Clears to transparent — whatever is not drawn is backdrop.
    pub fn begin(&self) {
        unsafe {
            let _ = self.rt.BeginDraw();
            let _ = self.rt.Clear(None);
        }
    }

    /// Commits the frame to the screen.
    pub fn present(&self) {
        unsafe {
            let _ = self.rt.EndDraw(None, None);
            // Vsync'd: this surface changes a few times a minute at most,
            // and one frame of wait is nothing against tearing.
            let _ = self.swap_chain.Present(1, DXGI_PRESENT(0));
        }
    }
}

/// The D3D device the swap chain and D2D share, with the immediate context
/// it came with (D2D never touches it, but the API insists on handing it
/// back).
fn make_d3d_device() -> Result<(
    ID3D11Device,
    windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
)> {
    unsafe {
        let mut device = None;
        let mut context = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
        let device =
            device.ok_or_else(|| windows::core::Error::from_hresult(windows::core::HRESULT(-1)))?;
        let context = context
            .ok_or_else(|| windows::core::Error::from_hresult(windows::core::HRESULT(-1)))?;
        Ok((device, context))
    }
}
/// Helpers on top of a render target for the drawing the app actually does.
pub struct Painter<'a> {
    pub rt: &'a ID2D1RenderTarget,
    pub engine: &'a D2DEngine,
}

impl<'a> Painter<'a> {
    pub fn brush(&self, c: Rgba) -> Result<ID2D1SolidColorBrush> {
        unsafe { self.rt.CreateSolidColorBrush(&color(c), None) }
    }

    pub fn fill_geometry(&self, geometry: &ID2D1PathGeometry, brush: &ID2D1SolidColorBrush) {
        unsafe {
            let _ = self.rt.FillGeometry(geometry, brush, None::<&ID2D1Brush>);
        }
    }

    pub fn draw_geometry(
        &self,
        geometry: &ID2D1PathGeometry,
        brush: &ID2D1SolidColorBrush,
        width: f32,
        round_caps: bool,
    ) {
        unsafe {
            let style = if round_caps {
                &self.engine.round_stroke
            } else {
                &self.engine.flat_stroke
            };
            let _ = self.rt.DrawGeometry(geometry, brush, width, Some(style));
        }
    }

    pub fn fill_rounded_rect(&self, r: D2D_RECT_F, radius: f32, brush: &ID2D1SolidColorBrush) {
        if let Ok(geometry) = rounded_rect_geometry(self.engine, r, radius) {
            self.fill_geometry(&geometry, brush);
        }
    }

    pub fn draw_rounded_rect(
        &self,
        r: D2D_RECT_F,
        radius: f32,
        brush: &ID2D1SolidColorBrush,
        stroke: f32,
    ) {
        if let Ok(geometry) = rounded_rect_geometry(self.engine, r, radius) {
            self.draw_geometry(&geometry, brush, stroke, false);
        }
    }

    pub fn fill_ellipse(&self, center: Vector2, radius: f32, brush: &ID2D1SolidColorBrush) {
        unsafe {
            let _ = self.rt.FillEllipse(
                &D2D1_ELLIPSE {
                    point: center,
                    radiusX: radius,
                    radiusY: radius,
                },
                brush,
            );
        }
    }

    pub fn draw_line(&self, from: Vector2, to: Vector2, brush: &ID2D1SolidColorBrush, width: f32) {
        unsafe {
            let _ = self
                .rt
                .DrawLine(from, to, brush, width, Some(&self.engine.round_stroke));
        }
    }

    pub fn fill_rect(&self, r: D2D_RECT_F, brush: &ID2D1SolidColorBrush) {
        unsafe {
            let _ = self.rt.FillRectangle(&r, brush);
        }
    }

    /// A radial gradient — the hover halo, which on the macOS side is a
    /// shadow on the arc masked inward. Here it is a soft disc beneath the
    /// ring, which on a solid surface reads the same and has no colour to
    /// get wrong.
    pub fn draw_halo(&self, center: Vector2, outer: f32, c: Rgba) -> Result<()> {
        unsafe {
            // The caller's alpha is the strength; the falloff shape is
            // fixed here.
            let stops = [
                D2D1_GRADIENT_STOP {
                    position: 0.0,
                    color: color(c.with_alpha(c.a * 0.38)),
                },
                D2D1_GRADIENT_STOP {
                    position: 1.0,
                    color: color(c.with_alpha(0.0)),
                },
            ];
            let stop_collection = self.rt.CreateGradientStopCollection(
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )?;
            let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                center,
                gradientOriginOffset: point(0.0, 0.0),
                radiusX: outer,
                radiusY: outer,
            };
            let brush: ID2D1RadialGradientBrush =
                self.rt
                    .CreateRadialGradientBrush(&props, None, &stop_collection)?;
            let _ = self.rt.FillEllipse(
                &D2D1_ELLIPSE {
                    point: center,
                    radiusX: outer,
                    radiusY: outer,
                },
                &brush,
            );
            Ok(())
        }
    }

    /// Draws text into a rect. Alignment: 0 leading, 1 center, 2 trailing;
    /// vertical 0 top, 1 center.
    pub fn text(
        &self,
        text: &str,
        r: D2D_RECT_F,
        size_px: f32,
        weight: DWRITE_FONT_WEIGHT,
        brush: &ID2D1SolidColorBrush,
        halign: u32,
        valign: u32,
    ) {
        let wide: Vec<u16> = text.encode_utf16().collect();
        if wide.is_empty() {
            return;
        }
        // Private-use scalars are Fluent icon glyphs, not text — the
        // tofu boxes people see are what happens when they are drawn
        // with a face that does not carry them.
        let glyph_font = text.chars().any(|c| ('\u{E700}'..='\u{F8FF}').contains(&c));
        let Ok(format) =
            self.engine
                .text_format(size_px, weight, halign == 1 && valign == 1, glyph_font)
        else {
            return;
        };
        unsafe {
            if !(halign == 1 && valign == 1) {
                let _ = format.SetTextAlignment(match halign {
                    1 => DWRITE_TEXT_ALIGNMENT_CENTER,
                    2 => DWRITE_TEXT_ALIGNMENT_TRAILING,
                    _ => DWRITE_TEXT_ALIGNMENT_LEADING,
                });
                let _ = format.SetParagraphAlignment(match valign {
                    1 => DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                    2 => DWRITE_PARAGRAPH_ALIGNMENT_FAR,
                    _ => DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
                });
            }
            let _ = self.rt.DrawText(
                &wide,
                &format,
                &r,
                brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
}

/// Opens a path geometry for building. The sink closes on `finish` or on
/// drop, so an abandoned builder cannot poison the factory.
pub struct PathBuilder {
    geometry: ID2D1PathGeometry,
    sink: Option<ID2D1GeometrySink>,
    open: bool,
}

impl PathBuilder {
    pub fn new(engine: &D2DEngine, fill_mode: D2D1_FILL_MODE) -> Result<PathBuilder> {
        unsafe {
            let geometry = engine.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            let _ = sink.SetFillMode(fill_mode);
            Ok(PathBuilder {
                geometry,
                sink: Some(sink),
                open: false,
            })
        }
    }

    pub fn begin_at(&mut self, x: f64, y: f64) {
        self.end_closed();
        if let Some(sink) = &self.sink {
            unsafe {
                sink.BeginFigure(point(x as f32, y as f32), D2D1_FIGURE_BEGIN_FILLED);
            }
        }
        self.open = true;
    }

    pub fn line(&mut self, x: f64, y: f64) {
        if let Some(sink) = &self.sink {
            unsafe {
                sink.AddLine(point(x as f32, y as f32));
            }
        }
    }

    /// A cubic bezier from the current point, with two control points.
    pub fn curve(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
        if let Some(sink) = &self.sink {
            unsafe {
                sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                    point1: point(c1x as f32, c1y as f32),
                    point2: point(c2x as f32, c2y as f32),
                    point3: point(x as f32, y as f32),
                });
            }
        }
    }

    /// An arc to `to`, along a circle of `radius` — D2D picks the centre
    /// from the two endpoints. `large` when the sweep exceeds half a
    /// circle.
    pub fn arc_to(&mut self, to: Vector2, radius: f32, large: bool, clockwise: bool) {
        if let Some(sink) = &self.sink {
            unsafe {
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: to,
                    size: D2D_SIZE_F {
                        width: radius,
                        height: radius,
                    },
                    rotationAngle: 0.0,
                    sweepDirection: if clockwise {
                        D2D1_SWEEP_DIRECTION_CLOCKWISE
                    } else {
                        D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE
                    },
                    arcSize: if large {
                        D2D1_ARC_SIZE_LARGE
                    } else {
                        D2D1_ARC_SIZE_SMALL
                    },
                });
            }
        }
    }

    pub fn end_closed(&mut self) {
        if self.open {
            if let Some(sink) = &self.sink {
                unsafe {
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                }
            }
            self.open = false;
        }
    }

    pub fn finish(&mut self) -> Result<ID2D1PathGeometry> {
        self.end_closed();
        if let Some(sink) = self.sink.take() {
            unsafe {
                sink.Close()?;
            }
        }
        Ok(self.geometry.clone())
    }
}

impl Drop for PathBuilder {
    fn drop(&mut self) {
        self.end_closed();
        if let Some(sink) = self.sink.take() {
            unsafe {
                let _ = sink.Close();
            }
        }
    }
}

/// A filled rounded rectangle's geometry.
pub fn rounded_rect_geometry(
    engine: &D2DEngine,
    r: D2D_RECT_F,
    radius: f32,
) -> Result<ID2D1PathGeometry> {
    let radius = radius
        .min((r.right - r.left) / 2.0)
        .min((r.bottom - r.top) / 2.0);
    // The kappa constant turns a circular corner into two beziers.
    let k = radius * 0.5523;
    let mut builder = PathBuilder::new(engine, D2D1_FILL_MODE_ALTERNATE)?;
    builder.begin_at((r.left + radius) as f64, r.top as f64);
    builder.line((r.right - radius) as f64, r.top as f64);
    builder.curve(
        (r.right - radius + k) as f64,
        r.top as f64,
        r.right as f64,
        (r.top + radius - k) as f64,
        r.right as f64,
        (r.top + radius) as f64,
    );
    builder.line(r.right as f64, (r.bottom - radius) as f64);
    builder.curve(
        r.right as f64,
        (r.bottom - radius + k) as f64,
        (r.right - radius + k) as f64,
        r.bottom as f64,
        (r.right - radius) as f64,
        r.bottom as f64,
    );
    builder.line((r.left + radius) as f64, r.bottom as f64);
    builder.curve(
        (r.left + radius - k) as f64,
        r.bottom as f64,
        r.left as f64,
        (r.bottom - radius + k) as f64,
        r.left as f64,
        (r.bottom - radius) as f64,
    );
    builder.line(r.left as f64, (r.top + radius) as f64);
    builder.curve(
        r.left as f64,
        (r.top + radius - k) as f64,
        (r.left + radius - k) as f64,
        r.top as f64,
        (r.left + radius) as f64,
        r.top as f64,
    );
    builder.end_closed();
    builder.finish()
}

/// A progress arc from 12 o'clock clockwise. Empty sweeps draw nothing.
pub fn arc_geometry(
    engine: &D2DEngine,
    center: Vector2,
    radius: f32,
    fraction: f64,
) -> Result<Option<ID2D1PathGeometry>> {
    let fraction = fraction.clamp(0.0, 1.0);
    if fraction <= 0.0 {
        return Ok(None);
    }
    let full = fraction >= 1.0;
    let mut builder = PathBuilder::new(engine, D2D1_FILL_MODE_ALTERNATE)?;
    let (cx, cy) = (center.X, center.Y);
    let start = (cx, cy - radius);
    if full {
        // A full circle as one closed figure of two arcs: an exact-circle
        // stroke, not two nearly-halves with a seam.
        builder.begin_at(start.0 as f64, start.1 as f64);
        builder.arc_to(point(cx, cy + radius), radius, false, true);
        builder.arc_to(point(start.0, start.1), radius, false, true);
        builder.end_closed();
    } else {
        let angle = fraction * std::f64::consts::TAU;
        builder.begin_at(start.0 as f64, start.1 as f64);
        builder.arc_to(
            point(
                cx + angle.sin() as f32 * radius,
                cy - angle.cos() as f32 * radius,
            ),
            radius,
            angle > std::f64::consts::PI,
            true,
        );
    }
    Ok(Some(builder.finish()?))
}

/// A circular track: always full, always stroked, never animated.
pub fn circle_geometry(
    engine: &D2DEngine,
    center: Vector2,
    radius: f32,
) -> Result<ID2D1PathGeometry> {
    let mut builder = PathBuilder::new(engine, D2D1_FILL_MODE_ALTERNATE)?;
    let start = point(center.X, center.Y - radius);
    builder.begin_at(start.X as f64, start.Y as f64);
    builder.arc_to(point(center.X, center.Y + radius), radius, false, true);
    builder.arc_to(start, radius, false, true);
    builder.end_closed();
    builder.finish()
}

/// An arbitrary sweep of a circle, from `start_deg` to `start_deg +
/// sweep_deg` measured clockwise from 12 o'clock. Used by the travelling
/// busy/refresh marks.
pub fn arc_sweep_geometry(
    engine: &D2DEngine,
    center: Vector2,
    radius: f32,
    start_deg: f64,
    sweep_deg: f64,
) -> Result<Option<ID2D1PathGeometry>> {
    if sweep_deg <= 0.0 {
        return Ok(None);
    }
    let rad = |deg: f64| deg * std::f64::consts::PI / 180.0;
    let at = |deg: f64| {
        point(
            center.X + rad(deg).sin() as f32 * radius,
            center.Y - rad(deg).cos() as f32 * radius,
        )
    };
    let mut builder = PathBuilder::new(engine, D2D1_FILL_MODE_ALTERNATE)?;
    let start = at(start_deg);
    builder.begin_at(start.X as f64, start.Y as f64);
    builder.arc_to(at(start_deg + sweep_deg), radius, sweep_deg > 180.0, true);
    Ok(Some(builder.finish()?))
}

static GLOBAL_ENGINE: std::sync::OnceLock<D2DEngine> = std::sync::OnceLock::new();

/// The process-wide engine. Initialized in `main` before any window exists,
/// so every later call can borrow it.
pub fn global_engine() -> &'static D2DEngine {
    GLOBAL_ENGINE.get_or_init(|| D2DEngine::new().expect("Direct2D initialization"))
}
