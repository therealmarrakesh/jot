//! Direct3D 11 + DirectComposition + Direct2D 1.1 renderer.
//!
//! Binds `ID2D1DeviceContext` directly to a `CreateSwapChainForComposition` premultiplied
//! `B8G8R8A8_UNORM` swapchain for Windows 11 Mica Alt backdrop, transparent overlay HWNDs, and
//! vsync-paced `FLIP_SEQUENTIAL` presentation with zero HLSL shaders and zero glyph atlases.

use std::collections::HashMap;
use std::time::Instant;

use windows::Win32::{
    Foundation::HWND,
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
        DirectWrite::{DWRITE_TEXT_RANGE, IDWriteTextLayout},
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
use windows::core::Interface;
use windows_numerics::{Matrix3x2, Vector2};

use crate::color::Rgba;
use crate::geom::{Corners, Edges, Point, Rect, Shadows, Size, point, size};
use crate::types::{Error, Result};

const SWAPCHAIN_BUFFERS: u32 = 3;
const MAX_SHADOW_CACHE_ENTRIES: usize = 24;

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

/// Process-wide Direct3D 11 + DXGI + Direct2D 1.1 device objects shared across all Jot windows.
#[derive(Clone)]
pub(crate) struct Direct2DDevices {
    pub(crate) dxgi_factory: IDXGIFactory6,
    pub(crate) d3d_device: ID3D11Device,
    pub(crate) dxgi_device: IDXGIDevice,
    pub(crate) d2d_factory: ID2D1Factory1,
    pub(crate) d2d_device: ID2D1Device,
}

impl Direct2DDevices {
    pub(crate) fn new() -> Result<Self> {
        let dxgi_factory: IDXGIFactory6 =
            unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };
        let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let mut d3d_device: Option<ID3D11Device> = None;

        let hw = unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut d3d_device),
                None,
                None,
            )
        };
        if hw.is_err() {
            unsafe {
                D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_WARP,
                    windows::Win32::Foundation::HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&feature_levels),
                    D3D11_SDK_VERSION,
                    Some(&mut d3d_device),
                    None,
                    None,
                )?;
            }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ShadowCacheKey {
    width_px: u32,
    height_px: u32,
    radius_tenths: u32,
    blur_tenths: u32,
    color_bits: [u32; 4],
    dpi_hundredths: u32,
}

struct CachedShadow {
    bitmap: ID2D1Bitmap1,
    pad_dip: f32,
    last_used: Instant,
}

/// Filled/bordered rounded-rectangle primitive.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub(crate) struct PaintQuad {
    pub(crate) bounds: Rect,
    pub(crate) corner_radii: Corners,
    pub(crate) background: Rgba,
    pub(crate) border_widths: Edges,
    pub(crate) border_color: Rgba,
}

impl PaintQuad {
    #[inline]
    pub(crate) fn corner_radii(mut self, radii: Corners) -> Self {
        self.corner_radii = radii;
        self
    }
}

#[inline]
pub(crate) fn fill(bounds: Rect, background: Rgba) -> PaintQuad {
    PaintQuad {
        bounds,
        background,
        ..Default::default()
    }
}

/// Per-window Direct2D 1.1 + DirectComposition renderer bound to a premultiplied DXGI swapchain.
pub(crate) struct D2DRenderer {
    devices: Direct2DDevices,
    swap_chain: IDXGISwapChain1,
    _comp_device: IDCompositionDevice,
    _comp_target: IDCompositionTarget,
    _comp_visual: IDCompositionVisual,
    context: ID2D1DeviceContext,
    target_bitmap: Option<ID2D1Bitmap1>,
    solid_brush: ID2D1SolidColorBrush,
    shadow_effect: Option<ID2D1Effect>,
    shadow_cache: HashMap<ShadowCacheKey, CachedShadow>,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    clip_depth: usize,
    layer_depth: usize,
}

