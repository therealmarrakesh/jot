//! Drawing and text, over Direct2D, DirectWrite, DXGI and DirectComposition.
//!
//! The `Gpu` holds the devices every window draws with, and replaces them when they're lost.
//! A `WindowSurface` is a window and the swap chain it's composed from; drawing it hands out a
//! `Canvas`, which paints quads, shadows and text within nested scopes. The `TextSystem` shapes
//! text: single rows for the editor, multi-line labels for the UI, and a `FontFace` per style
//! for measuring, wrapping and character advances.

#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::Hash;
use std::ops::{Range, RangeInclusive};
use std::rc::Rc;

use windows::Win32::{
    Foundation::{D2DERR_RECREATE_TARGET, HWND},
    Graphics::{
        Direct2D::{
            CLSID_D2D1Shadow,
            Common::{
                D2D_RECT_F, D2D_SIZE_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
                D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
                D2D1_PIXEL_FORMAT,
            },
            D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_SMALL,
            D2D1_BITMAP_OPTIONS, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET,
            D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
            D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT, D2D1_FACTORY_OPTIONS,
            D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_INTERPOLATION_MODE_LINEAR,
            D2D1_LAYER_OPTIONS1_NONE, D2D1_LAYER_PARAMETERS1, D2D1_PROPERTY_TYPE_UNKNOWN,
            D2D1_ROUNDED_RECT, D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION, D2D1_SHADOW_PROP_COLOR,
            D2D1_SWEEP_DIRECTION_CLOCKWISE, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory,
            ID2D1Bitmap1, ID2D1Device, ID2D1DeviceContext, ID2D1Effect, ID2D1Factory1,
            ID2D1Geometry, ID2D1SolidColorBrush,
        },
        Direct3D::{
            D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_11_1,
        },
        Direct3D11::{
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
        },
        DirectComposition::{
            DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
        },
        DirectWrite::{
            DWRITE_BREAK_CONDITION_CAN_BREAK, DWRITE_BREAK_CONDITION_MAY_NOT_BREAK,
            DWRITE_BREAK_CONDITION_MUST_BREAK, DWRITE_FACTORY_TYPE_SHARED,
            DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
            DWRITE_HIT_TEST_METRICS, DWRITE_LINE_BREAKPOINT, DWRITE_LINE_METRICS,
            DWRITE_LINE_SPACING_METHOD_UNIFORM, DWRITE_READING_DIRECTION,
            DWRITE_READING_DIRECTION_LEFT_TO_RIGHT, DWRITE_SCRIPT_ANALYSIS, DWRITE_TEXT_METRICS,
            DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
            DWRITE_WORD_WRAPPING_WRAP, DWriteCreateFactory, IDWriteFactory5,
            IDWriteNumberSubstitution, IDWriteTextAnalysisSink, IDWriteTextAnalysisSink_Impl,
            IDWriteTextAnalysisSource, IDWriteTextAnalysisSource_Impl, IDWriteTextAnalyzer,
            IDWriteTextFormat, IDWriteTextLayout,
        },
        Dxgi::{
            Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
            CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_ERROR_DEVICE_REMOVED,
            DXGI_ERROR_DEVICE_RESET, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
            DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory6, IDXGISurface,
            IDXGISwapChain1,
        },
    },
};
use windows::core::{BOOL, HRESULT, HSTRING, Interface, implement};
use windows_numerics::{Matrix3x2, Vector2};

use crate::geom::{
    Corners, Edges, Point, Rect, Rgba, Shadows, Size, black, point, size, transparent_black, white,
};
use crate::platform::NativeWindow;
use crate::types::{Error, Result, SharedStr};

// ---- 1. Win32 helpers -----------------------------------------------------------------------

fn d2d_color(c: Rgba) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

fn d2d_rect(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.left(),
        top: r.top(),
        right: r.right(),
        bottom: r.bottom(),
    }
}

/// Bitmap properties for a premultiplied BGRA bitmap at `scale`.
fn bitmap_props(scale: f32, options: D2D1_BITMAP_OPTIONS) -> D2D1_BITMAP_PROPERTIES1 {
    let dpi = 96.0 * scale;
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: dpi,
        dpiY: dpi,
        bitmapOptions: options,
        ..Default::default()
    }
}

// ---- 2. The GPU -----------------------------------------------------------------------------

/// The Direct3D, DXGI and Direct2D devices everything is drawn with.
#[derive(Clone)]
struct Devices {
    dxgi_factory: IDXGIFactory6,
    d3d_device: ID3D11Device,
    dxgi_device: IDXGIDevice,
    d2d_factory: ID2D1Factory1,
    d2d_device: ID2D1Device,
}

impl Devices {
    /// On the hardware adapter, or on WARP if there's none.
    fn new() -> Result<Self> {
        let dxgi_factory: IDXGIFactory6 =
            unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };
        let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let mut d3d_device: Option<ID3D11Device> = None;
        let mut created = Ok(());
        for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
            created = unsafe {
                D3D11CreateDevice(
                    None,
                    driver,
                    windows::Win32::Foundation::HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&feature_levels),
                    D3D11_SDK_VERSION,
                    Some(&mut d3d_device),
                    None,
                    None,
                )
            };
            if created.is_ok() {
                break;
            }
        }
        created?;
        let d3d_device =
            d3d_device.ok_or_else(|| Error::new("D3D11CreateDevice returned null device"))?;
        let dxgi_device: IDXGIDevice = d3d_device.cast()?;
        let options = D2D1_FACTORY_OPTIONS::default();
        let d2d_factory: ID2D1Factory1 =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&options))? };
        let d2d_device = unsafe { d2d_factory.CreateDevice(&dxgi_device)? };
        Ok(Self {
            dxgi_factory,
            d3d_device,
            dxgi_device,
            d2d_factory,
            d2d_device,
        })
    }

    /// Whether `hr` (or the device itself) says the device is gone.
    fn lost(&self, hr: HRESULT) -> bool {
        hr == D2DERR_RECREATE_TARGET
            || hr == DXGI_ERROR_DEVICE_REMOVED
            || hr == DXGI_ERROR_DEVICE_RESET
            || unsafe { self.d3d_device.GetDeviceRemovedReason() }.is_err()
    }
}

/// The devices every window draws with. When they're lost they're replaced, and the
/// generation counts up, so each surface knows to rebuild what it made on the old ones.
pub(crate) struct Gpu {
    devices: RefCell<Devices>,
    generation: Cell<u64>,
}

impl Gpu {
    pub(crate) fn new() -> Result<Rc<Self>> {
        let devices = Devices::new().map_err(|err| {
            Error::new(format!("initializing Direct3D11/Direct2D devices: {err}"))
        })?;
        Ok(Rc::new(Self {
            devices: RefCell::new(devices),
            generation: Cell::new(0),
        }))
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// The current devices and their generation.
    fn devices(&self) -> (u64, Devices) {
        (self.generation.get(), self.devices.borrow().clone())
    }

    /// Replaces the devices lost at generation `lost` (unless someone already has).
    fn recover(&self, lost: u64) -> Result<()> {
        if self.generation.get() != lost {
            return Ok(());
        }
        *self.devices.borrow_mut() = Devices::new()?;
        self.generation.set(lost.wrapping_add(1));
        Ok(())
    }
}

// ---- 3. Window surfaces ---------------------------------------------------------------------

const SWAPCHAIN_BUFFERS: u32 = 2;

/// A swap chain composed into a window, and the canvas drawing into its back buffer.
struct Target {
    generation: u64,
    /// The swap chain's size in pixels, and the scale it's drawn at.
    size: (u32, u32),
    scale: f32,
    swap_chain: IDXGISwapChain1,
    _comp_device: IDCompositionDevice,
    _comp_target: IDCompositionTarget,
    _comp_visual: IDCompositionVisual,
    canvas: Canvas,
}

impl Target {
    fn new(
        devices: Devices,
        generation: u64,
        hwnd: HWND,
        size: (u32, u32),
        scale: f32,
    ) -> Result<Self> {
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: size.0,
            Height: size.1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: SWAPCHAIN_BUFFERS,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        let (swap_chain, comp_device, comp_target, comp_visual) = unsafe {
            let factory = &devices.dxgi_factory;
            let swap_chain =
                factory.CreateSwapChainForComposition(&devices.d3d_device, &desc, None)?;
            let comp_device: IDCompositionDevice = DCompositionCreateDevice(&devices.dxgi_device)?;
            let comp_target = comp_device.CreateTargetForHwnd(hwnd, true)?;
            let comp_visual = comp_device.CreateVisual()?;
            comp_visual.SetContent(&swap_chain)?;
            comp_target.SetRoot(&comp_visual)?;
            comp_device.Commit()?;
            (swap_chain, comp_device, comp_target, comp_visual)
        };
        let mut target = Self {
            generation,
            size,
            scale,
            swap_chain,
            _comp_device: comp_device,
            _comp_target: comp_target,
            _comp_visual: comp_visual,
            canvas: Canvas::new(devices, scale)?,
        };
        target.attach_backbuffer()?;
        Ok(target)
    }

