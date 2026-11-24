use std::cell::RefCell;
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
            D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
            D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            D2D1_FACTORY_OPTIONS, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_INTERPOLATION_MODE_LINEAR,
            D2D1_LAYER_OPTIONS1_NONE, D2D1_LAYER_PARAMETERS1, D2D1_PROPERTY_TYPE_UNKNOWN,
            D2D1_ROUNDED_RECT, D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION, D2D1_SHADOW_PROP_COLOR,
            D2D1_SWEEP_DIRECTION_CLOCKWISE, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory,
            ID2D1Bitmap1, ID2D1Device, ID2D1DeviceContext, ID2D1Effect, ID2D1Factory1,
            ID2D1PathGeometry, ID2D1SolidColorBrush,
        },
        Direct3D::{
            D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_11_1,
        },
        Direct3D11::{
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
        },
        DirectComposition::{
            DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget,
            IDCompositionVisual,
        },
        DirectWrite::IDWriteTextLayout,
        Dxgi::{
            Common::{
                DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
            },
            CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_ERROR_DEVICE_REMOVED,
            DXGI_ERROR_DEVICE_RESET, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
            DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory6, IDXGISurface,
            IDXGISwapChain1,
        },
    },
};
use windows::core::{HRESULT, Interface};
use windows_numerics::{Matrix3x2, Vector2};

use crate::color::Rgba;
use crate::geom::{Corners, Edges, Point, Rect, Shadows, Size, point, size};
use crate::platform::NativeWindow;
use crate::types::{Error, Result};

use super::FrameCache;

const SWAPCHAIN_BUFFERS: u32 = 2;
const MAX_SHADOW_CACHE_ENTRIES: usize = 16;
const SHADOW_CACHE_KEEP_FRAMES: u64 = 120;

#[inline]
fn is_device_lost_hr(hr: HRESULT) -> bool {
    hr == D2DERR_RECREATE_TARGET
        || hr == DXGI_ERROR_DEVICE_REMOVED
        || hr == DXGI_ERROR_DEVICE_RESET
}

#[inline]
fn to_d2d_color(c: Rgba) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

#[inline]
fn to_d2d_rect(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.left(),
        top: r.top(),
        right: r.right(),
        bottom: r.bottom(),
    }
}

#[derive(Clone)]
struct Direct2DDevices {
    dxgi_factory: IDXGIFactory6,
    d3d_device: ID3D11Device,
    dxgi_device: IDXGIDevice,
    d2d_factory: ID2D1Factory1,
    d2d_device: ID2D1Device,
}

impl Direct2DDevices {
    fn new() -> Result<Self> {
        let dxgi_factory: IDXGIFactory6 =
            unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };
        let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let mut d3d_device: Option<ID3D11Device> = None;
        let mut last_err = None;
        for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
            match unsafe {
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
            } {
                Ok(()) => {
                    last_err = None;
                    break;
                }
                Err(err) => last_err = Some(err),
            }
        }
        if let Some(err) = last_err {
            return Err(err.into());
        }
        let d3d_device =
            d3d_device.ok_or_else(|| Error::new("D3D11CreateDevice returned null device"))?;
        let dxgi_device: IDXGIDevice = d3d_device.cast()?;

        let d2d_factory: ID2D1Factory1 = unsafe {
            D2D1CreateFactory(
                D2D1_FACTORY_TYPE_SINGLE_THREADED,
                Some(&D2D1_FACTORY_OPTIONS::default()),
            )?
        };
        let d2d_device = unsafe { d2d_factory.CreateDevice(&dxgi_device)? };

        Ok(Self {
            dxgi_factory,
            d3d_device,
            dxgi_device,
            d2d_factory,
            d2d_device,
        })
    }
}

struct GpuInner {
    devices: Direct2DDevices,
    generation: u64,
}

pub(crate) struct Gpu {
    inner: RefCell<GpuInner>,
}

impl Gpu {
    pub(crate) fn new() -> Result<Rc<Self>> {
        let devices = Direct2DDevices::new()
            .map_err(|err| Error::new(format!("initializing Direct3D11/Direct2D devices: {err}")))?;
        Ok(Rc::new(Self {
            inner: RefCell::new(GpuInner {
                devices,
                generation: 0,
            }),
        }))
    }

    pub(crate) fn generation(&self) -> u64 {
        self.inner.borrow().generation
    }

