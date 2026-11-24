#![allow(unsafe_code)]

use std::cell::RefCell;
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
            D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
            D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            D2D1_FACTORY_OPTIONS, D2D1_FACTORY_TYPE_SINGLE_THREADED,
            D2D1_INTERPOLATION_MODE_LINEAR, D2D1_LAYER_OPTIONS1_NONE, D2D1_LAYER_PARAMETERS1,
            D2D1_PROPERTY_TYPE_UNKNOWN, D2D1_ROUNDED_RECT,
            D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION, D2D1_SHADOW_PROP_COLOR,
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

use crate::geom::{Corners, Edges, Point, Rect, Rgba, Shadows, Size, point, size};
use crate::platform::NativeWindow;
use crate::types::{Error, Result, SharedStr};

struct FrameCache<K, V> {
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

    fn insert(&mut self, key: K, value: V, item_weight: usize) {
        if let Some(entry) = self.entries.get_mut(&key) {
            self.weight = self.weight.saturating_sub(entry.2) + item_weight;
            *entry = (value, self.frame, item_weight);
            return;
        }
        while self.entries.len() >= self.max_entries
            || (self.max_weight > 0
                && !self.entries.is_empty()
                && self.weight.saturating_add(item_weight) > self.max_weight)
        {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, last, _))| *last)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((_, _, w)) = self.entries.remove(&oldest) {
                self.weight = self.weight.saturating_sub(w);
            }
        }
        self.weight += item_weight;
        self.entries.insert(key, (value, self.frame, item_weight));
    }

    fn advance_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
        let frame = self.frame;
        let keep = self.keep_frames;
        let weight = &mut self.weight;
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

    #[cfg(test)]
    fn age_counts(&self) -> (usize, usize) {
        let mut curr = 0;
        let mut prev = 0;
        for (_, last, _) in self.entries.values() {
            if *last == self.frame {
                curr += 1;
            } else {
                prev += 1;
            }
        }
        (curr, prev)
    }
}

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
                crate::platform::debug_log(&format!("surface resize lost device: {err}"));
            }
            return Err(lost);
        }
        if let Err(err) = self.attach_backbuffer(scale) {
            let lost = err.hresult().is_some_and(|hr| is_device_lost_hr(HRESULT(hr)));
            if lost {
                crate::platform::debug_log(&format!("surface resize lost device: {err}"));
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
            crate::platform::debug_log(&format!("D2D end_frame failed: {err}"));
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
            crate::platform::debug_log(&format!("DXGI present failed: {err}"));
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
                    crate::platform::debug_log(&format!("surface recreate failed: {err}"));
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
                        crate::platform::debug_log(&format!("GPU recovery failed: {err}"));
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

    fn draw_text_layout(
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

const MAX_LAYOUT_CACHE: usize = 512;
const MAX_SHAPED_CACHE_BYTES: usize = 4 * 1024 * 1024;
const MAX_CACHEABLE_LINE_BYTES: usize = 16 * 1024;
const ROW_CACHE_KEEP_FRAMES: u64 = 16;
const HUGE_LINE_BYTES: usize = 4096;
const TAB_STOP_COLUMNS: f32 = 8.0;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum FontWeight {
    #[default]
    Normal,
    Semibold,
}

impl FontWeight {
    #[inline]
    fn to_dwrite(self) -> DWRITE_FONT_WEIGHT {
        match self {
            Self::Normal => DWRITE_FONT_WEIGHT(400),
            Self::Semibold => DWRITE_FONT_WEIGHT(600),
        }
    }
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

pub(crate) fn font(family: impl Into<SharedStr>) -> Font {
    Font {
        family: family.into(),
        weight: FontWeight::Normal,
    }
}

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
    #[inline]
    fn quantize_size(size: f32) -> u32 {
        let safe = if size.is_finite() {
            size.clamp(1.0, 4096.0)
        } else {
            14.0
        };
        (safe * 100.0).round().max(1.0) as u32
    }

    pub(crate) fn new(font: Font, size: f32) -> Self {
        Self {
            font,
            size_hundredths: Self::quantize_size(size),
        }
    }

    pub(crate) fn regular(family: impl Into<SharedStr>, size: f32) -> Self {
        Self::new(font(family), size)
    }

    #[inline]
    pub(crate) fn set_size(&mut self, size: f32) {
        self.size_hundredths = Self::quantize_size(size);
    }

    #[inline]
    pub(crate) fn size(&self) -> f32 {
        self.size_hundredths as f32 / 100.0
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct LabelLayout {
    pub(crate) line_height: f32,
    pub(crate) wrap_width: Option<f32>,
    pub(crate) line_clamp: Option<usize>,
}

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
    pub(crate) fn x_for_index(&self, index: usize) -> f32 {
        let index = index.min(self.len);
        if index == 0 && self.is_ascii {
            return 0.0;
        }
        if index >= self.len && self.is_ascii {
            return self.width;
        }
        let Some(layout) = &self.layout else {
            return if index == 0 { 0.0 } else { self.width };
        };
        let utf16_pos = utf8_to_utf16_offset(self.text.as_str(), index, self.is_ascii);
        let mut x = 0.0f32;
        let mut y = 0.0f32;
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        if unsafe { layout.HitTestTextPosition(utf16_pos, false, &mut x, &mut y, &mut metrics) }
            .is_ok()
        {
            x
        } else {
            self.width
        }
    }

    pub(crate) fn selection_spans(&self, start: usize, end: usize) -> Vec<(f32, f32)> {
        let start = start.min(self.len);
        let end = end.min(self.len);
        if start >= end {
            return Vec::new();
        }
        if self.is_ascii {
            let x0 = self.x_for_index(start);
            let x1 = self.x_for_index(end);
            return if x1 > x0 { vec![(x0, x1)] } else { Vec::new() };
        }
        let Some(layout) = &self.layout else {
            return Vec::new();
        };
        let u16_start = utf8_to_utf16_offset(self.text.as_str(), start, false);
        let u16_end = utf8_to_utf16_offset(self.text.as_str(), end, false);
        let u16_len = u16_end.saturating_sub(u16_start);
        if u16_len == 0 {
            return Vec::new();
        }
        let mut actual = 0u32;
        let mut buf = [DWRITE_HIT_TEST_METRICS::default(); 8];
        let res = unsafe {
            layout.HitTestTextRange(u16_start, u16_len, 0.0, 0.0, Some(&mut buf), &mut actual)
        };
        if res.is_ok() {
            return buf[..actual as usize]
                .iter()
                .filter(|m| m.width > 0.0)
                .map(|m| (m.left, m.left + m.width))
                .collect();
        }
        if actual > 0 {
            let mut heap = vec![DWRITE_HIT_TEST_METRICS::default(); actual as usize];
            if unsafe {
                layout.HitTestTextRange(u16_start, u16_len, 0.0, 0.0, Some(&mut heap), &mut actual)
            }
            .is_ok()
            {
                return heap[..actual as usize]
                    .iter()
                    .filter(|m| m.width > 0.0)
                    .map(|m| (m.left, m.left + m.width))
                    .collect();
            }
        }
        let x0 = self.x_for_index(start);
        let x1 = self.x_for_index(end);
        let (lo, hi) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
        if hi > lo { vec![(lo, hi)] } else { Vec::new() }
    }

    pub(crate) fn closest_index_for_x(&self, x: f32) -> usize {
        if self.len == 0 {
            return 0;
        }
        if self.is_ascii && x <= 0.0 {
            return 0;
        }
        if self.is_ascii && x >= self.width {
            return self.len;
        }
        let Some(layout) = &self.layout else {
            return 0;
        };
        let mut is_trailing = BOOL(0);
        let mut is_inside = BOOL(0);
        let mut metrics = DWRITE_HIT_TEST_METRICS::default();
        if unsafe { layout.HitTestPoint(x, 0.0, &mut is_trailing, &mut is_inside, &mut metrics) }
            .is_err()
        {
            return 0;
        }
        let utf16_pos = if is_trailing.as_bool() {
            metrics.textPosition.saturating_add(metrics.length)
        } else {
            metrics.textPosition
        };
        utf16_to_utf8_offset(self.text.as_str(), utf16_pos, self.is_ascii)
    }
}

#[derive(Clone)]
pub(crate) struct Label {
    pub(crate) width: f32,
    pub(crate) line_count: usize,
    line_height: f32,
    layout_box_width: Option<f32>,
    clamped: bool,
    layout: Option<IDWriteTextLayout>,
}

impl Label {
    pub(crate) fn size(&self) -> Size {
        size(self.width.ceil(), self.line_height * self.line_count as f32)
    }
}

impl Canvas {
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
        let glyph_height = row.ascent + row.descent;
        let top_padding = ((line_height - glyph_height) * 0.5).max(0.0);
        let draw_origin = point(origin.x, origin.y + top_padding);

        self.draw_text_layout(draw_origin, layout, color);

        if let Some(range) = underline
            && range.start < range.end
        {
            let x0 = row.x_for_index(range.start);
            let x1 = row.x_for_index(range.end);
            let (lo, hi) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
            if hi > lo {
                let y = draw_origin.y + row.ascent + 2.0;
                self.paint_quad(fill(
                    Rect::new(point(draw_origin.x + lo, y), size(hi - lo, 1.0)),
                    color,
                ));
            }
        }
    }

    pub(crate) fn draw_label(
        &mut self,
        label: &Label,
        origin: Point,
        width: f32,
        color: Rgba,
    ) {
        let Some(layout) = &label.layout else {
            return;
        };
        if label.clamped {
            let clip_w = label
                .layout_box_width
                .map_or(width, |bw| width.max(bw))
                .max(label.width)
                + 2.0;
            self.with_clip(Rect::new(origin, size(clip_w, label.size().height)), |c| {
                c.draw_text_layout(origin, layout, color);
            });
        } else {
            self.draw_text_layout(origin, layout, color);
        }
    }
}

pub(crate) struct FontFace {
    factory: IDWriteFactory5,
    analyzer: Option<IDWriteTextAnalyzer>,
    format: IDWriteTextFormat,
    ascent: f32,
    descent: f32,
    space_width: f32,
    advances: RefCell<Option<AdvanceTable>>,
}

impl FontFace {
    #[inline]
    pub(crate) fn space_width(&self) -> f32 {
        self.space_width
    }

    fn with_advances<R>(&self, f: impl FnOnce(&mut AdvanceTable) -> R) -> R {
        let mut slot = self.advances.borrow_mut();
        let table = slot.get_or_insert_with(|| AdvanceTable::new(self.factory.clone(), self.format.clone()));
        f(table)
    }

    pub(crate) fn max_printable_ascii(&self) -> f32 {
        self.with_advances(|adv| adv.max_printable_ascii())
    }

    pub(crate) fn hanging_indent(&self, line: &str, max_chars: u32, max_width: f32) -> f32 {
        self.with_advances(|adv| {
            let mut chars = 0u32;
            let mut width = 0.0f32;
            for ch in line.chars() {
                if (ch == ' ' || ch == '\t') && chars < max_chars {
                    width += adv.advance(ch, width);
                    chars += 1;
                } else {
                    break;
                }
            }
            if width >= max_width { 0.0 } else { width }
        })
    }

    pub(crate) fn line_breaks(
        &self,
        line: &str,
        wrap_width: f32,
        indent_px: f32,
        out: &mut Vec<usize>,
    ) {
        out.clear();
        if line.is_empty() || !wrap_width.is_finite() || wrap_width <= 0.0 {
            return;
        }
        self.with_advances(|adv| {
            if can_wrap_with_advances(line) {
                adv.fast_breaks(line, wrap_width, indent_px, self.analyzer.as_ref(), out);
            } else {
                let cont_width = (wrap_width - indent_px).max(1.0);
                adv.dwrite_breaks(line, wrap_width, cont_width, out);
            }
        });
    }

    pub(crate) fn horizontal_window(
        &self,
        line: &str,
        min_x: f32,
        max_x: f32,
    ) -> Option<(usize, usize, f32, f32)> {
        if !can_wrap_with_advances(line) {
            return None;
        }
        Some(self.with_advances(|adv| adv.horizontal_slice(line, min_x, max_x)))
    }

    pub(crate) fn unwrapped_width(&self, line: &str) -> f32 {
        if line.is_empty() {
            return 0.0;
        }
        if line.len() > HUGE_LINE_BYTES && can_wrap_with_advances(line) {
            self.with_advances(|adv| adv.horizontal_slice(line, 0.0, 0.0).3)
        } else {
            let utf16 = encode_single_line_utf16(line);
            measure_utf16(&self.factory, &utf16, &self.format)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RowCacheKey {
    text: SharedStr,
    style: TextStyle,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct LabelCacheKey {
    text: SharedStr,
    style: TextStyle,
    line_height_tenths: u32,
    wrap_width_tenths: Option<u32>,
    line_clamp: Option<usize>,
}

pub(crate) struct TextSystem {
    factory: IDWriteFactory5,
    analyzer: Option<IDWriteTextAnalyzer>,
    state: RefCell<TextSystemState>,
}

struct TextSystemState {
    faces: HashMap<TextStyle, Rc<FontFace>>,
    rows: FrameCache<RowCacheKey, ShapedRow>,
    labels: FrameCache<LabelCacheKey, Label>,
}

impl TextSystem {
    pub(crate) fn new() -> Result<Self> {
        let factory: IDWriteFactory5 =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let analyzer = unsafe { factory.CreateTextAnalyzer() }.ok();
        Ok(Self {
            factory,
            analyzer,
            state: RefCell::new(TextSystemState {
                faces: HashMap::new(),
                rows: FrameCache::new(
                    MAX_LAYOUT_CACHE,
                    MAX_SHAPED_CACHE_BYTES,
                    ROW_CACHE_KEEP_FRAMES,
                ),
                labels: FrameCache::new(MAX_LAYOUT_CACHE, 0, 1),
            }),
        })
    }

    pub(crate) fn face(&self, style: &TextStyle) -> Rc<FontFace> {
        let mut state = self.state.borrow_mut();
        state
            .faces
            .entry(style.clone())
            .or_insert_with(|| Rc::new(create_font_face(&self.factory, &self.analyzer, style)))
            .clone()
    }

    pub(crate) fn finish_frame(&self) {
        let mut state = self.state.borrow_mut();
        state.labels.advance_frame();
        state.rows.advance_frame();
    }

    pub(crate) fn measure(&self, text: &str, style: &TextStyle) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        if text == " " {
            return self.face(style).space_width();
        }
        self.shape_label(
            SharedStr::from(text.to_owned()),
            style,
            LabelLayout {
                line_height: style.size(),
                wrap_width: None,
                line_clamp: None,
            },
        )
        .width
    }

    pub(crate) fn shape_row(&self, text: SharedStr, style: &TextStyle) -> ShapedRow {
        let face = self.face(style);
        if text.is_empty() {
            return ShapedRow {
                len: 0,
                width: 0.0,
                ascent: face.ascent,
                descent: face.descent,
                text,
                layout: None,
                is_ascii: true,
            };
        }

        let cache_key = (text.len() <= MAX_CACHEABLE_LINE_BYTES).then(|| RowCacheKey {
            text: text.clone(),
            style: style.clone(),
        });
        if let Some(key) = &cache_key
            && let Some(cached) = self.state.borrow_mut().rows.get(key)
        {
            return cached.clone();
        }

        let utf16 = encode_single_line_utf16(text.as_str());
        let is_ascii = text.as_str().is_ascii();
        let layout = unsafe {
            self.factory
                .CreateTextLayout(&utf16, &face.format, f32::MAX, f32::MAX)
        }
        .ok();

        if let Some(layout) = &layout {
            let _ = unsafe {
                layout.SetLineSpacing(
                    DWRITE_LINE_SPACING_METHOD_UNIFORM,
                    face.ascent + face.descent,
                    face.ascent,
                )
            };
        }

        let width = layout.as_ref().map_or(0.0, |l| {
            let mut metrics = DWRITE_TEXT_METRICS::default();
            if unsafe { l.GetMetrics(&mut metrics) }.is_ok() {
                metrics.widthIncludingTrailingWhitespace
            } else {
                0.0
            }
        });

        let shaped = ShapedRow {
            len: text.len(),
            width,
            ascent: face.ascent,
            descent: face.descent,
            text,
            layout,
            is_ascii,
        };

        if let Some(key) = cache_key {
            let weight = key.text.len().max(64);
            self.state.borrow_mut().rows.insert(key, shaped.clone(), weight);
        }
        shaped
    }

    pub(crate) fn shape_label(
        &self,
        text: SharedStr,
        style: &TextStyle,
        opts: LabelLayout,
    ) -> Label {
        let lh = opts.line_height.max(1.0);
        let box_w = opts.wrap_width.map(|w| w.max(1.0));
        let key = LabelCacheKey {
            text: text.clone(),
            style: style.clone(),
            line_height_tenths: (lh * 10.0).round() as u32,
            wrap_width_tenths: box_w.map(|w| (w * 10.0).round() as u32),
            line_clamp: opts.line_clamp,
        };

        if let Some(cached) = self.state.borrow_mut().labels.get(&key) {
            return cached.clone();
        }

        let face = self.face(style);
        let glyph_h = (face.ascent + face.descent).max(1.0);
        let utf16: Vec<u16> = text.as_str().encode_utf16().collect();
        let max_w = box_w.unwrap_or(f32::MAX);
        let layout = unsafe {
            self.factory
                .CreateTextLayout(&utf16, &face.format, max_w, f32::MAX)
        }
        .ok();

        if let Some(layout) = &layout {
            let top_pad = ((lh - glyph_h) * 0.5).max(0.0);
            unsafe {
                let _ = layout.SetWordWrapping(if box_w.is_some() {
                    DWRITE_WORD_WRAPPING_WRAP
                } else {
                    DWRITE_WORD_WRAPPING_NO_WRAP
                });
                let _ = layout.SetLineSpacing(
                    DWRITE_LINE_SPACING_METHOD_UNIFORM,
                    lh,
                    top_pad + face.ascent,
                );
            }
        }

        let (width, raw_lines) = layout.as_ref().map_or((0.0, 1), |l| {
            let mut m = DWRITE_TEXT_METRICS::default();
            let _ = unsafe { l.GetMetrics(&mut m) };
            (m.widthIncludingTrailingWhitespace, m.lineCount.max(1) as usize)
        });

        let (line_count, clamped) = match opts.line_clamp {
            Some(cap) if raw_lines > cap => {
                let clamped_lines = cap.max(1);
                if let Some(layout) = &layout {
                    unsafe {
                        let _ = layout.SetMaxHeight(lh * clamped_lines as f32);
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
                (clamped_lines, true)
            }
            _ => (raw_lines, false),
        };

        let label = Label {
            width,
            line_count,
            line_height: lh,
            layout_box_width: box_w,
            clamped,
            layout,
        };
        self.state.borrow_mut().labels.insert(key, label.clone(), 0);
        label
    }

    #[cfg(test)]
    pub(crate) fn label_cache_counts(&self) -> (usize, usize) {
        self.state.borrow().labels.age_counts()
    }
}

fn create_font_face(
    factory: &IDWriteFactory5,
    analyzer: &Option<IDWriteTextAnalyzer>,
    style: &TextStyle,
) -> FontFace {
    let safe_size = style.size();
    let family = HSTRING::from(style.font.family.as_str());
    let locale = HSTRING::from("");
    let dw_weight = style.font.weight.to_dwrite();
    let format = unsafe {
        factory
            .CreateTextFormat(
                &family,
                None,
                dw_weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                safe_size,
                &locale,
            )
            .or_else(|_| {
                let fallback = HSTRING::from("Segoe UI");
                factory.CreateTextFormat(
                    &fallback,
                    None,
                    DWRITE_FONT_WEIGHT(400),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    safe_size,
                    &locale,
                )
            })
    }
    .expect("CreateTextFormat fallback failed");
    let _ = unsafe { format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP) };
    let space_width = measure_utf16(factory, &[b' ' as u16], &format);
    if space_width > 0.0 {
        let _ = unsafe { format.SetIncrementalTabStop(space_width * TAB_STOP_COLUMNS) };
    }
    let sample = [b'H' as u16, b'g' as u16];
    let (ascent, descent) =
        unsafe { factory.CreateTextLayout(&sample, &format, f32::MAX, f32::MAX) }
            .ok()
            .map_or((safe_size * 0.8, safe_size * 0.2), |l| {
                let mut line_metrics = [DWRITE_LINE_METRICS::default(); 2];
                let mut count = 0u32;
                if unsafe { l.GetLineMetrics(Some(&mut line_metrics), &mut count) }.is_ok()
                    && count > 0
                {
                    let lm = line_metrics[0];
                    (lm.baseline, (lm.height - lm.baseline).max(0.0))
                } else {
                    (safe_size * 0.8, safe_size * 0.2)
                }
            });
    FontFace {
        factory: factory.clone(),
        analyzer: analyzer.clone(),
        format,
        ascent,
        descent,
        space_width,
        advances: RefCell::new(None),
    }
}

fn measure_utf16(
    factory: &IDWriteFactory5,
    utf16: &[u16],
    format: &IDWriteTextFormat,
) -> f32 {
    if utf16.is_empty() {
        return 0.0;
    }
    let Ok(layout) = (unsafe { factory.CreateTextLayout(utf16, format, f32::MAX, f32::MAX) })
    else {
        return 0.0;
    };
    let mut metrics = DWRITE_TEXT_METRICS::default();
    if unsafe { layout.GetMetrics(&mut metrics) }.is_ok() {
        metrics.widthIncludingTrailingWhitespace
    } else {
        0.0
    }
}

fn encode_single_line_utf16(text: &str) -> Vec<u16> {
    let mut out = Vec::with_capacity(text.len());
    for ch in text.chars() {
        let mapped = match ch {
            '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}' => '\u{25A1}',
            other => other,
        };
        let mut buf = [0u16; 2];
        out.extend_from_slice(mapped.encode_utf16(&mut buf));
    }
    out
}

#[inline]
pub(crate) fn utf8_to_utf16_offset(text: &str, utf8_offset: usize, is_ascii: bool) -> u32 {
    if is_ascii {
        return utf8_offset.min(text.len()) as u32;
    }
    let clamped = text.floor_char_boundary(utf8_offset.min(text.len()));
    text[..clamped].encode_utf16().count() as u32
}

#[inline]
pub(crate) fn utf16_to_utf8_offset(text: &str, utf16_offset: u32, is_ascii: bool) -> usize {
    if is_ascii {
        return (utf16_offset as usize).min(text.len());
    }
    let mut u16_acc = 0u32;
    for (byte_ix, ch) in text.char_indices() {
        if u16_acc >= utf16_offset {
            return byte_ix;
        }
        u16_acc += ch.len_utf16() as u32;
    }
    text.len()
}

const CHAR_WIDTH_RANGES: [RangeInclusive<u32>; 17] = [
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

#[inline]
fn is_printable_ascii_or_tab(bytes: &[u8]) -> bool {
    bytes.iter().all(|&b| b == b'\t' || (0x20..=0x7e).contains(&b))
}

fn can_wrap_with_advances(text: &str) -> bool {
    if is_printable_ascii_or_tab(text.as_bytes()) {
        return true;
    }
    text.chars()
        .all(|c| c == '\t' || CHAR_WIDTH_RANGES.iter().any(|r| r.contains(&(c as u32))))
}

struct AdvanceTable {
    factory: IDWriteFactory5,
    format: IDWriteTextFormat,
    ascii: [f32; 95],
    max_ascii: f32,
    non_ascii: HashMap<char, f32>,
}

impl AdvanceTable {
    fn new(factory: IDWriteFactory5, format: IDWriteTextFormat) -> Self {
        let mut ascii = [0.0f32; 95];
        let mut max_ascii = 0.0f32;
        let mut buf = [0u16; 1];
        for (i, byte) in (0x20u8..=0x7Eu8).enumerate() {
            buf[0] = u16::from(byte);
            let w = measure_utf16(&factory, &buf, &format);
            ascii[i] = w;
            if w > max_ascii {
                max_ascii = w;
            }
        }
        Self {
            factory,
            format,
            ascii,
            max_ascii,
            non_ascii: HashMap::new(),
        }
    }

    #[inline]
    fn max_printable_ascii(&self) -> f32 {
        self.max_ascii
    }

    #[inline]
    fn tab_stop_width(&self) -> f32 {
        (self.ascii[0] * TAB_STOP_COLUMNS).max(1.0)
    }

    #[inline]
    fn advance(&mut self, ch: char, current_x: f32) -> f32 {
        if ch == '\t' {
            let stop = self.tab_stop_width();
            let next = ((current_x / stop).floor() + 1.0) * stop;
            return (next - current_x).max(self.ascii[0].max(1.0));
        }
        let code = ch as u32;
        if (0x20..=0x7E).contains(&code) {
            return self.ascii[(code - 0x20) as usize];
        }
        if let Some(&w) = self.non_ascii.get(&ch) {
            return w;
        }
        let mut buf = [0u16; 2];
        let slice = ch.encode_utf16(&mut buf);
        let w = measure_utf16(&self.factory, slice, &self.format);
        self.non_ascii.insert(ch, w);
        w
    }

    fn horizontal_slice(
        &mut self,
        line: &str,
        min_x: f32,
        max_x: f32,
    ) -> (usize, usize, f32, f32) {
        let mut x = 0.0f32;
        let mut start_byte = 0usize;
        let mut start_x = 0.0f32;
        let mut found_start = false;
        let mut end_byte = line.len();
        let mut found_end = false;

        for (ix, ch) in line.char_indices() {
            let adv = self.advance(ch, x);
            if !found_start && x + adv >= min_x {
                start_byte = ix;
                start_x = x;
                found_start = true;
            }
            x += adv;
            if found_start && !found_end && x >= max_x {
                end_byte = ix + ch.len_utf8();
                found_end = true;
            }
        }
        if !found_start {
            start_byte = line.len();
            start_x = x;
            end_byte = line.len();
        }
        (start_byte, end_byte, start_x, x)
    }

    fn fast_breaks(
        &mut self,
        line: &str,
        wrap_width: f32,
        indent_px: f32,
        analyzer: Option<&IDWriteTextAnalyzer>,
        out: &mut Vec<usize>,
    ) {
        let break_offsets = analyzer
            .map(|a| analyze_break_opportunities(a, line))
            .unwrap_or_default();
        let mut row_start = 0usize;
        let mut row_width = 0.0f32;
        let mut last_break_ix = 0usize;
        let mut break_cursor = 0usize;

        for (ix, ch) in line.char_indices() {
            let char_end = ix + ch.len_utf8();
            while break_cursor < break_offsets.len() && break_offsets[break_cursor] <= ix {
                break_cursor += 1;
            }
            let w = self.advance(ch, row_width);
            if row_width + w > wrap_width && ix > row_start {
                let split_at = if last_break_ix > row_start {
                    last_break_ix
                } else {
                    ix
                };
                out.push(split_at);
                row_start = split_at;
                row_width = indent_px;
                for c in line[split_at..char_end].chars() {
                    row_width += self.advance(c, row_width);
                }
                last_break_ix = split_at;
            } else {
                row_width += w;
            }
            if break_cursor < break_offsets.len() && break_offsets[break_cursor] == char_end {
                last_break_ix = char_end;
                break_cursor += 1;
            }
        }
    }

    fn dwrite_breaks(
        &self,
        text: &str,
        first_width: f32,
        cont_width: f32,
        out: &mut Vec<usize>,
    ) {
        if text.is_empty() || !first_width.is_finite() || first_width <= 0.0 {
            return;
        }
        let utf16 = encode_single_line_utf16(text);
        if utf16.is_empty() {
            return;
        }

        let collect = |slice: &[u16], max_w: f32| -> Vec<usize> {
            let Ok(layout) = (unsafe {
                self.factory
                    .CreateTextLayout(slice, &self.format, max_w.max(1.0), f32::MAX)
            }) else {
                return Vec::new();
            };
            let _ = unsafe { layout.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP) };
            let mut m = DWRITE_TEXT_METRICS::default();
            if unsafe { layout.GetMetrics(&mut m) }.is_err() || m.lineCount <= 1 {
                return Vec::new();
            }
            let count = m.lineCount as usize;
            let mut metrics = vec![DWRITE_LINE_METRICS::default(); count];
            let mut actual = 0u32;
            let _ = unsafe { layout.GetLineMetrics(Some(&mut metrics), &mut actual) };
            let n = (actual as usize).min(metrics.len());
            if n <= 1 {
                return Vec::new();
            }
            let mut breaks = Vec::with_capacity(n - 1);
            let mut acc = 0usize;
            for lm in &metrics[..n - 1] {
                acc += lm.length as usize;
                if acc > 0 && acc < slice.len() {
                    breaks.push(acc);
                }
            }
            breaks
        };

        let mut u16_breaks = collect(&utf16, first_width);
        if u16_breaks.is_empty() {
            return;
        }
        if (cont_width - first_width).abs() > 0.01 {
            let first = u16_breaks[0];
            u16_breaks.truncate(1);
            if first < utf16.len() {
                for rel in collect(&utf16[first..], cont_width.max(1.0)) {
                    u16_breaks.push(first + rel);
                }
            }
        }

        let mut bi = 0usize;
        let mut u16_acc = 0usize;
        let mut last_byte = 0usize;
        for (byte_ix, ch) in text.char_indices() {
            while bi < u16_breaks.len() && u16_acc >= u16_breaks[bi] {
                if byte_ix > last_byte && byte_ix < text.len() {
                    out.push(byte_ix);
                    last_byte = byte_ix;
                }
                bi += 1;
            }
            if bi >= u16_breaks.len() {
                break;
            }
            u16_acc += ch.len_utf16();
        }
    }
}

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
        unsafe {
            *textlength = (self.utf16.len() - pos) as u32;
        }
        let _ = numbersubstitution.write(None);
        Ok(())
    }
}

#[implement(IDWriteTextAnalysisSink)]
struct BreakSink {
    out: Rc<RefCell<Vec<DWRITE_LINE_BREAKPOINT>>>,
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
        let start = textposition as usize;
        let len = textlength as usize;
        if len == 0 || linebreakpoints.is_null() {
            return Ok(());
        }
        let mut dst = self.out.borrow_mut();
        if start.saturating_add(len) <= dst.len() {
            let src = unsafe { std::slice::from_raw_parts(linebreakpoints, len) };
            dst[start..start + len].copy_from_slice(src);
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

fn analyze_break_opportunities(
    analyzer: &IDWriteTextAnalyzer,
    text: &str,
) -> Vec<usize> {
    let mut out = Vec::new();
    if text.is_empty() {
        return out;
    }

    let utf16: Rc<Vec<u16>> = Rc::new(text.encode_utf16().collect());
    let len = utf16.len();
    let breakpoints = Rc::new(RefCell::new(vec![DWRITE_LINE_BREAKPOINT::default(); len]));
    let source: IDWriteTextAnalysisSource = BreakSource {
        utf16,
        locale: HSTRING::from(""),
    }
    .into();
    let sink: IDWriteTextAnalysisSink = BreakSink {
        out: breakpoints.clone(),
    }
    .into();

    if unsafe { analyzer.AnalyzeLineBreakpoints(&source, 0, len as u32, &sink) }.is_err() {
        return out;
    }

    let bp = breakpoints.borrow();
    let can_break = DWRITE_BREAK_CONDITION_CAN_BREAK.0 as u8;
    let may_not_break = DWRITE_BREAK_CONDITION_MAY_NOT_BREAK.0 as u8;
    let must_break = DWRITE_BREAK_CONDITION_MUST_BREAK.0 as u8;
    let mut u16_pos = 0usize;
    for (byte_ix, ch) in text.char_indices() {
        let char_u16 = ch.len_utf16();
        let last_u16 = u16_pos + char_u16 - 1;
        let next_u16 = u16_pos + char_u16;
        let next_byte = byte_ix + ch.len_utf8();
        if next_byte < text.len() && next_u16 < bp.len() {
            let after = (bp[last_u16]._bitfield >> 2) & 0b11;
            let before = bp[next_u16]._bitfield & 0b11;
            let allowed = if after == must_break || before == must_break {
                true
            } else if after == may_not_break || before == may_not_break {
                false
            } else {
                after == can_break || before == can_break
            };
            if allowed {
                out.push(next_byte);
            }
        }
        u16_pos = next_u16;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let breaks = super::analyze_break_opportunities(analyzer, "hello world");
        assert!(breaks.contains(&6));
        assert!(
            super::analyze_break_opportunities(analyzer, "foo\u{00A0}bar")
                .is_empty()
        );
        assert_eq!(
            super::analyze_break_opportunities(analyzer, "foo-bar"),
            vec![4]
        );
    }

    #[test]
    fn wrapped_text_two_frame_cache_promotes_and_evicts() {
        use windows::core::Interface;

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