    /// Draws into the swap chain's back buffer (the context's target keeps it alive).
    fn attach_backbuffer(&mut self) -> Result<()> {
        let dpi = 96.0 * self.scale;
        let props = bitmap_props(
            self.scale,
            D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        );
        unsafe {
            let surface: IDXGISurface = self.swap_chain.GetBuffer(0)?;
            let context = &self.canvas.context;
            let bitmap = context.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            context.SetDpi(dpi, dpi);
            context.SetTarget(&bitmap);
        }
        Ok(())
    }

    fn resize(&mut self, size: (u32, u32), scale: f32) -> Result<()> {
        unsafe {
            self.canvas.context.SetTarget(None);
            self.swap_chain.ResizeBuffers(
                SWAPCHAIN_BUFFERS,
                size.0,
                size.1,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
        }
        (self.size, self.scale) = (size, scale);
        self.attach_backbuffer()?;
        // Shadows are rendered at a scale; another one needs them again.
        if (scale - self.canvas.scale).abs() >= f32::EPSILON {
            self.canvas.shadows.clear();
        }
        self.canvas.scale = scale;
        Ok(())
    }

    fn begin_frame(&mut self, clear: Rgba) {
        self.canvas.shadows.advance_frame();
        self.canvas.transform = Matrix3x2::identity();
        unsafe {
            self.canvas.context.BeginDraw();
            self.canvas.context.SetTransform(&self.canvas.transform);
            self.canvas.context.Clear(Some(&d2d_color(clear)));
        }
    }

    /// Finishes and presents the frame; whether the GPU device was lost.
    fn end_frame(&mut self) -> bool {
        let devices = &self.canvas.devices;
        if let Err(err) = unsafe { self.canvas.context.EndDraw(None, None) } {
            if devices.lost(err.code()) {
                return true;
            }
            crate::platform::debug_log(&format!("D2D end_frame failed: {err}"));
            return false;
        }
        let hr = unsafe { self.swap_chain.Present(0, DXGI_PRESENT(0)) };
        if hr.is_err() && devices.lost(hr) {
            return true;
        }
        if let Err(err) = hr.ok() {
            crate::platform::debug_log(&format!("DXGI present failed: {err}"));
        }
        false
    }
}

/// A window and the swap chain it's drawn into, at the window's size whenever it's drawn.
pub(crate) struct WindowSurface {
    gpu: Rc<Gpu>,
    // Drops before `window`, so the swap chain and composition target go before DestroyWindow.
    target: Option<Target>,
    pub(crate) window: NativeWindow,
    #[cfg(test)]
    force_device_lost: bool,
}

impl WindowSurface {
    pub(crate) fn new(gpu: Rc<Gpu>, window: NativeWindow) -> Result<Self> {
        let mut surface = Self {
            gpu,
            target: None,
            window,
            #[cfg(test)]
            force_device_lost: false,
        };
        surface.target = Some(surface.build_target()?);
        Ok(surface)
    }

    /// The window's client size in pixels (at least 1x1), and its scale.
    fn size(&self) -> ((u32, u32), f32) {
        let (w, h) = self.window.physical_size();
        ((w.max(1), h.max(1)), self.window.scale_factor())
    }

    fn build_target(&self) -> Result<Target> {
        let (size, scale) = self.size();
        let (generation, devices) = self.gpu.devices();
        Target::new(devices, generation, self.window.hwnd().raw(), size, scale)
    }

    /// Paints a frame cleared to `clear`; whether the GPU device was lost (it has been replaced,
    /// and every surface needs drawing again).
    pub(crate) fn draw(&mut self, clear: Rgba, paint: impl FnOnce(&mut Canvas)) -> bool {
        let (size, scale) = self.size();
        let generation = self.gpu.generation();
        // A target on replaced devices, or one that can't follow the window's size, is rebuilt.
        let stale = self.target.as_mut().is_some_and(|t| {
            t.generation != generation
                || ((t.size, t.scale) != (size, scale) && t.resize(size, scale).is_err())
        });
        if stale {
            self.target = None;
        }
        if self.target.is_none() {
            match self.build_target() {
                Ok(target) => self.target = Some(target),
                Err(err) => {
                    crate::platform::debug_log(&format!("surface build failed: {err}"));
                    return self.recover(generation);
                }
            }
        }
        let target = self.target.as_mut().expect("a target");
        target.begin_frame(clear);
        paint(&mut target.canvas);
        #[cfg(test)]
        let forced = std::mem::take(&mut self.force_device_lost);
        #[cfg(not(test))]
        let forced = false;
        if target.end_frame() || forced {
            let used = target.generation;
            // The old swap chain and composition target go before the devices are replaced.
            self.target = None;
            return self.recover(used);
        }
        false
    }

    /// Replaces the GPU devices, lost at `generation`; true, as every surface must draw again.
    fn recover(&self, generation: u64) -> bool {
        if let Err(err) = self.gpu.recover(generation) {
            crate::platform::debug_log(&format!("GPU recovery failed: {err}"));
        }
        true
    }

    #[cfg(test)]
    pub(crate) fn is_ready(&self) -> bool {
        self.target.is_some()
    }

    #[cfg(test)]
    pub(crate) fn mark_device_lost_for_test(&mut self) {
        self.force_device_lost = true;
    }
}

// ---- 4. The canvas --------------------------------------------------------------------------

/// A rounded rect to fill and stroke.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct PaintQuad {
    pub(crate) bounds: Rect,
    pub(crate) corner_radii: Corners,
    pub(crate) background: Rgba,
    pub(crate) border_widths: Edges,
    pub(crate) border_color: Rgba,
}

/// A quad filled with `background`, square and unbordered.
pub(crate) fn fill(bounds: Rect, background: Rgba) -> PaintQuad {
    PaintQuad {
        bounds,
        background,
        ..Default::default()
    }
}

/// How a scope draws its content: faded, scaled about a point, moved, and clipped.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Scope {
    pub(crate) opacity: Option<f32>,
    pub(crate) scale: Option<(Point, f32, f32)>,
    pub(crate) translate: Option<Point>,
    /// In the caller's coordinates: it stays put while `scale` and `translate` move the content.
    pub(crate) clip: Option<Rect>,
}

/// How a quad's corners are drawn: square, all with one radius, or each with its own.
#[derive(Copy, Clone, PartialEq)]
enum Corner {
    Square,
    Uniform,
    Each,
}

/// What a frame is drawn on.
pub(crate) struct Canvas {
    devices: Devices,
    context: ID2D1DeviceContext,
    brush: ID2D1SolidColorBrush,
    shadow_renderer: Option<ShadowRenderer>,
    shadows: FrameCache<ShadowKey, (ID2D1Bitmap1, f32)>,
    scale: f32,
    transform: Matrix3x2,
}

impl Canvas {
    fn new(devices: Devices, scale: f32) -> Result<Self> {
        let (context, brush) = unsafe {
            let context = devices
                .d2d_device
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            context.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            context.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = context.CreateSolidColorBrush(&d2d_color(black()), None)?;
            (context, brush)
        };
        Ok(Self {
            shadow_renderer: ShadowRenderer::new(&devices.d2d_device),
            devices,
            context,
            brush,
            shadows: FrameCache::new(MAX_SHADOWS, 0, SHADOW_KEEP_FRAMES),
            scale,
            transform: Matrix3x2::identity(),
        })
    }

    /// Draws `draw` within `scope`.
    pub(crate) fn scoped(&mut self, scope: Scope, draw: impl FnOnce(&mut Self)) {
        if let Some(opacity) = scope.opacity {
            let params = D2D1_LAYER_PARAMETERS1 {
                contentBounds: D2D_RECT_F {
                    left: f32::NEG_INFINITY,
                    top: f32::NEG_INFINITY,
                    right: f32::INFINITY,
                    bottom: f32::INFINITY,
                },
                geometricMask: std::mem::ManuallyDrop::new(None),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: Matrix3x2::identity(),
                opacity: opacity.clamp(0.0, 1.0),
                opacityBrush: std::mem::ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS1_NONE,
            };
            unsafe { self.context.PushLayer(&params, None) };
        }
        if let Some(clip) = scope.clip {
            let clip = d2d_rect(clip);
            unsafe {
                self.context
                    .PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE)
            };
        }
        let moved = scope.scale.is_some() || scope.translate.is_some();
        let outer = self.transform;
        if moved {
            let (sx, sy, mut tx, mut ty) = match scope.scale {
                Some((about, sx, sy)) => (sx, sy, about.x * (1.0 - sx), about.y * (1.0 - sy)),
                None => (1.0, 1.0, 0.0, 0.0),
            };
            if let Some(delta) = scope.translate {
                tx += delta.x;
                ty += delta.y;
            }
            let local = Matrix3x2 {
                M11: sx,
                M12: 0.0,
                M21: 0.0,
                M22: sy,
                M31: tx,
                M32: ty,
            };
            self.set_transform(local * outer);
        }