impl D2DRenderer {
    pub(crate) fn new(
        devices: Direct2DDevices,
        hwnd: HWND,
        physical_width: u32,
        physical_height: u32,
        scale_factor: f32,
    ) -> Result<Self> {
        let width = physical_width.max(1);
        let height = physical_height.max(1);

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

        let comp_device: IDCompositionDevice =
            unsafe { DCompositionCreateDevice(&devices.dxgi_device)? };
        let comp_target = unsafe { comp_device.CreateTargetForHwnd(hwnd, true)? };
        let comp_visual = unsafe { comp_device.CreateVisual()? };
        unsafe {
            comp_visual.SetContent(&swap_chain)?;
            comp_target.SetRoot(&comp_visual)?;
            comp_device.Commit()?;
        }

        let context = unsafe {
            devices
                .d2d_device
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?
        };
        unsafe {
            context.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            context.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        }

        let solid_brush = unsafe {
            context.CreateSolidColorBrush(&to_d2d_color(Rgba::new(0.0, 0.0, 0.0, 1.0)), None)?
        };
        let shadow_effect = unsafe { context.CreateEffect(&CLSID_D2D1Shadow) }.ok();

        let mut renderer = Self {
            devices,
            swap_chain,
            _comp_device: comp_device,
            _comp_target: comp_target,
            _comp_visual: comp_visual,
            context,
            target_bitmap: None,
            solid_brush,
            shadow_effect,
            shadow_cache: HashMap::new(),
            physical_width: width,
            physical_height: height,
            scale_factor,
            clip_depth: 0,
            layer_depth: 0,
        };
        renderer.attach_backbuffer()?;
        Ok(renderer)
    }