    fn snapshot(&self) -> (u64, Direct2DDevices) {
        let inner = self.inner.borrow();
        (inner.generation, inner.devices.clone())
    }

    fn recover(&self, stale_gen: u64) -> Result<(u64, Direct2DDevices)> {
        {
            let inner = self.inner.borrow();
            if inner.generation != stale_gen {
                return Ok((inner.generation, inner.devices.clone()));
            }
        }
        let next_devices = Direct2DDevices::new()?;
        let mut inner = self.inner.borrow_mut();
        if inner.generation == stale_gen {
            inner.devices = next_devices;
            inner.generation = inner.generation.wrapping_add(1);
        }
        Ok((inner.generation, inner.devices.clone()))
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum FrameOutcome {
    Presented,
    DeviceLost,
    Failed,
}

struct Target {
    generation: u64,
    swap_chain: IDXGISwapChain1,
    _comp_device: IDCompositionDevice,
    _comp_target: IDCompositionTarget,
    _comp_visual: IDCompositionVisual,
    target_bitmap: Option<ID2D1Bitmap1>,
    canvas: Canvas,
}

impl Target {
    fn new(
        devices: Direct2DDevices,
        generation: u64,
        hwnd: HWND,
        width: u32,
        height: u32,
        scale: f32,
    ) -> Result<Self> {
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
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
        let swap_chain = unsafe {
            devices
                .dxgi_factory
                .CreateSwapChainForComposition(&devices.d3d_device, &desc, None)?
        };

        let (comp_device, comp_target, comp_visual) = unsafe {
            let comp_device: IDCompositionDevice =
                DCompositionCreateDevice(&devices.dxgi_device)?;
            let comp_target = comp_device.CreateTargetForHwnd(hwnd, true)?;
            let comp_visual = comp_device.CreateVisual()?;
            comp_visual.SetContent(&swap_chain)?;
            comp_target.SetRoot(&comp_visual)?;
            comp_device.Commit()?;
            (comp_device, comp_target, comp_visual)
        };

        let canvas = Canvas::new(devices, scale)?;
        let mut target = Self {
            generation,
            swap_chain,
            _comp_device: comp_device,
            _comp_target: comp_target,
            _comp_visual: comp_visual,
            target_bitmap: None,
            canvas,
        };
        target.attach_backbuffer(scale)?;
        Ok(target)
    }

    fn attach_backbuffer(&mut self, scale: f32) -> Result<()> {
        let surface: IDXGISurface = unsafe { self.swap_chain.GetBuffer(0)? };
        let dpi = 96.0 * scale;
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: dpi,
            dpiY: dpi,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            ..Default::default()
        };
        let bitmap = unsafe {
            let bitmap = self
                .canvas
                .context
                .CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            self.canvas.context.SetDpi(dpi, dpi);
            self.canvas.context.SetTarget(&bitmap);
            bitmap
        };
        self.target_bitmap = Some(bitmap);
        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32, scale: f32) -> std::result::Result<(), bool> {
        unsafe {
            self.canvas.context.SetTarget(None);
        }
        self.target_bitmap = None;
        if let Err(err) = unsafe {
            self.swap_chain.ResizeBuffers(
                SWAPCHAIN_BUFFERS,
                width,
                height,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_SWAP_CHAIN_FLAG(0),
            )
        } {
            let lost = is_device_lost_hr(err.code());
            if lost {
                crate::platform::diag::debug_log(&format!("surface resize lost device: {err}"));
            }
            return Err(lost);
        }
        if let Err(err) = self.attach_backbuffer(scale) {
            let lost = err.hresult().is_some_and(|hr| is_device_lost_hr(HRESULT(hr)));
            if lost {
                crate::platform::diag::debug_log(&format!("surface resize lost device: {err}"));
            }
            return Err(lost);
        }
        if (scale - self.canvas.scale_factor).abs() >= f32::EPSILON {
            self.canvas.shadow_cache.clear();
        }
        self.canvas.scale_factor = scale;
        Ok(())
    }

    fn begin_frame(&mut self, scale: f32, clear: Rgba) -> bool {
        let mut attach_failed = false;
        if self.target_bitmap.is_none() && self.attach_backbuffer(scale).is_err() {
            attach_failed = true;
        }
        self.canvas.shadow_cache.advance_frame();
        self.canvas.transform = Matrix3x2::identity();
        unsafe {
            self.canvas.context.BeginDraw();
            self.canvas.context.SetTransform(&self.canvas.transform);
            self.canvas.context.Clear(Some(&to_d2d_color(clear)));
        }
        attach_failed
    }

    fn end_frame(&mut self, force_lost: bool) -> FrameOutcome {
        let end_res = unsafe { self.canvas.context.EndDraw(None, None) };
        if force_lost {
            return FrameOutcome::DeviceLost;
        }
        if let Err(err) = end_res {
            let removed =
                unsafe { self.canvas.devices.d3d_device.GetDeviceRemovedReason() }.is_err();
            if is_device_lost_hr(err.code()) || removed {
                return FrameOutcome::DeviceLost;
            }
            crate::platform::diag::debug_log(&format!("D2D end_frame failed: {err}"));
            return FrameOutcome::Failed;
        }
        let hr = unsafe { self.swap_chain.Present(0, DXGI_PRESENT(0)) };
        let removed = if hr.is_err() {
            unsafe { self.canvas.devices.d3d_device.GetDeviceRemovedReason() }.is_err()
        } else {
            false
        };
        if is_device_lost_hr(hr) || removed {
            return FrameOutcome::DeviceLost;
        }
        if let Err(err) = hr.ok() {
            crate::platform::diag::debug_log(&format!("DXGI present failed: {err}"));
            return FrameOutcome::Failed;
        }
        FrameOutcome::Presented
    }
}

pub(crate) struct Surface {
    gpu: Rc<Gpu>,
    hwnd: HWND,
    width: u32,
    height: u32,
    scale: f32,
    target: Option<Target>,
    #[cfg(test)]
    force_device_lost: bool,
}

impl Surface {
    pub(crate) fn for_window(gpu: Rc<Gpu>, window: &NativeWindow) -> Result<Self> {
        let (pw, ph) = window.physical_size();
        let width = pw.max(1);
        let height = ph.max(1);
        let scale_factor = window.scale_factor();
        let hwnd = HWND(window.hwnd().raw_isize() as *mut _);
        let (generation, devices) = gpu.snapshot();
        let target = Target::new(devices, generation, hwnd, width, height, scale_factor)?;
        Ok(Self {
            gpu,
            hwnd,
            width,
            height,
            scale: scale_factor,
            target: Some(target),
            #[cfg(test)]
            force_device_lost: false,
        })
    }