        draw(self);

        if moved {
            self.set_transform(outer);
        }
        if scope.clip.is_some() {
            unsafe { self.context.PopAxisAlignedClip() };
        }
        if scope.opacity.is_some() {
            unsafe { self.context.PopLayer() };
        }
    }

    pub(crate) fn with_clip(&mut self, bounds: Rect, draw: impl FnOnce(&mut Self)) {
        let scope = Scope {
            clip: Some(bounds),
            ..Default::default()
        };
        self.scoped(scope, draw);
    }

    fn set_transform(&mut self, transform: Matrix3x2) {
        self.transform = transform;
        unsafe { self.context.SetTransform(&transform) };
    }

    fn set_color(&self, color: Rgba) {
        unsafe { self.brush.SetColor(&d2d_color(color)) };
    }

    /// Fills the quad's shape and strokes its border.
    pub(crate) fn paint_quad(&mut self, quad: PaintQuad) {
        let bounds = quad.bounds;
        if bounds.is_empty() {
            return;
        }
        let r = quad.corner_radii.clamp_to(bounds.size);
        let radii = [r.top_left, r.top_right, r.bottom_right, r.bottom_left];
        let largest = radii.into_iter().fold(0.0f32, f32::max);
        let corner = if largest <= 0.0 {
            Corner::Square
        } else if same(&radii) {
            Corner::Uniform
        } else {
            Corner::Each
        };
        if !quad.background.is_transparent() {
            self.set_color(quad.background);
            self.draw_shape(d2d_rect(bounds), corner, radii, None);
        }

        let bw = quad.border_widths;
        let widths = [bw.top, bw.right, bw.bottom, bw.left];
        let stroke = widths.into_iter().fold(0.0f32, f32::max);
        if quad.border_color.is_transparent() || stroke <= 0.0 {
            return;
        }
        self.set_color(quad.border_color);
        let uniform = same(&widths);
        let drawn_sides_equal = widths
            .into_iter()
            .filter(|&w| w > 0.0)
            .all(|w| (w - stroke).abs() < 1e-4);
        if uniform || (largest > 0.0 && drawn_sides_equal) {
            // A stroke centred inside the edge, its corners rounded half a stroke less. A border
            // missing on some sides is the same stroke with those sides clipped off.
            let half = stroke * 0.5;
            let inner = d2d_rect(bounds.inset(Edges::all(half)));
            let inner_radii = radii.map(|r| (r - half).max(0.0));
            if uniform {
                self.draw_shape(inner, corner, inner_radii, Some(stroke));
            } else {
                let off = |w: f32| if w <= 0.0 { stroke } else { 0.0 };
                let shown = bounds.inset(Edges {
                    top: off(bw.top),
                    right: off(bw.right),
                    bottom: off(bw.bottom),
                    left: off(bw.left),
                });
                self.with_clip(shown, |c| {
                    c.draw_shape(inner, corner, inner_radii, Some(stroke))
                });
            }
        } else {
            // Sides of different widths are drawn as a bar each.
            let b = bounds;
            let bars = [
                (bw.top, Rect::new(b.origin, size(b.size.width, bw.top))),
                (
                    bw.bottom,
                    Rect::new(
                        point(b.left(), b.bottom() - bw.bottom),
                        size(b.size.width, bw.bottom),
                    ),
                ),
                (bw.left, Rect::new(b.origin, size(bw.left, b.size.height))),
                (
                    bw.right,
                    Rect::new(
                        point(b.right() - bw.right, b.top()),
                        size(bw.right, b.size.height),
                    ),
                ),
            ];
            for (width, bar) in bars {
                if width > 0.0 {
                    unsafe { self.context.FillRectangle(&d2d_rect(bar), &self.brush) };
                }
            }
        }
    }

    /// Fills `rect` with `corner` corners of `radii` (top left, clockwise), or strokes it
    /// `stroke` wide.
    fn draw_shape(&self, rect: D2D_RECT_F, corner: Corner, radii: [f32; 4], stroke: Option<f32>) {
        let (context, brush) = (&self.context, &self.brush);
        unsafe {
            match (corner, stroke) {
                (Corner::Square, None) => context.FillRectangle(&rect, brush),
                (Corner::Square, Some(w)) => context.DrawRectangle(&rect, brush, w, None),
                (Corner::Uniform, _) => {
                    let r = radii.into_iter().fold(0.0f32, f32::max);
                    let rounded = D2D1_ROUNDED_RECT {
                        rect,
                        radiusX: r,
                        radiusY: r,
                    };
                    match stroke {
                        None => context.FillRoundedRectangle(&rounded, brush),
                        Some(w) => context.DrawRoundedRectangle(&rounded, brush, w, None),
                    }
                }
                (Corner::Each, _) => {
                    let Some(path) = self.rounded_path(rect, radii) else {
                        return;
                    };
                    match stroke {
                        None => context.FillGeometry(&path, brush, None),
                        Some(w) => context.DrawGeometry(&path, brush, w, None),
                    }
                }
            }
        }
    }

    /// `rect` as a path with each corner rounded by its radius (top left, clockwise), each kept
    /// within half the shorter side.
    fn rounded_path(&self, rect: D2D_RECT_F, radii: [f32; 4]) -> Option<ID2D1Geometry> {
        let (l, t, r, b) = (rect.left, rect.top, rect.right, rect.bottom);
        let max = ((r - l).max(0.0).min((b - t).max(0.0)) * 0.5).max(0.0);
        let [tl, tr, br, bl] = radii.map(|v| v.clamp(0.0, max));
        let arc = |x: f32, y: f32, radius: f32| D2D1_ARC_SEGMENT {
            point: Vector2 { X: x, Y: y },
            size: D2D_SIZE_F {
                width: radius,
                height: radius,
            },
            rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
            arcSize: D2D1_ARC_SIZE_SMALL,
        };
        unsafe {
            let path = self.devices.d2d_factory.CreatePathGeometry().ok()?;
            let sink = path.Open().ok()?;
            sink.BeginFigure(Vector2 { X: l + tl, Y: t }, D2D1_FIGURE_BEGIN_FILLED);
            for (line, corner) in [
                ((r - tr, t), (r, t + tr, tr)),
                ((r, b - br), (r - br, b, br)),
                ((l + bl, b), (l, b - bl, bl)),
                ((l, t + tl), (l + tl, t, tl)),
            ] {
                sink.AddLine(Vector2 {
                    X: line.0,
                    Y: line.1,
                });
                if corner.2 > 0.0 {
                    sink.AddArc(&arc(corner.0, corner.1, corner.2));
                }
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close().ok()?;
            Some(path.into())
        }
    }

    /// Paints the shadows a shape with `corner_radii` casts.
    pub(crate) fn paint_shadows(&mut self, bounds: Rect, corner_radii: Corners, shadows: Shadows) {
        if bounds.is_empty() || shadows.is_empty() {
            return;
        }
        let radius = corner_radii.clamp_to(bounds.size).top_left;
        for shadow in shadows.iter() {
            if shadow.color.is_transparent() {
                continue;
            }
            let spread = shadow.spread_radius;
            let mut shape = bounds.translate(shadow.offset).inflate(spread);
            shape.size = size(shape.size.width.max(1.0), shape.size.height.max(1.0));
            let blur = shadow.blur_radius.max(0.0);
            let radius = (radius + spread).max(0.0);
            if let Some((bitmap, pad)) = self.shadow(shape.size, radius, blur, shadow.color) {
                let dst = d2d_rect(shape.inflate(pad));
                unsafe {
                    self.context.DrawBitmap(
                        &bitmap,
                        Some(&dst),
                        1.0,
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        None,
                        None,
                    );
                }
            }
        }
    }

    /// The blurred shadow of a `shape` sized rounded rect, and how far it reaches past the
    /// shape; rendered once and kept while it's used.
    fn shadow(
        &mut self,
        shape: Size,
        radius: f32,
        blur: f32,
        color: Rgba,
    ) -> Option<(ID2D1Bitmap1, f32)> {
        let scale = self.scale;
        // Sizes are rounded up to 4 pixels, so a shadow is reused across nearby sizes.
        let quantize = |dip: f32| ((dip * scale).round().max(1.0) as u32).div_ceil(4) * 4;
        let (w_px, h_px) = (quantize(shape.width), quantize(shape.height));
        let key = ShadowKey {
            width_px: w_px,
            height_px: h_px,
            radius_tenths: (radius * 10.0).round() as u32,
            blur_tenths: (blur * 10.0).round() as u32,
            color_bits: [color.r, color.g, color.b, color.a].map(f32::to_bits),
        };
        if let Some(cached) = self.shadows.get(&key) {
            return Some(cached.clone());
        }
        let renderer = self.shadow_renderer.clone()?;
        let rendered = renderer.render(scale, w_px, h_px, radius, blur, color)?;
        self.shadows.insert(key, rendered.clone(), 0);
        Some(rendered)
    }

    fn draw_text(&self, origin: Point, layout: &IDWriteTextLayout, color: Rgba) {
        if color.is_transparent() {
            return;
        }
        self.set_color(color);
        let at = Vector2 {
            X: origin.x,
            Y: origin.y,
        };
        let options = D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT;
        unsafe {
            self.context
                .DrawTextLayout(at, layout, &self.brush, options)
        };
    }

    /// Draws `row` centred in a line `line_height` tall at `origin`, underlining `underline`
    /// (an IME composition).
    pub(crate) fn draw_row(
        &mut self,
        row: &ShapedRow,
        origin: Point,
        line_height: f32,
        color: Rgba,
        underline: Option<Range<usize>>,
    ) {
        let Some(layout) = &row.layout else {
            return;
        };
        let top_padding = ((line_height - (row.ascent + row.descent)) * 0.5).max(0.0);
        let at = point(origin.x, origin.y + top_padding);
        self.draw_text(at, layout, color);
        if let Some(range) = underline.filter(|r| r.start < r.end) {
            let (x0, x1) = (row.x_for_index(range.start), row.x_for_index(range.end));
            let (lo, hi) = (x0.min(x1), x0.max(x1));
            if hi > lo {
                let y = at.y + row.ascent + 2.0;
                self.paint_quad(fill(
                    Rect::new(point(at.x + lo, y), size(hi - lo, 1.0)),
                    color,
                ));
            }
        }
    }

    /// Draws `label` at `origin` in a box `width` wide; a clamped label is clipped to its lines.
    pub(crate) fn draw_label(&mut self, label: &Label, origin: Point, width: f32, color: Rgba) {
        let Some(layout) = &label.layout else {
            return;
        };
        if !label.clamped {
            self.draw_text(origin, layout, color);
            return;
        }
        let clip_w = label
            .wrap_width
            .map_or(width, |w| width.max(w))
            .max(label.width)
            + 2.0;
        let clip = Rect::new(origin, size(clip_w, label.size().height));
        self.with_clip(clip, |c| c.draw_text(origin, layout, color));
    }
}

