//! The provider icons: the same Lobe Icons SVG set the macOS app ships,
//! rasterised once into transparent white PNGs and tinted at load into the
//! theme's ink — the template-image arrangement, without the template.
//!
//! The white pixels are a stand-in: only each PNG's alpha channel carries
//! shape, and the decode pass rewrites RGB to the current theme's text
//! colour (premultiplied), so the dark theme draws white marks and the
//! light theme black ones. A theme change clears the cache and everything
//! re-tints on the next frame.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use windows::core::Interface;

use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Bitmap, ID2D1DeviceContext, D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPRGBA, IWICImagingFactory,
    WICBitmapCacheOnDemand, WICBitmapDitherTypeNone, WICBitmapLockRead, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

use crate::d2d::D2DEngine;
use crate::theme::panel;

const ANTIGRAVITY_PNG: &[u8] = include_bytes!("../assets/icons/antigravity.png");
const CLAUDE_PNG: &[u8] = include_bytes!("../assets/icons/claude.png");
const COMMANDCODE_PNG: &[u8] = include_bytes!("../assets/icons/commandcode.png");
const CURSOR_PNG: &[u8] = include_bytes!("../assets/icons/cursor.png");
const DEEPSEEK_PNG: &[u8] = include_bytes!("../assets/icons/deepseek.png");
const GITHUB_PNG: &[u8] = include_bytes!("../assets/icons/github.png");
const GROK_PNG: &[u8] = include_bytes!("../assets/icons/grok.png");
const KIMI_PNG: &[u8] = include_bytes!("../assets/icons/kimi.png");
const MINIMAX_PNG: &[u8] = include_bytes!("../assets/icons/minimax.png");
const OLLAMA_PNG: &[u8] = include_bytes!("../assets/icons/ollama.png");
const OPENAI_PNG: &[u8] = include_bytes!("../assets/icons/openai.png");
const OPENCODE_PNG: &[u8] = include_bytes!("../assets/icons/opencode.png");
const QINGYAN_PNG: &[u8] = include_bytes!("../assets/icons/qingyan.png");
const VOLCENGINE_PNG: &[u8] = include_bytes!("../assets/icons/volcengine.png");
const XAI_PNG: &[u8] = include_bytes!("../assets/icons/xai.png");
const ZAI_PNG: &[u8] = include_bytes!("../assets/icons/zai.png");

/// The parent brand's mark per provider — the same mapping as the macOS
/// app's `iconResource`.
fn png_for(provider: &str) -> Option<&'static [u8]> {
    match provider {
        "claudeCode" => Some(CLAUDE_PNG),
        "codex" => Some(OPENAI_PNG),
        "antigravity" => Some(ANTIGRAVITY_PNG),
        "cursor" => Some(CURSOR_PNG),
        "openCodeGo" => Some(OPENCODE_PNG),
        "kimiCode" => Some(KIMI_PNG),
        "ollamaCloud" => Some(OLLAMA_PNG),
        "zai" => Some(ZAI_PNG),
        "glmCoding" => Some(QINGYAN_PNG),
        "minimax" | "minimaxCN" => Some(MINIMAX_PNG),
        "copilot" => Some(GITHUB_PNG),
        "grok" => Some(GROK_PNG),
        "grokBot" => Some(XAI_PNG),
        "volcengine" => Some(VOLCENGINE_PNG),
        "commandCode" => Some(COMMANDCODE_PNG),
        "deepSeek" => Some(DEEPSEEK_PNG),
        _ => None,
    }
}

static CACHE: OnceLock<Mutex<HashMap<String, ID2D1Bitmap>>> = OnceLock::new();

/// A resource context on the shared D2D device, created once: bitmaps made
/// here serve every window that draws through the device.
fn resource_context(engine: &D2DEngine) -> ID2D1DeviceContext {
    static CTX: OnceLock<ID2D1DeviceContext> = OnceLock::new();
    CTX.get_or_init(|| unsafe {
        engine
            .device
            .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
            .expect("resource device context")
    })
    .clone()
}

/// The provider's mark as a bitmap in the current theme's ink. Bitmaps
/// belong to the shared D2D device, so one cache serves the dock and the
/// flyout alike.
pub fn bitmap_for(engine: &D2DEngine, provider: &str) -> Option<ID2D1Bitmap> {
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap();
    if let Some(existing) = cache.get(provider) {
        return Some(existing.clone());
    }
    let (bitmap, _size) = decode_tinted(
        &resource_context(engine),
        png_for(provider)?,
        panel::palette().text_primary,
    )?;
    cache.insert(provider.to_string(), bitmap.clone());
    Some(bitmap)
}

/// A theme change re-inks everything: drop the cache, the next frame
/// reloads.
pub fn clear_cache() {
    if let Some(cache) = CACHE.get() {
        cache.lock().unwrap().clear();
    }
}

/// Decodes a white PNG and rewrites its RGB to `ink`, keeping the alpha —
/// the premultiplied output D2D expects, in BGRA byte order.
fn decode_tinted(
    rt: &ID2D1DeviceContext,
    png: &[u8],
    ink: crate::d2d::Rgba,
) -> Option<(ID2D1Bitmap, (u32, u32))> {
    ensure_com();
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let stream = factory.CreateStream().ok()?;
        stream.InitializeFromMemory(png).ok()?;
        let decoder = factory
            .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let converter = factory.CreateFormatConverter().ok()?;
        converter
            .Initialize(
                &frame,
                &GUID_WICPixelFormat32bppPRGBA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .ok()?;
        let source = factory
            .CreateBitmapFromSource(&converter, WICBitmapCacheOnDemand)
            .ok()?;
        let mut w = 0u32;
        let mut h = 0u32;
        source.GetSize(&mut w, &mut h).ok()?;
        let lock = source
            .Lock(std::ptr::null(), WICBitmapLockRead.0 as u32)
            .ok()?;
        let mut stride = lock.GetStride().ok()?;
        let mut size = 0u32;
        let mut data: *mut u8 = std::ptr::null_mut();
        lock.GetDataPointer(&mut size, &mut data).ok()?;
        let rows = std::slice::from_raw_parts(data, size as usize);

        // The source is straight-alpha white; the destination is
        // premultiplied BGRA in the ink colour.
        let mut out = vec![0u8; (w * h * 4) as usize];
        for y in 0..h as usize {
            let row = &rows[y * stride as usize..];
            for x in 0..w as usize {
                let i = x * 4 + 3;
                let a = row[i];
                let o = (y * w as usize + x) * 4;
                out[o] = (ink.b * a as f32) as u8;
                out[o + 1] = (ink.g * a as f32) as u8;
                out[o + 2] = (ink.r * a as f32) as u8;
                out[o + 3] = a;
            }
        }

        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let bitmap1 = rt
            .CreateBitmap(
                windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_U {
                    width: w,
                    height: h,
                },
                Some(out.as_ptr() as *const core::ffi::c_void),
                w * 4,
                &props,
            )
            .ok()?;
        let bitmap: ID2D1Bitmap = bitmap1.cast().ok()?;
        Some((bitmap, (w, h)))
    }
}

/// WIC needs COM; one init is enough and a changed-mode error means
/// somebody got there first, which is fine.
fn ensure_com() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    });
}