    pub(crate) fn resize(&mut self, physical_width: u32, physical_height: u32, scale_factor: f32) {
        let width = physical_width.max(1);
        let height = physical_height.max(1);
        let same = width == self.width
            && height == self.height
            && (scale_factor - self.scale).abs() < f32::EPSILON;
        self.width = width;
        self.height = height;
        self.scale = scale_factor;
        if let Some(target) = &mut self.target {
            if same && target.target_bitmap.is_some() {
                return;
            }
            if let Err(true) = target.resize(width, height, scale_factor) {
                self.target = None;
            }
        }
    }

    pub(crate) fn draw(&mut self, clear: Rgba, paint: impl FnOnce(&mut Canvas)) -> bool {
        let current_gen = self.gpu.generation();
        if self
            .target
            .as_ref()
            .is_some_and(|t| t.generation != current_gen)
        {
            // Drop old swapchain and DComp target before creating the replacement.
            self.target = None;
        }
        if self.target.is_none() {
            let (generation, devices) = self.gpu.snapshot();
            match Target::new(devices, generation, self.hwnd, self.width, self.height, self.scale) {
                Ok(target) => self.target = Some(target),
                Err(err) => {
                    crate::platform::diag::debug_log(&format!("surface recreate failed: {err}"));
                    return false;
                }
            }
        }
        let Some(target) = &mut self.target else {
            return false;
        };
        let used_gen = target.generation;
        let attach_failed = target.begin_frame(self.scale, clear);
        paint(&mut target.canvas);
        #[cfg(test)]
        let force_lost = attach_failed || std::mem::take(&mut self.force_device_lost);
        #[cfg(not(test))]
        let force_lost = attach_failed;
        match target.end_frame(force_lost) {
            FrameOutcome::Presented | FrameOutcome::Failed => false,
            FrameOutcome::DeviceLost => {
                // Drop old swapchain and DComp target before creating the replacement.
                self.target = None;
                match self.gpu.recover(used_gen) {
                    Ok((new_gen, new_devices)) => {
                        self.target = Target::new(
                            new_devices,
                            new_gen,
                            self.hwnd,
                            self.width,
                            self.height,
                            self.scale,
                        )
                        .ok();
                    }
                    Err(err) => {
                        crate::platform::diag::debug_log(&format!("GPU recovery failed: {err}"));
                    }
                }
                true
            }
        }
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

pub(crate) struct WindowSurface {
    // Surface drops before NativeWindow so swapchain/DComp target release before DestroyWindow.
    pub(crate) surface: Surface,
    pub(crate) window: NativeWindow,
}

impl WindowSurface {
    pub(crate) fn new(gpu: Rc<Gpu>, window: NativeWindow) -> Result<Self> {
        let surface = Surface::for_window(gpu, &window)?;
        Ok(Self { surface, window })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ShadowCacheKey {
    width_px: u32,
    height_px: u32,
    radius_tenths: u32,
    blur_tenths: u32,
    color_bits: [u32; 4],
}

struct CachedShadow {
    bitmap: ID2D1Bitmap1,
    pad_dip: f32,
}

#[derive(Clone)]
struct ShadowPipeline {
    context: ID2D1DeviceContext,
    white_brush: ID2D1SolidColorBrush,
    effect: ID2D1Effect,
}

impl ShadowPipeline {
    fn new(d2d_device: &ID2D1Device) -> Option<Self> {
        let context =
            unsafe { d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE) }.ok()?;
        let white_brush = unsafe {
            context.CreateSolidColorBrush(&to_d2d_color(Rgba::new(1.0, 1.0, 1.0, 1.0)), None)
        }
        .ok()?;
        let effect = unsafe { context.CreateEffect(&CLSID_D2D1Shadow) }.ok()?;
        Some(Self {
            context,
            white_brush,
            effect,
        })
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct PaintQuad {
    pub(crate) bounds: Rect,
    pub(crate) corner_radii: Corners,
    pub(crate) background: Rgba,
    pub(crate) border_widths: Edges,
    pub(crate) border_color: Rgba,
}

#[inline]
pub(crate) fn fill(bounds: Rect, background: Rgba) -> PaintQuad {
    PaintQuad {
        bounds,
        background,
        ..Default::default()
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct Scope {
    pub(crate) opacity: Option<f32>,
    pub(crate) scale: Option<(Point, f32, f32)>,
    pub(crate) translate: Option<Point>,
    pub(crate) clip: Option<Rect>,
}

pub(crate) struct Canvas {
    devices: Direct2DDevices,
    context: ID2D1DeviceContext,
    solid_brush: ID2D1SolidColorBrush,
    shadow_pipeline: Option<ShadowPipeline>,
    shadow_cache: FrameCache<ShadowCacheKey, CachedShadow>,
    scale_factor: f32,
    transform: Matrix3x2,
}

impl Canvas {
    fn new(devices: Direct2DDevices, scale_factor: f32) -> Result<Self> {
        let (context, solid_brush) = unsafe {
            let context = devices
                .d2d_device
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            context.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            context.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let solid_brush =
                context.CreateSolidColorBrush(&to_d2d_color(Rgba::new(0.0, 0.0, 0.0, 1.0)), None)?;
            (context, solid_brush)
        };
        let shadow_pipeline = ShadowPipeline::new(&devices.d2d_device);

        Ok(Self {
            devices,
            context,
            solid_brush,
            shadow_pipeline,
            shadow_cache: FrameCache::new(MAX_SHADOW_CACHE_ENTRIES, 0, SHADOW_CACHE_KEEP_FRAMES),
            scale_factor,
            transform: Matrix3x2::identity(),
        })
    }

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
            unsafe {
                self.context.PushLayer(&params, None);
            }
        }
        let prev_transform = (scope.scale.is_some() || scope.translate.is_some()).then(|| {
            let (sx, sy, mut tx, mut ty) = match scope.scale {
                Some((origin, sx, sy)) => (sx, sy, origin.x * (1.0 - sx), origin.y * (1.0 - sy)),
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
            let prev = self.transform;
            self.transform = local * prev;
            unsafe {
                self.context.SetTransform(&self.transform);
            }
            prev
        });
        if let Some(bounds) = scope.clip {
            let rect = to_d2d_rect(bounds);
            unsafe {
                self.context
                    .PushAxisAlignedClip(&rect, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            }
        }

        draw(self);

        if scope.clip.is_some() {
            unsafe {
                self.context.PopAxisAlignedClip();
            }
        }
        if let Some(prev) = prev_transform {
            self.transform = prev;
            unsafe {
                self.context.SetTransform(&prev);
            }
        }
        if scope.opacity.is_some() {
            unsafe {
                self.context.PopLayer();
            }
        }
    }

    pub(crate) fn with_clip(&mut self, bounds: Rect, draw: impl FnOnce(&mut Self)) {
        self.scoped(
            Scope {
                clip: Some(bounds),
                ..Default::default()
            },
            draw,
        );
    }

    #[inline]
    fn set_brush_color(&self, color: Rgba) {
        unsafe {
            self.solid_brush.SetColor(&to_d2d_color(color));
        }
    }

    fn create_per_corner_geometry(
        &self,
        rect: D2D_RECT_F,
        tl: f32,
        tr: f32,
        br: f32,
        bl: f32,
    ) -> Option<ID2D1PathGeometry> {
        let (geom, sink) = unsafe {
            let geom = self.devices.d2d_factory.CreatePathGeometry().ok()?;
            let sink = geom.Open().ok()?;
            (geom, sink)
        };
        let (l, t, r, b) = (rect.left, rect.top, rect.right, rect.bottom);
        let w = (r - l).max(0.0);
        let h = (b - t).max(0.0);
        let max_r = (w.min(h) * 0.5).max(0.0);
        let (tl, tr, br, bl) = (
            tl.clamp(0.0, max_r),
            tr.clamp(0.0, max_r),
            br.clamp(0.0, max_r),
            bl.clamp(0.0, max_r),
        );
        let arc = |end: Vector2, rad: f32| D2D1_ARC_SEGMENT {
            point: end,
            size: D2D_SIZE_F {
                width: rad,
                height: rad,
            },
            rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
            arcSize: D2D1_ARC_SIZE_SMALL,
        };
        unsafe {
            sink.BeginFigure(Vector2 { X: l + tl, Y: t }, D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(Vector2 { X: r - tr, Y: t });
            if tr > 0.0 {
                sink.AddArc(&arc(Vector2 { X: r, Y: t + tr }, tr));
            }
            sink.AddLine(Vector2 { X: r, Y: b - br });
            if br > 0.0 {
                sink.AddArc(&arc(Vector2 { X: r - br, Y: b }, br));
            }
            sink.AddLine(Vector2 { X: l + bl, Y: b });
            if bl > 0.0 {
                sink.AddArc(&arc(Vector2 { X: l, Y: b - bl }, bl));
            }
            sink.AddLine(Vector2 { X: l, Y: t + tl });
            if tl > 0.0 {
                sink.AddArc(&arc(Vector2 { X: l + tl, Y: t }, tl));
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close().ok()?;
        }
        Some(geom.into())
    }

    pub(crate) fn paint_quad(&mut self, quad: PaintQuad) {
        if quad.bounds.is_empty() {
            return;
        }
        let radii = quad.corner_radii.clamp_to(quad.bounds.size);
        let (tl, tr, br, bl) = (
            radii.top_left,
            radii.top_right,
            radii.bottom_right,
            radii.bottom_left,
        );
        let max_r = tl.max(tr).max(br).max(bl);
        let uniform_radii = (tl - tr).abs() < 1e-4
            && (tr - br).abs() < 1e-4
            && (br - bl).abs() < 1e-4;

        if !quad.background.is_transparent() {
            self.set_brush_color(quad.background);
            let rect = to_d2d_rect(quad.bounds);
            if max_r <= 0.0 {
                unsafe {
                    self.context.FillRectangle(&rect, &self.solid_brush);
                }
            } else if uniform_radii {
                let rr = D2D1_ROUNDED_RECT {
                    rect,
                    radiusX: max_r,
                    radiusY: max_r,
                };
                unsafe {
                    self.context.FillRoundedRectangle(&rr, &self.solid_brush);
                }
            } else if let Some(geom) = self.create_per_corner_geometry(rect, tl, tr, br, bl) {
                unsafe {
                    self.context.FillGeometry(&geom, &self.solid_brush, None);
                }
            }
        }

        let bw = quad.border_widths;
        let (bt, brw, bb, blw) = (bw.top, bw.right, bw.bottom, bw.left);
        let max_bw = bt.max(brw).max(bb).max(blw);
        if quad.border_color.is_transparent() || max_bw <= 0.0 {
            return;
        }

        self.set_brush_color(quad.border_color);
        let uniform_border = (bt - brw).abs() < 1e-4
            && (brw - bb).abs() < 1e-4
            && (bb - blw).abs() < 1e-4;
        let active_equal = [bt, brw, bb, blw]
            .into_iter()
            .filter(|&w| w > 0.0)
            .all(|w| (w - max_bw).abs() < 1e-4);

        if uniform_border || (max_r > 0.0 && active_equal) {
            let stroke = max_bw;
            let half = stroke * 0.5;
            let inset_rect = to_d2d_rect(quad.bounds.inset(Edges::all(half)));
            let draw_stroke = |canvas: &Self| {
                if max_r <= 0.0 {
                    unsafe {
                        canvas
                            .context
                            .DrawRectangle(&inset_rect, &canvas.solid_brush, stroke, None);
                    }
                } else if uniform_radii {
                    let r = (max_r - half).max(0.0);
                    let rr = D2D1_ROUNDED_RECT {
                        rect: inset_rect,
                        radiusX: r,
                        radiusY: r,
                    };
                    unsafe {
                        canvas
                            .context
                            .DrawRoundedRectangle(&rr, &canvas.solid_brush, stroke, None);
                    }
                } else if let Some(geom) = canvas.create_per_corner_geometry(
                    inset_rect,
                    (tl - half).max(0.0),
                    (tr - half).max(0.0),
                    (br - half).max(0.0),
                    (bl - half).max(0.0),
                ) {
                    unsafe {
                        canvas
                            .context
                            .DrawGeometry(&geom, &canvas.solid_brush, stroke, None);
                    }
                }
            };
            if uniform_border {
                draw_stroke(self);
            } else {
                let clip_bounds = quad.bounds.inset(Edges {
                    top: if bt <= 0.0 { stroke } else { 0.0 },
                    right: if brw <= 0.0 { stroke } else { 0.0 },
                    bottom: if bb <= 0.0 { stroke } else { 0.0 },
                    left: if blw <= 0.0 { stroke } else { 0.0 },
                });
                self.with_clip(clip_bounds, |c| draw_stroke(c));
            }
        } else {
            let b = quad.bounds;
            let draw_edge = |r: Rect| {
                let rect = to_d2d_rect(r);
                unsafe { self.context.FillRectangle(&rect, &self.solid_brush) };
            };
            if bt > 0.0 {
                draw_edge(Rect::new(b.origin, size(b.size.width, bt)));
            }
            if bb > 0.0 {
                draw_edge(Rect::new(point(b.left(), b.bottom() - bb), size(b.size.width, bb)));
            }
            if blw > 0.0 {
                draw_edge(Rect::new(b.origin, size(blw, b.size.height)));
            }
            if brw > 0.0 {
                draw_edge(Rect::new(point(b.right() - brw, b.top()), size(brw, b.size.height)));
            }
        }
    }

    pub(crate) fn paint_shadows(&mut self, bounds: Rect, corner_radii: Corners, shadows: Shadows) {
        if bounds.is_empty() || shadows.is_empty() {
            return;
        }
        let max_r = corner_radii.clamp_to(bounds.size).top_left;
        for shadow in shadows.iter() {
            if shadow.color.is_transparent() {
                continue;
            }
            let spread = shadow.spread_radius;
            let mut sb = bounds.translate(shadow.offset).inflate(spread);
            sb.size = size(sb.size.width.max(1.0), sb.size.height.max(1.0));
            let blur = shadow.blur_radius.max(0.0);
            let radius = (max_r + spread).max(0.0);

            if let Some((bitmap, pad_dip)) =
                self.get_or_render_shadow(sb.size, radius, blur, shadow.color)
            {
                let dst = to_d2d_rect(sb.inflate(pad_dip));
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

    fn get_or_render_shadow(
        &mut self,
        shape_size: Size,
        radius: f32,
        blur_radius: f32,
        color: Rgba,
    ) -> Option<(ID2D1Bitmap1, f32)> {
        let scale = self.scale_factor;
        let quant = |px: u32| px.div_ceil(4) * 4;
        let width_px = quant((shape_size.width * scale).round().max(1.0) as u32);
        let height_px = quant((shape_size.height * scale).round().max(1.0) as u32);
        let key = ShadowCacheKey {
            width_px,
            height_px,
            radius_tenths: (radius * 10.0).round() as u32,
            blur_tenths: (blur_radius * 10.0).round() as u32,
            color_bits: [
                color.r.to_bits(),
                color.g.to_bits(),
                color.b.to_bits(),
                color.a.to_bits(),
            ],
        };

        if let Some(entry) = self.shadow_cache.get(&key) {
            return Some((entry.bitmap.clone(), entry.pad_dip));
        }

        let pipeline = self.shadow_pipeline.clone()?;
        let sigma = (blur_radius * 0.5).max(0.5);
        let pad_dip = (sigma * 3.0).ceil();
        let shape_w_dip = width_px as f32 / scale;
        let shape_h_dip = height_px as f32 / scale;
        let total_w_dip = shape_w_dip + pad_dip * 2.0;
        let total_h_dip = shape_h_dip + pad_dip * 2.0;
        let total_w_px = (total_w_dip * scale).ceil().max(1.0) as u32;
        let total_h_px = (total_h_dip * scale).ceil().max(1.0) as u32;
        if total_w_px > 4096 || total_h_px > 4096 {
            return None;
        }

        let dpi = 96.0 * scale;
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: dpi,
            dpiY: dpi,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
            ..Default::default()
        };
        let (shape_bmp, blurred_bmp) = unsafe {
            let sz = D2D_SIZE_U {
                width: total_w_px,
                height: total_h_px,
            };
            let shape_bmp = pipeline.context.CreateBitmap(sz, None, 0, &props).ok()?;
            let blurred_bmp = pipeline.context.CreateBitmap(sz, None, 0, &props).ok()?;
            (shape_bmp, blurred_bmp)
        };

        unsafe {
            pipeline.context.SetDpi(dpi, dpi);
            pipeline.context.SetTarget(&shape_bmp);
            pipeline.context.BeginDraw();
            pipeline
                .context
                .Clear(Some(&to_d2d_color(Rgba::new(0.0, 0.0, 0.0, 0.0))));
            let rr = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: pad_dip,
                    top: pad_dip,
                    right: pad_dip + shape_w_dip,
                    bottom: pad_dip + shape_h_dip,
                },
                radiusX: radius,
                radiusY: radius,
            };
            pipeline
                .context
                .FillRoundedRectangle(&rr, &pipeline.white_brush);
            pipeline.context.EndDraw(None, None).ok()?;
        }

        let mut color_bytes = [0u8; 16];
        color_bytes[0..4].copy_from_slice(&color.r.to_le_bytes());
        color_bytes[4..8].copy_from_slice(&color.g.to_le_bytes());
        color_bytes[8..12].copy_from_slice(&color.b.to_le_bytes());
        color_bytes[12..16].copy_from_slice(&color.a.to_le_bytes());
        unsafe {
            pipeline.effect.SetInput(0, &shape_bmp, true);
            let _ = pipeline.effect.SetValue(
                D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_UNKNOWN,
                &sigma.to_le_bytes(),
            );
            let _ = pipeline.effect.SetValue(
                D2D1_SHADOW_PROP_COLOR.0 as u32,
                D2D1_PROPERTY_TYPE_UNKNOWN,
                &color_bytes,
            );
            pipeline.context.SetTarget(&blurred_bmp);
            pipeline.context.BeginDraw();
            pipeline
                .context
                .Clear(Some(&to_d2d_color(Rgba::new(0.0, 0.0, 0.0, 0.0))));
            let output = pipeline.effect.GetOutput().ok()?;
            pipeline.context.DrawImage(
                &output,
                Some(&Vector2 { X: 0.0, Y: 0.0 }),
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
            pipeline.context.EndDraw(None, None).ok()?;
            pipeline.context.SetTarget(None);
        }

        self.shadow_cache.insert(
            key,
            CachedShadow {
                bitmap: blurred_bmp.clone(),
                pad_dip,
            },
            0,
        );
        Some((blurred_bmp, pad_dip))
    }

    pub(super) fn draw_text_layout(
        &mut self,
        origin: Point,
        layout: &IDWriteTextLayout,
        default_color: Rgba,
    ) {
        if default_color.is_transparent() {
            return;
        }
        self.set_brush_color(default_color);
        unsafe {
            self.context.DrawTextLayout(
                Vector2 {
                    X: origin.x,
                    Y: origin.y,
                },
                layout,
                &self.solid_brush,
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            );
        }
    }
}