/// Whether the four values are equal, to within rounding.
fn same(v: &[f32; 4]) -> bool {
    (v[0] - v[1]).abs() < 1e-4 && (v[1] - v[2]).abs() < 1e-4 && (v[2] - v[3]).abs() < 1e-4
}

const MAX_SHADOWS: usize = 16;
const SHADOW_KEEP_FRAMES: u64 = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ShadowKey {
    width_px: u32,
    height_px: u32,
    radius_tenths: u32,
    blur_tenths: u32,
    color_bits: [u32; 4],
}

/// Renders shadow bitmaps on a context of its own: a rounded rect, through Direct2D's shadow
/// effect.
#[derive(Clone)]
struct ShadowRenderer {
    context: ID2D1DeviceContext,
    white: ID2D1SolidColorBrush,
    effect: ID2D1Effect,
}

impl ShadowRenderer {
    fn new(d2d_device: &ID2D1Device) -> Option<Self> {
        unsafe {
            let context = d2d_device
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
                .ok()?;
            let white = context
                .CreateSolidColorBrush(&d2d_color(white()), None)
                .ok()?;
            let effect = context.CreateEffect(&CLSID_D2D1Shadow).ok()?;
            Some(Self {
                context,
                white,
                effect,
            })
        }
    }

    /// The shadow of a `w_px` by `h_px` rect with corners of `radius`, blurred by `blur` in
    /// `color`, and the padding it has around the shape.
    fn render(
        &self,
        scale: f32,
        w_px: u32,
        h_px: u32,
        radius: f32,
        blur: f32,
        color: Rgba,
    ) -> Option<(ID2D1Bitmap1, f32)> {
        let sigma = (blur * 0.5).max(0.5);
        let pad = (sigma * 3.0).ceil();
        let (shape_w, shape_h) = (w_px as f32 / scale, h_px as f32 / scale);
        let total = |dip: f32| ((dip + pad * 2.0) * scale).ceil().max(1.0) as u32;
        let px = D2D_SIZE_U {
            width: total(shape_w),
            height: total(shape_h),
        };
        if px.width > 4096 || px.height > 4096 {
            return None;
        }
        let dpi = 96.0 * scale;
        let props = bitmap_props(scale, D2D1_BITMAP_OPTIONS_TARGET);
        let clear = d2d_color(transparent_black());
        let context = &self.context;
        unsafe {
            let shape = context.CreateBitmap(px, None, 0, &props).ok()?;
            let blurred = context.CreateBitmap(px, None, 0, &props).ok()?;
            context.SetDpi(dpi, dpi);
            context.SetTarget(&shape);
            context.BeginDraw();
            context.Clear(Some(&clear));
            let rounded = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: pad,
                    top: pad,
                    right: pad + shape_w,
                    bottom: pad + shape_h,
                },
                radiusX: radius,
                radiusY: radius,
            };
            context.FillRoundedRectangle(&rounded, &self.white);
            context.EndDraw(None, None).ok()?;

            let color_bytes = [color.r, color.g, color.b, color.a].map(f32::to_le_bytes);
            self.effect.SetInput(0, &shape, true);
            let _ = self.effect.SetValue(
                D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_UNKNOWN,
                &sigma.to_le_bytes(),
            );
            let _ = self.effect.SetValue(
                D2D1_SHADOW_PROP_COLOR.0 as u32,
                D2D1_PROPERTY_TYPE_UNKNOWN,
                color_bytes.as_flattened(),
            );
            context.SetTarget(&blurred);
            context.BeginDraw();
            context.Clear(Some(&clear));
            let output = self.effect.GetOutput().ok()?;
            context.DrawImage(
                &output,
                Some(&Vector2 { X: 0.0, Y: 0.0 }),
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
            context.EndDraw(None, None).ok()?;
            context.SetTarget(None);
            Some((blurred, pad))
        }
    }
}

// ---- 5. Text --------------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum FontWeight {
    #[default]
    Normal,
    Semibold,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Font {
    pub(crate) family: SharedStr,
    pub(crate) weight: FontWeight,
}

impl Default for Font {
    fn default() -> Self {
        Self {
            family: SharedStr::Static("Segoe UI Variable Text"),
            weight: FontWeight::Normal,
        }
    }
}

/// A font at a size. Sizes are kept to the hundredth, so styles can key caches.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TextStyle {
    pub(crate) font: Font,
    size_hundredths: u32,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self::new(Font::default(), 14.0)
    }
}

impl TextStyle {
    fn new(font: Font, size: f32) -> Self {
        let mut style = Self {
            font,
            size_hundredths: 0,
        };
        style.set_size(size);
        style
    }

    pub(crate) fn regular(family: impl Into<SharedStr>, size: f32) -> Self {
        let font = Font {
            family: family.into(),
            weight: FontWeight::Normal,
        };
        Self::new(font, size)
    }

    /// Sets the size, between 1 and 4096 (14 if it isn't a number).
    pub(crate) fn set_size(&mut self, size: f32) {
        let size = if size.is_finite() {
            size.clamp(1.0, 4096.0)
        } else {
            14.0
        };
        self.size_hundredths = (size * 100.0).round().max(1.0) as u32;
    }

    pub(crate) fn size(&self) -> f32 {
        self.size_hundredths as f32 / 100.0
    }
}

/// How a label is laid out: its line height, the width it wraps at (if it wraps), and the most
/// lines it shows.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct LabelLayout {
    pub(crate) line_height: f32,
    pub(crate) wrap_width: Option<f32>,
    pub(crate) line_clamp: Option<usize>,
}