    fn attach_backbuffer(&mut self) -> Result<()> {
        let surface: IDXGISurface = unsafe { self.swap_chain.GetBuffer(0)? };
        let dpi = 96.0 * self.scale_factor;
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
            self.context
                .CreateBitmapFromDxgiSurface(&surface, Some(&props))?
        };
        unsafe {
            self.context.SetDpi(dpi, dpi);
            self.context.SetTarget(&bitmap);
        }
        self.target_bitmap = Some(bitmap);
        Ok(())
    }

    pub(crate) fn resize(
        &mut self,
        physical_width: u32,
        physical_height: u32,
        scale_factor: f32,
    ) -> Result<()> {
        let width = physical_width.max(1);
        let height = physical_height.max(1);
        if width == self.physical_width
            && height == self.physical_height
            && (scale_factor - self.scale_factor).abs() < f32::EPSILON
        {
            return Ok(());
        }
        self.physical_width = width;
        self.physical_height = height;
        if (scale_factor - self.scale_factor).abs() >= f32::EPSILON {
            self.scale_factor = scale_factor;
            self.shadow_cache.clear();
        }
        unsafe {
            self.context.SetTarget(None);
        }
        self.target_bitmap = None;
        unsafe {
            self.swap_chain.ResizeBuffers(
                SWAPCHAIN_BUFFERS,
                width,
                height,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_SWAP_CHAIN_FLAG(0),
            )?;
        }
        self.attach_backbuffer()
    }

    pub(crate) fn begin_frame(&mut self, clear_color: Rgba) {
        self.clip_depth = 0;
        self.layer_depth = 0;
        unsafe {
            self.context.BeginDraw();
            self.context.SetTransform(&Matrix3x2::identity());
            self.context.Clear(Some(&to_d2d_color(clear_color)));
        }
    }

    /// Ends the D2D frame and presents the swapchain. Returns `Ok(true)` if the GPU device was lost.
    pub(crate) fn end_frame(&mut self) -> Result<bool> {
        while self.clip_depth > 0 {
            self.pop_clip();
        }
        while self.layer_depth > 0 {
            self.pop_layer();
        }
        let end_res = unsafe { self.context.EndDraw(None, None) };
        if let Err(err) = end_res {
            let code = err.code();
            if code == DXGI_ERROR_DEVICE_REMOVED || code == DXGI_ERROR_DEVICE_RESET {
                return Ok(true);
            }
            return Err(err.into());
        }
        let hr = unsafe { self.swap_chain.Present(0, DXGI_PRESENT(0)) };
        if hr == DXGI_ERROR_DEVICE_REMOVED || hr == DXGI_ERROR_DEVICE_RESET {
            return Ok(true);
        }
        hr.ok()?;
        Ok(false)
    }

    pub(crate) fn push_clip(&mut self, bounds: Rect) {
        let rect = to_d2d_rect(bounds);
        unsafe {
            self.context
                .PushAxisAlignedClip(&rect, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
        self.clip_depth += 1;
    }

    pub(crate) fn pop_clip(&mut self) {
        if self.clip_depth > 0 {
            unsafe {
                self.context.PopAxisAlignedClip();
            }
            self.clip_depth -= 1;
        }
    }

    pub(crate) fn push_opacity_layer(&mut self, bounds: Rect, opacity: f32) {
        let params = D2D1_LAYER_PARAMETERS1 {
            contentBounds: to_d2d_rect(bounds),
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
        self.layer_depth += 1;
    }

    pub(crate) fn pop_layer(&mut self) {
        if self.layer_depth > 0 {
            unsafe {
                self.context.PopLayer();
            }
            self.layer_depth -= 1;
        }
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
        let geom = unsafe { self.devices.d2d_factory.CreatePathGeometry() }.ok()?;
        let sink = unsafe { geom.Open() }.ok()?;
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

        // 1. Background fill
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

        // 2. Border stroke
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
            let inset_rect = D2D_RECT_F {
                left: quad.bounds.left() + half,
                top: quad.bounds.top() + half,
                right: quad.bounds.right() - half,
                bottom: quad.bounds.bottom() - half,
            };
            let need_clip = !uniform_border;
            if need_clip {
                let clip_rect = D2D_RECT_F {
                    left: quad.bounds.left() + if blw <= 0.0 { stroke } else { 0.0 },
                    top: quad.bounds.top() + if bt <= 0.0 { stroke } else { 0.0 },
                    right: quad.bounds.right() - if brw <= 0.0 { stroke } else { 0.0 },
                    bottom: quad.bounds.bottom() - if bb <= 0.0 { stroke } else { 0.0 },
                };
                unsafe {
                    self.context
                        .PushAxisAlignedClip(&clip_rect, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                }
            }
            if max_r <= 0.0 {
                unsafe {
                    self.context
                        .DrawRectangle(&inset_rect, &self.solid_brush, stroke, None);
                }
            } else if uniform_radii {
                let r = (max_r - half).max(0.0);
                let rr = D2D1_ROUNDED_RECT {
                    rect: inset_rect,
                    radiusX: r,
                    radiusY: r,
                };
                unsafe {
                    self.context
                        .DrawRoundedRectangle(&rr, &self.solid_brush, stroke, None);
                }
            } else if let Some(geom) = self.create_per_corner_geometry(
                inset_rect,
                (tl - half).max(0.0),
                (tr - half).max(0.0),
                (br - half).max(0.0),
                (bl - half).max(0.0),
            ) {
                unsafe {
                    self.context
                        .DrawGeometry(&geom, &self.solid_brush, stroke, None);
                }
            }
            if need_clip {
                unsafe {
                    self.context.PopAxisAlignedClip();
                }
            }
        } else {
            let b = quad.bounds;
            let draw_edge = |rect: D2D_RECT_F| unsafe {
                self.context.FillRectangle(&rect, &self.solid_brush);
            };
            if bt > 0.0 {
                draw_edge(D2D_RECT_F {
                    left: b.left(),
                    top: b.top(),
                    right: b.right(),
                    bottom: b.top() + bt,
                });
            }
            if bb > 0.0 {
                draw_edge(D2D_RECT_F {
                    left: b.left(),
                    top: b.bottom() - bb,
                    right: b.right(),
                    bottom: b.bottom(),
                });
            }
            if blw > 0.0 {
                draw_edge(D2D_RECT_F {
                    left: b.left(),
                    top: b.top(),
                    right: b.left() + blw,
                    bottom: b.bottom(),
                });
            }
            if brw > 0.0 {
                draw_edge(D2D_RECT_F {
                    left: b.right() - brw,
                    top: b.top(),
                    right: b.right(),
                    bottom: b.bottom(),
                });
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
            let sb = Rect::new(
                point(
                    bounds.origin.x + shadow.offset.x - spread,
                    bounds.origin.y + shadow.offset.y - spread,
                ),
                size(
                    (bounds.size.width + spread * 2.0).max(1.0),
                    (bounds.size.height + spread * 2.0).max(1.0),
                ),
            );
            let blur = shadow.blur_radius.max(0.0);
            let radius = (max_r + spread).max(0.0);

            if let Some((bitmap, pad_dip)) =
                self.get_or_render_shadow(sb.size, radius, blur, shadow.color)
            {
                let dst = D2D_RECT_F {
                    left: sb.left() - pad_dip,
                    top: sb.top() - pad_dip,
                    right: sb.right() + pad_dip,
                    bottom: sb.bottom() + pad_dip,
                };
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
        let width_px = (shape_size.width * scale).round().max(1.0) as u32;
        let height_px = (shape_size.height * scale).round().max(1.0) as u32;
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
            dpi_hundredths: (scale * 100.0).round() as u32,
        };

        if let Some(entry) = self.shadow_cache.get_mut(&key) {
            entry.last_used = Instant::now();
            return Some((entry.bitmap.clone(), entry.pad_dip));
        }

        let effect = self.shadow_effect.clone()?;
        let sigma = (blur_radius * 0.5).max(0.5);
        let pad_dip = (sigma * 3.0).ceil();
        let total_w_dip = shape_size.width + pad_dip * 2.0;
        let total_h_dip = shape_size.height + pad_dip * 2.0;
        let total_w_px = (total_w_dip * scale).ceil().max(1.0) as u32;
        let total_h_px = (total_h_dip * scale).ceil().max(1.0) as u32;
        if total_w_px > 4096 || total_h_px > 4096 {
            return None;
        }

        let offscreen = unsafe {
            self.devices
                .d2d_device
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
        }
        .ok()?;
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
        let shape_bmp = unsafe {
            offscreen.CreateBitmap(
                D2D_SIZE_U {
                    width: total_w_px,
                    height: total_h_px,
                },
                None,
                0,
                &props,
            )
        }
        .ok()?;
        let blurred_bmp = unsafe {
            offscreen.CreateBitmap(
                D2D_SIZE_U {
                    width: total_w_px,
                    height: total_h_px,
                },
                None,
                0,
                &props,
            )
        }
        .ok()?;

        let white_brush = unsafe {
            offscreen.CreateSolidColorBrush(&to_d2d_color(Rgba::new(1.0, 1.0, 1.0, 1.0)), None)
        }
        .ok()?;

        unsafe {
            offscreen.SetDpi(dpi, dpi);
            offscreen.SetTarget(&shape_bmp);
            offscreen.BeginDraw();
            offscreen.Clear(Some(&to_d2d_color(Rgba::new(0.0, 0.0, 0.0, 0.0))));
            let rr = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left: pad_dip,
                    top: pad_dip,
                    right: pad_dip + shape_size.width,
                    bottom: pad_dip + shape_size.height,
                },
                radiusX: radius,
                radiusY: radius,
            };
            offscreen.FillRoundedRectangle(&rr, &white_brush);
            offscreen.EndDraw(None, None).ok()?;
        }

        let shadow_eff = unsafe { offscreen.CreateEffect(&CLSID_D2D1Shadow) }.unwrap_or(effect);
        let color_vec: [f32; 4] = [color.r, color.g, color.b, color.a];
        let color_bytes: [u8; 16] = unsafe { std::mem::transmute(color_vec) };
        unsafe {
            shadow_eff.SetInput(0, &shape_bmp, true);
            let _ = shadow_eff.SetValue(
                D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_UNKNOWN,
                &sigma.to_le_bytes(),
            );
            let _ = shadow_eff.SetValue(
                D2D1_SHADOW_PROP_COLOR.0 as u32,
                D2D1_PROPERTY_TYPE_UNKNOWN,
                &color_bytes,
            );
            offscreen.SetTarget(&blurred_bmp);
            offscreen.BeginDraw();
            offscreen.Clear(Some(&to_d2d_color(Rgba::new(0.0, 0.0, 0.0, 0.0))));
            let output = shadow_eff.GetOutput().ok()?;
            offscreen.DrawImage(
                &output,
                Some(&Vector2 { X: 0.0, Y: 0.0 }),
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
            offscreen.EndDraw(None, None).ok()?;
            offscreen.SetTarget(None);
        }

        if self.shadow_cache.len() >= MAX_SHADOW_CACHE_ENTRIES
            && let Some(oldest_key) = self
                .shadow_cache
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(k, _)| *k)
        {
            self.shadow_cache.remove(&oldest_key);
        }

        self.shadow_cache.insert(
            key,
            CachedShadow {
                bitmap: blurred_bmp.clone(),
                pad_dip,
                last_used: Instant::now(),
            },
        );
        Some((blurred_bmp, pad_dip))
    }

    pub(crate) fn paint_text_layout(
        &mut self,
        origin: Point,
        layout: &IDWriteTextLayout,
        default_color: Rgba,
        run_colors: &[(u32, u32, Rgba)],
    ) {
        if default_color.is_transparent() && run_colors.is_empty() {
            return;
        }
        for &(start, len, color) in run_colors {
            if color != default_color
                && let Ok(brush) =
                    unsafe { self.context.CreateSolidColorBrush(&to_d2d_color(color), None) }
            {
                let range = DWRITE_TEXT_RANGE {
                    startPosition: start,
                    length: len,
                };
                let _ = unsafe { layout.SetDrawingEffect(&brush, range) };
            }
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