/// One row of editor text, shaped on a single line, with its hit testing.
#[derive(Clone)]
pub(crate) struct ShapedRow {
    pub(crate) len: usize,
    pub(crate) width: f32,
    ascent: f32,
    descent: f32,
    text: SharedStr,
    layout: Option<IDWriteTextLayout>,
    is_ascii: bool,
}

#[cfg(test)]
impl Default for ShapedRow {
    fn default() -> Self {
        Self {
            len: 0,
            width: 0.0,
            ascent: 11.0,
            descent: 3.0,
            text: SharedStr::default(),
            layout: None,
            is_ascii: true,
        }
    }
}

impl ShapedRow {
    /// Where the caret before byte `index` goes.
    pub(crate) fn x_for_index(&self, index: usize) -> f32 {
        let index = index.min(self.len);
        if self.is_ascii && (index == 0 || index >= self.len) {
            return if index == 0 { 0.0 } else { self.width };
        }
        let Some(layout) = &self.layout else {
            return if index == 0 { 0.0 } else { self.width };
        };
        let pos = utf8_to_utf16_offset(&self.text, index, self.is_ascii);
        let (mut x, mut y) = (0.0f32, 0.0f32);
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        let hit = unsafe { layout.HitTestTextPosition(pos, false, &mut x, &mut y, &mut metrics) };
        if hit.is_ok() { x } else { self.width }
    }

    /// The spans `start..end` covers, as (left, right) pairs (more than one where bidi text
    /// splits it).
    pub(crate) fn selection_spans(&self, start: usize, end: usize) -> Vec<(f32, f32)> {
        let (start, end) = (start.min(self.len), end.min(self.len));
        if start >= end {
            return Vec::new();
        }
        if self.is_ascii {
            let (x0, x1) = (self.x_for_index(start), self.x_for_index(end));
            return if x1 > x0 { vec![(x0, x1)] } else { Vec::new() };
        }
        let Some(layout) = &self.layout else {
            return Vec::new();
        };
        let from = utf8_to_utf16_offset(&self.text, start, false);
        let len = utf8_to_utf16_offset(&self.text, end, false).saturating_sub(from);
        if len == 0 {
            return Vec::new();
        }
        let spans = |metrics: &[DWRITE_HIT_TEST_METRICS]| -> Vec<(f32, f32)> {
            let shown = metrics.iter().filter(|m| m.width > 0.0);
            shown.map(|m| (m.left, m.left + m.width)).collect()
        };
        // Most ranges fit in a few boxes; DirectWrite says how many if not.
        let mut few = [DWRITE_HIT_TEST_METRICS::default(); 8];
        let mut count = 0u32;
        let test = |buf: &mut [DWRITE_HIT_TEST_METRICS], count: &mut u32| unsafe {
            layout.HitTestTextRange(from, len, 0.0, 0.0, Some(buf), count)
        };
        if test(&mut few, &mut count).is_ok() {
            return spans(&few[..count as usize]);
        }
        if count > 0 {
            let mut many = vec![DWRITE_HIT_TEST_METRICS::default(); count as usize];
            if test(&mut many, &mut count).is_ok() {
                return spans(&many[..count as usize]);
            }
        }
        let (x0, x1) = (self.x_for_index(start), self.x_for_index(end));
        let (lo, hi) = (x0.min(x1), x0.max(x1));
        if hi > lo { vec![(lo, hi)] } else { Vec::new() }
    }

    /// The byte offset nearest `x`.
    pub(crate) fn closest_index_for_x(&self, x: f32) -> usize {
        if self.len == 0 {
            return 0;
        }
        if self.is_ascii && (x <= 0.0 || x >= self.width) {
            return if x <= 0.0 { 0 } else { self.len };
        }
        let Some(layout) = &self.layout else {
            return 0;
        };
        let (mut trailing, mut inside) = (BOOL(0), BOOL(0));
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        if unsafe { layout.HitTestPoint(x, 0.0, &mut trailing, &mut inside, &mut metrics) }.is_err()
        {
            return 0;
        }
        let pos = if trailing.as_bool() {
            metrics.textPosition.saturating_add(metrics.length)
        } else {
            metrics.textPosition
        };
        utf16_to_utf8_offset(&self.text, pos, self.is_ascii)
    }
}

/// UI text, laid out to a `LabelLayout`.
#[derive(Clone)]
pub(crate) struct Label {
    pub(crate) width: f32,
    pub(crate) line_count: usize,
    line_height: f32,
    wrap_width: Option<f32>,
    /// It had more lines than it shows; the last one ends in an ellipsis.
    clamped: bool,
    layout: Option<IDWriteTextLayout>,
}

impl Label {
    pub(crate) fn size(&self) -> Size {
        size(self.width.ceil(), self.line_height * self.line_count as f32)
    }
}

/// A style's DirectWrite format, with its metrics and what it measures and wraps.
pub(crate) struct FontFace {
    factory: IDWriteFactory5,
    analyzer: Option<IDWriteTextAnalyzer>,
    format: IDWriteTextFormat,
    ascent: f32,
    descent: f32,
    pub(crate) space_width: f32,
    advances: RefCell<Option<AdvanceTable>>,
}

/// Tab stops are this many spaces apart.
const TAB_STOP_COLUMNS: f32 = 8.0;

impl FontFace {
    /// `style`'s face, in Segoe UI if its family isn't installed.
    fn new(
        factory: &IDWriteFactory5,
        analyzer: &Option<IDWriteTextAnalyzer>,
        style: &TextStyle,
    ) -> Self {
        let size = style.size();
        let weight = match style.font.weight {
            FontWeight::Normal => DWRITE_FONT_WEIGHT(400),
            FontWeight::Semibold => DWRITE_FONT_WEIGHT(600),
        };
        let locale = HSTRING::from("");
        let candidates = [
            (style.font.family.as_str(), weight),
            ("Segoe UI", DWRITE_FONT_WEIGHT(400)),
        ];
        let format = candidates
            .into_iter()
            .find_map(|(family, weight)| unsafe {
                let (italic, stretch) = (DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL);
                let family = HSTRING::from(family);
                factory
                    .CreateTextFormat(&family, None, weight, italic, stretch, size, &locale)
                    .ok()
            })
            .expect("CreateTextFormat fallback failed");
        let _ = unsafe { format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP) };
        let space_width = measure(factory, &[u16::from(b' ')], &format);
        if space_width > 0.0 {
            let _ = unsafe { format.SetIncrementalTabStop(space_width * TAB_STOP_COLUMNS) };
        }
        // The line a capital and a descender fill.
        let sample = [u16::from(b'H'), u16::from(b'g')];
        let (ascent, descent) =
            unsafe { factory.CreateTextLayout(&sample, &format, f32::MAX, f32::MAX) }
                .ok()
                .and_then(|layout| {
                    let mut lines = [DWRITE_LINE_METRICS::default(); 2];
                    let mut count = 0u32;
                    let ok = unsafe { layout.GetLineMetrics(Some(&mut lines), &mut count) }.is_ok();
                    let line = lines[0];
                    (ok && count > 0)
                        .then(|| (line.baseline, (line.height - line.baseline).max(0.0)))
                })
                .unwrap_or((size * 0.8, size * 0.2));
        Self {
            factory: factory.clone(),
            analyzer: analyzer.clone(),
            format,
            ascent,
            descent,
            space_width,
            advances: RefCell::new(None),
        }
    }

    fn layout(&self, utf16: &[u16], max_width: f32) -> Option<IDWriteTextLayout> {
        unsafe {
            self.factory
                .CreateTextLayout(utf16, &self.format, max_width, f32::MAX)
        }
        .ok()
    }

    /// Runs `f` with this face's character advances.
    pub(crate) fn with_advances<R>(&self, f: impl FnOnce(&mut AdvanceTable) -> R) -> R {
        let mut table = self.advances.borrow_mut();
        let table = table.get_or_insert_with(|| AdvanceTable::new(self));
        f(table)
    }

    /// `text`'s width on one line.
    pub(crate) fn measure(&self, text: &str) -> f32 {
        measure(&self.factory, &single_line_utf16(text), &self.format)
    }

    /// DirectWrite's own wrapping of `text` (byte offsets): its first row `first_width` wide and
    /// the rest `cont_width`.
    pub(crate) fn wrap_breaks(
        &self,
        text: &str,
        first_width: f32,
        cont_width: f32,
        out: &mut Vec<usize>,
    ) {
        let utf16 = single_line_utf16(text);
        if utf16.is_empty() {
            return;
        }
        // Where a wrapped layout `width` wide ends its lines, in UTF-16 units.
        let line_ends = |slice: &[u16], width: f32| -> Vec<usize> {
            let Some(layout) = self.layout(slice, width.max(1.0)) else {
                return Vec::new();
            };
            let _ = unsafe { layout.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP) };
            let mut m = DWRITE_TEXT_METRICS::default();
            if unsafe { layout.GetMetrics(&mut m) }.is_err() || m.lineCount <= 1 {
                return Vec::new();
            }
            let mut lines = vec![DWRITE_LINE_METRICS::default(); m.lineCount as usize];
            let mut count = 0u32;
            let _ = unsafe { layout.GetLineMetrics(Some(&mut lines), &mut count) };
            let n = (count as usize).min(lines.len());
            let mut end = 0;
            let mut ends = Vec::with_capacity(n.saturating_sub(1));
            for line in &lines[..n.saturating_sub(1)] {
                end += line.length as usize;
                if end > 0 && end < slice.len() {
                    ends.push(end);
                }
            }
            ends
        };
        let mut breaks = line_ends(&utf16, first_width);
        if breaks.is_empty() {
            return;
        }
        // The rows after the first wrap at their own width.
        if (cont_width - first_width).abs() > 0.01 {
            let first = breaks[0];
            breaks.truncate(1);
            if first < utf16.len() {
                breaks.extend(
                    line_ends(&utf16[first..], cont_width.max(1.0))
                        .into_iter()
                        .map(|e| first + e),
                );
            }
        }
        let (mut next, mut units, mut last) = (0, 0, 0);
        for (byte, ch) in text.char_indices() {
            while next < breaks.len() && units >= breaks[next] {
                if byte > last && byte < text.len() {
                    out.push(byte);
                    last = byte;
                }
                next += 1;
            }
            if next >= breaks.len() {
                break;
            }
            units += ch.len_utf16();
        }
    }

    /// Where `text` may break between lines (byte offsets), from DirectWrite's line-break analysis.
    pub(crate) fn break_opportunities(&self, text: &str) -> Vec<usize> {
        let analyze = |a: &IDWriteTextAnalyzer| break_opportunities(a, text);
        self.analyzer.as_ref().map(analyze).unwrap_or_default()
    }
}

/// Shapes text, keeping a face per style and the rows and labels it shaped recently.
pub(crate) struct TextSystem {
    factory: IDWriteFactory5,
    analyzer: Option<IDWriteTextAnalyzer>,
    faces: RefCell<HashMap<TextStyle, Rc<FontFace>>>,
    /// Rows shaped in the last few frames, up to a total size.
    rows: RefCell<FrameCache<(SharedStr, TextStyle), ShapedRow>>,
    /// Labels shaped in this frame or the last.
    labels: RefCell<FrameCache<LabelKey, Label>>,
}

const MAX_SHAPED: usize = 512;
const MAX_ROW_BYTES: usize = 4 * 1024 * 1024;
/// Rows longer than this aren't kept.
const MAX_KEPT_ROW_LEN: usize = 16 * 1024;
const ROW_KEEP_FRAMES: u64 = 16;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct LabelKey {
    text: SharedStr,
    style: TextStyle,
    line_height_tenths: u32,
    wrap_width_tenths: Option<u32>,
    line_clamp: Option<usize>,
}

impl TextSystem {
    pub(crate) fn new() -> Result<Self> {
        let factory: IDWriteFactory5 = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let analyzer = unsafe { factory.CreateTextAnalyzer() }.ok();
        Ok(Self {
            factory,
            analyzer,
            faces: RefCell::default(),
            rows: RefCell::new(FrameCache::new(MAX_SHAPED, MAX_ROW_BYTES, ROW_KEEP_FRAMES)),
            labels: RefCell::new(FrameCache::new(MAX_SHAPED, 0, 1)),
        })
    }

    pub(crate) fn face(&self, style: &TextStyle) -> Rc<FontFace> {
        let mut faces = self.faces.borrow_mut();
        let face = faces
            .entry(style.clone())
            .or_insert_with(|| Rc::new(FontFace::new(&self.factory, &self.analyzer, style)));
        face.clone()
    }

    /// Ages the shaped rows and labels; those not used for a while are dropped.
    pub(crate) fn finish_frame(&self) {
        self.labels.borrow_mut().advance_frame();
        self.rows.borrow_mut().advance_frame();
    }

    /// `text` shaped on one line in `style`.
    pub(crate) fn shape_row(&self, text: SharedStr, style: &TextStyle) -> ShapedRow {
        let face = self.face(style);
        let row = |text: SharedStr, width, layout, is_ascii| ShapedRow {
            len: text.len(),
            width,
            ascent: face.ascent,
            descent: face.descent,
            text,
            layout,
            is_ascii,
        };
        if text.is_empty() {
            return row(text, 0.0, None, true);
        }
        let key = (text.len() <= MAX_KEPT_ROW_LEN).then(|| (text.clone(), style.clone()));
        if let Some(key) = &key
            && let Some(kept) = self.rows.borrow_mut().get(key)
        {
            return kept.clone();
        }
        let is_ascii = text.is_ascii();
        let layout = face.layout(&single_line_utf16(&text), f32::MAX);
        if let Some(layout) = &layout {
            let (height, baseline) = (face.ascent + face.descent, face.ascent);
            let _ = unsafe {
                layout.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, height, baseline)
            };
        }
        let width = layout.as_ref().map_or(0.0, layout_width);
        let shaped = row(text, width, layout, is_ascii);
        if let Some(key) = key {
            let weight = key.0.len().max(64);
            self.rows.borrow_mut().insert(key, shaped.clone(), weight);
        }
        shaped
    }

    /// `text` laid out as a label in `style`.
    pub(crate) fn shape_label(
        &self,
        text: SharedStr,
        style: &TextStyle,
        opts: LabelLayout,
    ) -> Label {
        let line_height = opts.line_height.max(1.0);
        let wrap_width = opts.wrap_width.map(|w| w.max(1.0));
        let key = LabelKey {
            text: text.clone(),
            style: style.clone(),
            line_height_tenths: (line_height * 10.0).round() as u32,
            wrap_width_tenths: wrap_width.map(|w| (w * 10.0).round() as u32),
            line_clamp: opts.line_clamp,
        };
        if let Some(kept) = self.labels.borrow_mut().get(&key) {
            return kept.clone();
        }

        let face = self.face(style);
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let layout = face.layout(&utf16, wrap_width.unwrap_or(f32::MAX));
        if let Some(layout) = &layout {
            // Each line `line_height` tall, with the glyphs centred in it.
            let glyphs = (face.ascent + face.descent).max(1.0);
            let baseline = ((line_height - glyphs) * 0.5).max(0.0) + face.ascent;
            let wrapping = if wrap_width.is_some() {
                DWRITE_WORD_WRAPPING_WRAP
            } else {
                DWRITE_WORD_WRAPPING_NO_WRAP
            };
            unsafe {
                let _ = layout.SetWordWrapping(wrapping);
                let _ = layout.SetLineSpacing(
                    DWRITE_LINE_SPACING_METHOD_UNIFORM,
                    line_height,
                    baseline,
                );
            }
        }
        let (width, lines) = layout.as_ref().map_or((0.0, 1), |layout| {
            let mut m = DWRITE_TEXT_METRICS::default();
            let _ = unsafe { layout.GetMetrics(&mut m) };
            (
                m.widthIncludingTrailingWhitespace,
                m.lineCount.max(1) as usize,
            )
        });
        // Past the clamp, the last line shown ends in an ellipsis.
        let shown = match opts.line_clamp {
            Some(cap) if lines > cap => Some(cap.max(1)),
            _ => None,
        };
        if let (Some(shown), Some(layout)) = (shown, &layout) {
            unsafe {
                let _ = layout.SetMaxHeight(line_height * shown as f32);
                if let Ok(sign) = self.factory.CreateEllipsisTrimmingSign(&face.format) {
                    let trimming = DWRITE_TRIMMING {
                        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                        delimiter: 0,
                        delimiterCount: 0,
                    };
                    let _ = layout.SetTrimming(&trimming, &sign);
                }
            }
        }
        let label = Label {
            width,
            line_count: shown.unwrap_or(lines),
            line_height,
            wrap_width,
            clamped: shown.is_some(),
            layout,
        };
        self.labels.borrow_mut().insert(key, label.clone(), 0);
        label
    }

    #[cfg(test)]
    fn label_cache_counts(&self) -> (usize, usize) {
        self.labels.borrow().age_counts()
    }
}

/// The width of `utf16` on one line in `format`.
fn measure(factory: &IDWriteFactory5, utf16: &[u16], format: &IDWriteTextFormat) -> f32 {
    if utf16.is_empty() {
        return 0.0;
    }
    unsafe { factory.CreateTextLayout(utf16, format, f32::MAX, f32::MAX) }
        .map_or(0.0, |layout| layout_width(&layout))
}

fn layout_width(layout: &IDWriteTextLayout) -> f32 {
    let mut metrics = DWRITE_TEXT_METRICS::default();
    if unsafe { layout.GetMetrics(&mut metrics) }.is_ok() {
        metrics.widthIncludingTrailingWhitespace
    } else {
        0.0
    }
}

/// `text` in UTF-16, with the characters DirectWrite would break a line at shown as boxes.
fn single_line_utf16(text: &str) -> Vec<u16> {
    let mut out = Vec::with_capacity(text.len());
    for ch in text.chars() {
        let ch = match ch {
            '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}' => '\u{25A1}',
            other => other,
        };
        out.extend_from_slice(ch.encode_utf16(&mut [0u16; 2]));
    }
    out
}

fn utf8_to_utf16_offset(text: &str, utf8_offset: usize, is_ascii: bool) -> u32 {
    if is_ascii {
        return utf8_offset.min(text.len()) as u32;
    }
    let end = text.floor_char_boundary(utf8_offset.min(text.len()));
    text[..end].encode_utf16().count() as u32
}

fn utf16_to_utf8_offset(text: &str, utf16_offset: u32, is_ascii: bool) -> usize {
    if is_ascii {
        return (utf16_offset as usize).min(text.len());
    }
    let mut units = 0u32;
    for (byte, ch) in text.char_indices() {
        if units >= utf16_offset {
            return byte;
        }
        units += ch.len_utf16() as u32;
    }
    text.len()
}

// ---- 6. Character advances and break analysis -----------------------------------------------

/// Ranges of characters that don't combine with their neighbours when shaped: Latin, Greek,
/// Cyrillic, punctuation and symbols, and CJK.
const SIMPLE_CHARS: [RangeInclusive<u32>; 17] = [
    0x0020..=0x007e,
    0x00a0..=0x02ff,
    0x0370..=0x0482,
    0x048a..=0x052f,
    0x1e00..=0x1fff,
    0x2010..=0x2027,
    0x2030..=0x205e,
    0x2070..=0x20cf,
    0x2100..=0x2bff,
    0x3000..=0x3029,
    0x3030..=0x3098,
    0x309b..=0x30ff,
    0x3400..=0x4dbf,
    0x4e00..=0x9fff,
    0xac00..=0xd7a3,
    0xf900..=0xfaff,
    0xff01..=0xffee,
];

/// Whether `text` is as wide as its characters' advances added up (no shaping across characters).
pub(crate) fn has_simple_advances(text: &str) -> bool {
    text.bytes()
        .all(|b| b == b'\t' || (0x20..=0x7e).contains(&b))
        || text
            .chars()
            .all(|c| c == '\t' || SIMPLE_CHARS.iter().any(|r| r.contains(&(c as u32))))
}

/// Character advances for one face, each measured once (printable ASCII up front).
pub(crate) struct AdvanceTable {
    factory: IDWriteFactory5,
    format: IDWriteTextFormat,
    ascii: [f32; 95],
    /// The widest printable ASCII character.
    pub(crate) max_ascii: f32,
    others: HashMap<char, f32>,
}

impl AdvanceTable {
    fn new(face: &FontFace) -> Self {
        let mut ascii = [0.0f32; 95];
        for (advance, byte) in ascii.iter_mut().zip(0x20u8..=0x7e) {
            *advance = measure(&face.factory, &[u16::from(byte)], &face.format);
        }
        Self {
            factory: face.factory.clone(),
            format: face.format.clone(),
            max_ascii: ascii.into_iter().fold(0.0f32, f32::max),
            ascii,
            others: HashMap::new(),
        }
    }

    /// How far `ch` advances when drawn at `current_x` (tabs advance to the next tab stop).
    pub(crate) fn advance(&mut self, ch: char, current_x: f32) -> f32 {
        let space = self.ascii[0];
        if ch == '\t' {
            let stop = (space * TAB_STOP_COLUMNS).max(1.0);
            let next = ((current_x / stop).floor() + 1.0) * stop;
            return (next - current_x).max(space.max(1.0));
        }
        if (0x20..=0x7e).contains(&(ch as u32)) {
            return self.ascii[(ch as u32 - 0x20) as usize];
        }
        let (factory, format) = (&self.factory, &self.format);
        *self
            .others
            .entry(ch)
            .or_insert_with(|| measure(factory, ch.encode_utf16(&mut [0u16; 2]), format))
    }
}

/// Where `text` may break between lines (byte offsets): after a character where the analysis
/// says it must, or where it can and nothing says it may not.
fn break_opportunities(analyzer: &IDWriteTextAnalyzer, text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    if text.is_empty() {
        return out;
    }
    let utf16: Rc<Vec<u16>> = Rc::new(text.encode_utf16().collect());
    let len = utf16.len();
    let points = Rc::new(RefCell::new(vec![DWRITE_LINE_BREAKPOINT::default(); len]));
    let source: IDWriteTextAnalysisSource = BreakSource {
        utf16,
        locale: HSTRING::from(""),
    }
    .into();
    let sink: IDWriteTextAnalysisSink = BreakSink {
        points: points.clone(),
    }
    .into();
    if unsafe { analyzer.AnalyzeLineBreakpoints(&source, 0, len as u32, &sink) }.is_err() {
        return out;
    }
    let points = points.borrow();
    let [can, may_not, must] = [
        DWRITE_BREAK_CONDITION_CAN_BREAK,
        DWRITE_BREAK_CONDITION_MAY_NOT_BREAK,
        DWRITE_BREAK_CONDITION_MUST_BREAK,
    ]
    .map(|c| c.0 as u8);
    let mut unit = 0;
    for (byte, ch) in text.char_indices() {
        let (last, next) = (unit + ch.len_utf16() - 1, unit + ch.len_utf16());
        let next_byte = byte + ch.len_utf8();
        if next_byte < text.len() && next < points.len() {
            let after = (points[last]._bitfield >> 2) & 0b11;
            let before = points[next]._bitfield & 0b11;
            let either = |c| after == c || before == c;
            if either(must) || (!either(may_not) && either(can)) {
                out.push(next_byte);
            }
        }
        unit = next;
    }
    out
}

/// The text a line-break analysis reads, all one left-to-right paragraph.
#[implement(IDWriteTextAnalysisSource)]
struct BreakSource {
    utf16: Rc<Vec<u16>>,
    locale: HSTRING,
}

impl IDWriteTextAnalysisSource_Impl for BreakSource_Impl {
    fn GetTextAtPosition(
        &self,
        textposition: u32,
        textstring: *mut *mut u16,
        textlength: *mut u32,
    ) -> windows::core::Result<()> {
        let pos = textposition as usize;
        unsafe {
            if pos >= self.utf16.len() {
                *textstring = std::ptr::null_mut();
                *textlength = 0;
            } else {
                *textstring = self.utf16.as_ptr().add(pos) as *mut u16;
                *textlength = (self.utf16.len() - pos) as u32;
            }
        }
        Ok(())
    }

    fn GetTextBeforePosition(
        &self,
        textposition: u32,
        textstring: *mut *mut u16,
        textlength: *mut u32,
    ) -> windows::core::Result<()> {
        let pos = (textposition as usize).min(self.utf16.len());
        unsafe {
            if pos == 0 {
                *textstring = std::ptr::null_mut();
                *textlength = 0;
            } else {
                *textstring = self.utf16.as_ptr() as *mut u16;
                *textlength = pos as u32;
            }
        }
        Ok(())
    }

    fn GetParagraphReadingDirection(&self) -> DWRITE_READING_DIRECTION {
        DWRITE_READING_DIRECTION_LEFT_TO_RIGHT
    }

    fn GetLocaleName(
        &self,
        textposition: u32,
        textlength: *mut u32,
        localename: *mut *mut u16,
    ) -> windows::core::Result<()> {
        let pos = (textposition as usize).min(self.utf16.len());
        unsafe {
            *textlength = (self.utf16.len() - pos) as u32;
            *localename = self.locale.as_ptr() as *mut u16;
        }
        Ok(())
    }

    fn GetNumberSubstitution(
        &self,
        textposition: u32,
        textlength: *mut u32,
        numbersubstitution: windows::core::OutRef<'_, IDWriteNumberSubstitution>,
    ) -> windows::core::Result<()> {
        let pos = (textposition as usize).min(self.utf16.len());
        unsafe { *textlength = (self.utf16.len() - pos) as u32 };
        let _ = numbersubstitution.write(None);
        Ok(())
    }
}

/// Collects a line-break analysis's breakpoints.
#[implement(IDWriteTextAnalysisSink)]
struct BreakSink {
    points: Rc<RefCell<Vec<DWRITE_LINE_BREAKPOINT>>>,
}

impl IDWriteTextAnalysisSink_Impl for BreakSink_Impl {
    fn SetScriptAnalysis(
        &self,
        _textposition: u32,
        _textlength: u32,
        _scriptanalysis: *const DWRITE_SCRIPT_ANALYSIS,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn SetLineBreakpoints(
        &self,
        textposition: u32,
        textlength: u32,
        linebreakpoints: *const DWRITE_LINE_BREAKPOINT,
    ) -> windows::core::Result<()> {
        let (start, len) = (textposition as usize, textlength as usize);
        if len == 0 || linebreakpoints.is_null() {
            return Ok(());
        }
        let mut points = self.points.borrow_mut();
        if start.saturating_add(len) <= points.len() {
            let src = unsafe { std::slice::from_raw_parts(linebreakpoints, len) };
            points[start..start + len].copy_from_slice(src);
        }
        Ok(())
    }

    fn SetBidiLevel(
        &self,
        _textposition: u32,
        _textlength: u32,
        _explicitlevel: u8,
        _resolvedlevel: u8,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn SetNumberSubstitution(
        &self,
        _textposition: u32,
        _textlength: u32,
        _numbersubstitution: windows::core::Ref<'_, IDWriteNumberSubstitution>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

// ---- 7. The frame cache ---------------------------------------------------------------------

/// A cache of values used across frames: an entry not used for `keep_frames` frames is dropped,
/// and the least recently used goes first when it's over `max_entries` or (if it's nonzero)
/// `max_weight`.
struct FrameCache<K, V> {
    /// Each value with the frame it was last used in and its weight.
    entries: HashMap<K, (V, u64, usize)>,
    frame: u64,
    max_entries: usize,
    max_weight: usize,
    weight: usize,
    keep_frames: u64,
}

impl<K: Eq + Hash + Clone, V> FrameCache<K, V> {
    fn new(max_entries: usize, max_weight: usize, keep_frames: u64) -> Self {
        Self {
            entries: HashMap::new(),
            frame: 0,
            max_entries,
            max_weight,
            weight: 0,
            keep_frames,
        }
    }

    fn get(&mut self, key: &K) -> Option<&V> {
        let entry = self.entries.get_mut(key)?;
        entry.1 = self.frame;
        Some(&entry.0)
    }

    fn insert(&mut self, key: K, value: V, weight: usize) {
        if let Some(entry) = self.entries.get_mut(&key) {
            self.weight = self.weight.saturating_sub(entry.2) + weight;
            *entry = (value, self.frame, weight);
            return;
        }
        let over = |c: &Self| {
            c.entries.len() >= c.max_entries
                || (c.max_weight > 0
                    && !c.entries.is_empty()
                    && c.weight.saturating_add(weight) > c.max_weight)
        };
        while over(self) {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.1)
                .map(|(k, _)| k.clone());
            let Some(oldest) = oldest else {
                break;
            };
            if let Some((_, _, w)) = self.entries.remove(&oldest) {
                self.weight = self.weight.saturating_sub(w);
            }
        }
        self.weight += weight;
        self.entries.insert(key, (value, self.frame, weight));
    }

    fn advance_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        let (frame, keep, weight) = (self.frame, self.keep_frames, &mut self.weight);
        self.entries.retain(|_, (_, last, w)| {
            let alive = frame.wrapping_sub(*last) <= keep;
            if !alive {
                *weight = weight.saturating_sub(*w);
            }
            alive
        });
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.weight = 0;
    }

    /// How many entries were used this frame, and how many before.
    #[cfg(test)]
    fn age_counts(&self) -> (usize, usize) {
        let current = self.entries.values().filter(|e| e.1 == self.frame).count();
        (current, self.entries.len() - current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scope's clip stays in the caller's coordinates while its translate moves the content,
    /// so a sliding reveal (the menu unroll) never draws above the card it's clipped to.
    #[test]
    fn scope_clip_stays_put_while_the_translate_moves_content() {
        use windows::Win32::Graphics::Direct2D::{D2D1_BITMAP_OPTIONS_CPU_READ, D2D1_MAP_OPTIONS_READ};
        let mut canvas = Canvas::new(Devices::new().unwrap(), 1.0).unwrap();
        let props = |bitmap_options| D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: bitmap_options,
            ..Default::default()
        };
        let one_by_eight = D2D_SIZE_U { width: 1, height: 8 };
        let painted_rows = unsafe {
            let target = canvas
                .context
                .CreateBitmap(one_by_eight, None, 0, &props(D2D1_BITMAP_OPTIONS_TARGET))
                .unwrap();
            canvas.context.SetTarget(&target);
            canvas.context.BeginDraw();
            canvas.context.Clear(None);
            let scope = Scope {
                clip: Some(Rect::new(point(0., 4.), size(1., 4.))),
                translate: Some(point(0., -4.)),
                ..Default::default()
            };
            canvas.scoped(scope, |c| c.paint_quad(fill(Rect::new(point(0., 4.), size(1., 8.)), black())));
            canvas.context.EndDraw(None, None).unwrap();
            let cpu = D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW;
            let readback = canvas.context.CreateBitmap(one_by_eight, None, 0, &props(cpu)).unwrap();
            readback.CopyFromBitmap(None, &target, None).unwrap();
            let map = readback.Map(D2D1_MAP_OPTIONS_READ).unwrap();
            let alpha = |row: usize| *map.bits.add(row * map.pitch as usize + 3);
            let rows: Vec<bool> = (0..8).map(|row| alpha(row) != 0).collect();
            readback.Unmap().unwrap();
            rows
        };
        assert_eq!(painted_rows, [false, false, false, false, true, true, true, true]);
    }

    #[test]
    fn utf8_utf16_roundtrip_handles_ascii_cjk_and_surrogate_emoji() {
        let sample = "hi 日本語 🎉 end";
        for (byte_ix, _) in sample.char_indices().chain([(sample.len(), '\0')]) {
            let u16_off = utf8_to_utf16_offset(sample, byte_ix, false);
            assert_eq!(utf16_to_utf8_offset(sample, u16_off, false), byte_ix);
        }
    }

    #[test]
    fn directwrite_line_breaks_and_shaping_work() {
        let ts = TextSystem::new().unwrap();
        let style = TextStyle::regular("Segoe UI Variable Text", 14.0);
        let shaped = ts.shape_row(SharedStr::Static("Hello 🎉 world"), &style);
        assert!(shaped.width > 0.0);
        let mid_x = shaped.x_for_index(6);
        assert!(mid_x > 0.0 && mid_x < shaped.width);
        let hit = shaped.closest_index_for_x(mid_x);
        assert!(hit <= shaped.len);

        let analyzer = ts.analyzer.as_ref().expect("text analyzer");
        let breaks = super::break_opportunities(analyzer, "hello world");
        assert!(breaks.contains(&6));
        assert!(
            super::break_opportunities(analyzer, "foo\u{00A0}bar")
                .is_empty()
        );
        assert_eq!(
            super::break_opportunities(analyzer, "foo-bar"),
            vec![4]
        );
    }

    #[test]
    fn wrapped_text_two_frame_cache_promotes_and_evicts() {
        let ts = TextSystem::new().unwrap();
        let style = TextStyle::regular("Segoe UI Variable Small", 12.0);
        let single = LabelLayout {
            line_height: 16.0,
            wrap_width: None,
            line_clamp: None,
        };
        let wrapped = LabelLayout {
            wrap_width: Some(80.0),
            ..single
        };

        let a = ts.shape_label(SharedStr::Static("Ln 1, Col 1"), &style, single);
        let b = ts.shape_label(SharedStr::Static("Ln 1, Col 1"), &style, wrapped);
        assert_ne!(
            a.layout.as_ref().unwrap().as_raw(),
            b.layout.as_ref().unwrap().as_raw()
        );
        assert_eq!(ts.label_cache_counts(), (2, 0));

        ts.finish_frame();
        assert_eq!(ts.label_cache_counts(), (0, 2));
        let a2 = ts.shape_label(SharedStr::Static("Ln 1, Col 1"), &style, single);
        assert_eq!(
            a.layout.as_ref().unwrap().as_raw(),
            a2.layout.as_ref().unwrap().as_raw()
        );
        assert_eq!(ts.label_cache_counts(), (1, 1));

        ts.finish_frame();
        assert_eq!(ts.label_cache_counts(), (0, 1));
        ts.finish_frame();
        assert_eq!(ts.label_cache_counts(), (0, 0));
    }
}
