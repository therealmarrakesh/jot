#![deny(unsafe_op_in_unsafe_fn)]

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::{Rc, Weak},
    str::FromStr,
    sync::{Arc, Once, atomic::AtomicBool},
    time::{Duration, Instant},
};

use crate::ui::anyhow::{Context as _, Result};
use crate::ui::MainTaskQueue;
use crate::ui::smallvec::SmallVec;
use crate::ui::gpui_util::ResultExt;
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Dwm::*,
        Graphics::Gdi::*,
        System::{Com::*, LibraryLoader::*, Ole::*, SystemServices::*},
        UI::{HiDpi::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
    },
    core::*,
};

use crate::win32::direct_manipulation::DirectManipulationHandler;
use crate::win32::*;
use crate::ui::*;

pub(crate) struct WindowsWindow(pub(crate) Rc<WindowsWindowInner>);

impl std::ops::Deref for WindowsWindow {
    type Target = WindowsWindowInner;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub(crate) struct WindowsWindowState {
    pub(crate) origin: Cell<Point<Pixels>>,
    pub(crate) logical_size: Cell<Size<Pixels>>,
    pub(crate) min_size: Cell<Option<Size<Pixels>>>,
    pub(crate) in_size_move_loop: Cell<bool>,
    pub(crate) border_offset: WindowBorderOffset,
    pub(crate) appearance: Cell<WindowAppearance>,
    pub(crate) background_appearance: Cell<WindowBackgroundAppearance>,
    pub(crate) scale_factor: Cell<f32>,
    pub(crate) restore_from_minimized: Cell<Option<RequestFrameCallback>>,

    pub(crate) callbacks: Callbacks,
    pub(crate) input_handler: Cell<Option<PlatformInputHandler>>,
    pub(crate) ime_enabled: Cell<bool>,
    pub(crate) pending_surrogate: Cell<Option<u16>>,
    pub(crate) last_reported_modifiers: Cell<Option<Modifiers>>,
    pub(crate) last_reported_capslock: Cell<Option<Capslock>>,
    pub(crate) hovered: Cell<bool>,
    pub(crate) direct_manipulation: DirectManipulationHandler,

    pub(crate) renderer: RefCell<DirectXRenderer>,
    pub(crate) force_render_pending: Cell<bool>,

    pub(crate) click_state: ClickState,
    pub(crate) current_cursor: Cell<Option<HCURSOR>>,
    /// Shared with [`WindowsPlatformState::cursor_visible`].
    pub(crate) cursor_visible: Arc<AtomicBool>,
    pub(crate) nc_button_pressed: Cell<Option<u32>>,

    pub(crate) display: Cell<WindowsDisplay>,
    /// Flag to instruct the `VSyncProvider` thread to invalidate the directx devices
    /// as resizing them has failed, causing us to have lost at least the render target.
    pub(crate) invalidate_devices: Arc<AtomicBool>,
    /// Shared with [`WindowsPlatformState::draw_coordinator`] and every other window.
    pub(crate) draw_coordinator: Rc<DrawCoordinator>,
    hwnd: HWND,
}

pub(crate) struct WindowsWindowInner {
    hwnd: HWND,
    pub(crate) dialog_owner: Rc<crate::win32::dialog::DialogOwner>,
    drop_target_helper: IDropTargetHelper,
    pub(crate) state: WindowsWindowState,
    system_settings: WindowsSystemSettings,
    pub(crate) handle: AnyWindowHandle,
    pub(crate) is_overlay: bool,
    pub(crate) hide_title_bar: bool,
    pub(crate) is_movable: bool,
    pub(crate) is_resizable: bool,
    pub(crate) is_minimizable: bool,
    pub(crate) executor: ForegroundExecutor,
    pub(crate) validation_number: usize,
    pub(crate) main_queue: MainTaskQueue,
    pub(crate) platform_window_handle: HWND,
}

impl WindowsWindowState {
    pub(crate) fn pointer_position(&self, hwnd: HWND) -> Point<Pixels> {
        if self.in_size_move_loop.get() {
            return point(px(-1.), px(-1.));
        }
        let cursor = unsafe {
            let mut cursor = POINT::default();
            GetCursorPos(&mut cursor)
                .context("unable to get cursor position")
                .log_err();
            ScreenToClient(hwnd, &mut cursor).ok().log_err();
            cursor
        };
        logical_point(cursor.x as f32, cursor.y as f32, self.scale_factor.get())
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        hwnd: HWND,
        directx_devices: &DirectXDevices,
        window_params: &CREATESTRUCTW,
        current_cursor: Option<HCURSOR>,
        cursor_visible: Arc<AtomicBool>,
        display: WindowsDisplay,
        min_size: Option<Size<Pixels>>,
        appearance: WindowAppearance,
        disable_direct_composition: bool,
        invalidate_devices: Arc<AtomicBool>,
        draw_coordinator: Rc<DrawCoordinator>,
    ) -> Result<Self> {
        let scale_factor = {
            let monitor_dpi = unsafe { GetDpiForWindow(hwnd) } as f32;
            monitor_dpi / USER_DEFAULT_SCREEN_DPI as f32
        };
        let origin = logical_point(window_params.x as f32, window_params.y as f32, scale_factor);
        let logical_size = {
            let physical_size = size(
                DevicePixels(window_params.cx),
                DevicePixels(window_params.cy),
            );
            physical_size.to_pixels(scale_factor)
        };
        let border_offset = WindowBorderOffset::default();
        let restore_from_minimized = None;
        let renderer = DirectXRenderer::new(hwnd, directx_devices, disable_direct_composition)
            .context("Creating DirectX renderer")?;
        let callbacks = Callbacks::default();
        let input_handler = None;
        let pending_surrogate = None;
        let last_reported_modifiers = None;
        let last_reported_capslock = None;
        let hovered = false;
        let click_state = ClickState::new();
        let nc_button_pressed = None;

        let direct_manipulation = DirectManipulationHandler::new(hwnd, scale_factor)
            .context("initializing Direct Manipulation")?;

        Ok(Self {
            origin: Cell::new(origin),
            logical_size: Cell::new(logical_size),
            border_offset,
            appearance: Cell::new(appearance),
            background_appearance: Cell::new(WindowBackgroundAppearance::Opaque),
            scale_factor: Cell::new(scale_factor),
            restore_from_minimized: Cell::new(restore_from_minimized),
            min_size: Cell::new(min_size),
            in_size_move_loop: Cell::new(false),
            callbacks,
            input_handler: Cell::new(input_handler),
            ime_enabled: Cell::new(true),
            pending_surrogate: Cell::new(pending_surrogate),
            last_reported_modifiers: Cell::new(last_reported_modifiers),
            last_reported_capslock: Cell::new(last_reported_capslock),
            hovered: Cell::new(hovered),
            renderer: RefCell::new(renderer),
            force_render_pending: Cell::new(false),
            click_state,
            current_cursor: Cell::new(current_cursor),
            cursor_visible,
            nc_button_pressed: Cell::new(nc_button_pressed),
            display: Cell::new(display),
            hwnd,
            invalidate_devices,
            draw_coordinator,
            direct_manipulation,
        })
    }

    pub(crate) fn is_maximized(&self) -> bool {
        unsafe { IsZoomed(self.hwnd) }.as_bool()
    }

    fn bounds(&self) -> Bounds<Pixels> {
        Bounds {
            origin: self.origin.get(),
            size: self.logical_size.get(),
        }
    }

    // Calculate the bounds used for saving and whether the window is maximized.
    fn calculate_window_bounds(&self) -> (Bounds<Pixels>, bool) {
        let placement = unsafe {
            let mut placement = WINDOWPLACEMENT {
                length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };
            GetWindowPlacement(self.hwnd, &mut placement)
                .context("failed to get window placement")
                .log_err();
            placement
        };
        (
            calculate_client_rect(
                placement.rcNormalPosition,
                &self.border_offset,
                self.scale_factor.get(),
            ),
            placement.showCmd == SW_SHOWMAXIMIZED.0 as u32,
        )
    }

    fn window_bounds(&self) -> WindowBounds {
        let (bounds, maximized) = self.calculate_window_bounds();

        if maximized {
            WindowBounds::Maximized(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        }
    }

    /// get the logical size of the app's drawable area.
    fn content_size(&self) -> Size<Pixels> {
        self.logical_size.get()
    }
}

impl WindowsWindowInner {
    fn new(context: &mut WindowCreateContext, hwnd: HWND, cs: &CREATESTRUCTW) -> Result<Rc<Self>> {
        let state = WindowsWindowState::new(
            hwnd,
            &context.directx_devices,
            cs,
            context.current_cursor,
            context.cursor_visible.clone(),
            context.display,
            context.min_size,
            context.appearance,
            context.disable_direct_composition,
            context.invalidate_devices.clone(),
            context.draw_coordinator.clone(),
        )?;

        Ok(Rc::new(Self {
            hwnd,
            dialog_owner: crate::win32::dialog::DialogOwner::new(hwnd),
            drop_target_helper: context.drop_target_helper.clone(),
            state,
            handle: context.handle,
            is_overlay: context.is_overlay,
            hide_title_bar: context.hide_title_bar,
            is_movable: context.is_movable,
            is_resizable: context.is_resizable,
            is_minimizable: context.is_minimizable,
            executor: context.executor.clone(),
            validation_number: context.validation_number,
            main_queue: context.main_queue.clone(),
            platform_window_handle: context.platform_window_handle,
            system_settings: WindowsSystemSettings::new(),
        }))
    }

    pub(crate) fn system_settings(&self) -> &WindowsSystemSettings {
        &self.system_settings
    }
}

pub(crate) type RequestFrameCallback = Box<dyn FnMut(RequestFrameOptions)>;
pub(crate) type InputCallback = Box<dyn FnMut(PlatformInput) -> DispatchEventResult>;
pub(crate) type BoolCallback = Box<dyn FnMut(bool)>;
pub(crate) type ResizeCallback = Box<dyn FnMut(Size<Pixels>, f32)>;
pub(crate) type NotifyCallback = Box<dyn FnMut()>;
pub(crate) type ShouldCloseCallback = Box<dyn FnMut() -> bool>;
pub(crate) type CloseCallback = Box<dyn FnOnce()>;
pub(crate) type HitTestWindowControlCallback = Box<dyn FnMut() -> Option<WindowControlArea>>;

#[derive(Default)]
pub(crate) struct Callbacks {
    pub(crate) request_frame: Cell<Option<RequestFrameCallback>>,
    pub(crate) input: Cell<Option<InputCallback>>,
    pub(crate) active_status_change: Cell<Option<BoolCallback>>,
    pub(crate) hovered_status_change: Cell<Option<BoolCallback>>,
    pub(crate) resize: Cell<Option<ResizeCallback>>,
    pub(crate) moved: Cell<Option<NotifyCallback>>,
    pub(crate) should_close: Cell<Option<ShouldCloseCallback>>,
    pub(crate) close: Cell<Option<CloseCallback>>,
    pub(crate) hit_test_window_control: Cell<Option<HitTestWindowControlCallback>>,
    pub(crate) appearance_changed: Cell<Option<NotifyCallback>>,
}

struct WindowCreateContext {
    inner: Option<Result<Rc<WindowsWindowInner>>>,
    handle: AnyWindowHandle,
    is_overlay: bool,
    hide_title_bar: bool,
    display: WindowsDisplay,
    is_movable: bool,
    is_resizable: bool,
    is_minimizable: bool,
    min_size: Option<Size<Pixels>>,
    executor: ForegroundExecutor,
    current_cursor: Option<HCURSOR>,
    cursor_visible: Arc<AtomicBool>,
    drop_target_helper: IDropTargetHelper,
    validation_number: usize,
    main_queue: MainTaskQueue,
    platform_window_handle: HWND,
    appearance: WindowAppearance,
    disable_direct_composition: bool,
    directx_devices: DirectXDevices,
    invalidate_devices: Arc<AtomicBool>,
    draw_coordinator: Rc<DrawCoordinator>,
}

impl WindowsWindow {
    pub(crate) fn new(
        handle: AnyWindowHandle,
        params: WindowParams,
        creation_info: WindowCreationInfo,
    ) -> Result<Self> {
        let WindowCreationInfo {
            icon,
            executor,
            current_cursor,
            cursor_visible,
            drop_target_helper,
            validation_number,
            main_queue,
            platform_window_handle,
            disable_direct_composition,
            directx_devices,
            invalidate_devices,
            draw_coordinator,
        } = creation_info;
        register_window_class(icon);
        let is_overlay = params.kind == WindowKind::Overlay;
        let hide_title_bar = params
            .titlebar
            .as_ref()
            .map(|titlebar| titlebar.appears_transparent)
            .unwrap_or(true);
        let window_name = HSTRING::from(
            params
                .titlebar
                .as_ref()
                .and_then(|titlebar| titlebar.title.as_ref())
                .map(|title| title.as_ref())
                .unwrap_or(""),
        );

        let (mut dwexstyle, dwstyle) = if is_overlay {
            (
                WS_EX_TOOLWINDOW
                    | WS_EX_TOPMOST
                    | WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_NOACTIVATE,
                WS_POPUP,
            )
        } else {
            let mut dwstyle = WS_SYSMENU;
            if params.is_resizable {
                dwstyle |= WS_THICKFRAME | WS_MAXIMIZEBOX;
            }
            if params.is_minimizable {
                dwstyle |= WS_MINIMIZEBOX;
            }
            (WS_EX_APPWINDOW, dwstyle)
        };
        if !disable_direct_composition {
            dwexstyle |= WS_EX_NOREDIRECTIONBITMAP;
        }

        let hinstance = get_module_handle();
        let display = if let Some(display_id) = params.display_id {
            WindowsDisplay::new(display_id)
        } else {
            None
        }
        .or_else(WindowsDisplay::primary_monitor)
        .context("failed to find any monitor")?;
        let appearance = app_appearance().unwrap_or_default();
        let mut context = WindowCreateContext {
            inner: None,
            handle,
            is_overlay,
            hide_title_bar,
            display,
            is_movable: params.is_movable,
            is_resizable: params.is_resizable,
            is_minimizable: params.is_minimizable,
            min_size: params.window_min_size,
            executor,
            current_cursor,
            cursor_visible,
            drop_target_helper,
            validation_number,
            main_queue,
            platform_window_handle,
            appearance,
            disable_direct_composition,
            directx_devices,
            invalidate_devices,
            draw_coordinator,
        };
        let creation_result = unsafe {
            CreateWindowExW(
                dwexstyle,
                WINDOW_CLASS_NAME,
                &window_name,
                dwstyle,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                Some(hinstance.into()),
                Some(&context as *const _ as *const _),
            )
        };

        // Failure to create a `WindowsWindowState` can cause window creation to fail,
        // so check the inner result first.
        let this = context.inner.take().transpose()?;
        let hwnd = creation_result?;
        let this = this.unwrap();

        if is_overlay {
            let set_dwm = |attribute: DWMWINDOWATTRIBUTE, value: u32| {
                let _ = unsafe {
                    DwmSetWindowAttribute(
                        hwnd,
                        attribute,
                        (&raw const value).cast(),
                        std::mem::size_of::<u32>() as u32,
                    )
                };
            };
            set_dwm(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND.0 as u32);
            set_dwm(DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE);
            set_dwm(DWMWA_TRANSITIONS_FORCEDISABLED, 1);
            unsafe {
                let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            }
        } else {
            register_drag_drop(&this)?;
            set_non_rude_hwnd(hwnd, true);
            configure_dwm_dark_mode(hwnd, appearance);
        }
        this.state.border_offset.update(hwnd)?;
        let placement =
            retrieve_window_placement(hwnd, display, params.bounds, &this.state.border_offset)?;
        if params.show {
            let mut placement = placement;
            if !params.focus {
                placement.showCmd = SW_SHOWNOACTIVATE.0 as u32;
            }
            unsafe { SetWindowPlacement(hwnd, &placement)? };
        }

        Ok(Self(this))
    }
}

impl Drop for WindowsWindow {
    fn drop(&mut self) {
        self.0.dialog_owner.close();
        unsafe { ShowWindowAsync(self.0.hwnd, SW_HIDE).ok().log_err() };
        let this = self.0.clone();
        self.0
            .executor
            .spawn(async move {
                this.dialog_owner.when_idle().await;
                let handle = this.hwnd;
                unsafe {
                    if !this.is_overlay {
                        RevokeDragDrop(handle).log_err();
                    }
                    DestroyWindow(handle).log_err();
                }
            })
            .detach();
    }
}

impl WindowsWindow {
    pub(crate) fn bounds(&self) -> Bounds<Pixels> {
        self.state.bounds()
    }

    pub(crate) fn raw_hwnd(&self) -> Option<isize> {
        Some(self.0.hwnd.0 as isize)
    }

    pub(crate) fn is_maximized(&self) -> bool {
        self.state.is_maximized()
    }

    pub(crate) fn window_bounds(&self) -> WindowBounds {
        self.state.window_bounds()
    }

    pub(crate) fn content_size(&self) -> Size<Pixels> {
        self.state.content_size()
    }

    pub(crate) fn scale_factor(&self) -> f32 {
        self.state.scale_factor.get()
    }

    pub(crate) fn appearance(&self) -> WindowAppearance {
        self.state.appearance.get()
    }

    pub(crate) fn mouse_position(&self) -> Point<Pixels> {
        self.0.state.pointer_position(self.0.hwnd)
    }

    pub(crate) fn modifiers(&self) -> Modifiers {
        current_modifiers()
    }

    pub(crate) fn capslock(&self) -> Capslock {
        current_capslock()
    }

    pub(crate) fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        self.state.input_handler.set(Some(input_handler));
    }

    pub(crate) fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.state.input_handler.take()
    }

    pub(crate) fn is_active(&self) -> bool {
        self.0.hwnd == unsafe { GetActiveWindow() }
    }

    pub(crate) fn is_hovered(&self) -> bool {
        self.state.hovered.get()
    }

    pub(crate) fn set_title(&mut self, title: &str) {
        unsafe { SetWindowTextW(self.0.hwnd, &HSTRING::from(title)) }
            .inspect_err(|e| crate::ui::log::error!("Set title failed: {e}"))
            .ok();
    }

    pub(crate) fn set_background_appearance(
        &self,
        background_appearance: WindowBackgroundAppearance,
    ) {
        self.state.background_appearance.set(background_appearance);
        if self.0.is_overlay {
            return;
        }
        let hwnd = self.0.hwnd;

        match background_appearance {
            WindowBackgroundAppearance::Opaque => {
                set_window_composition_attribute(hwnd, None, 0);
            }
            WindowBackgroundAppearance::Transparent => {
                set_window_composition_attribute(hwnd, None, 2);
            }
            WindowBackgroundAppearance::MicaAltBackdrop => {
                dwm_set_window_composition_attribute(hwnd, 4);
            }
        }
    }

    pub(crate) fn zoom(&self) {
        unsafe {
            if IsWindowVisible(self.0.hwnd).as_bool() {
                ShowWindowAsync(self.0.hwnd, SW_MAXIMIZE).ok().log_err();
            }
        }
    }

    pub(crate) fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.state.callbacks.request_frame.set(Some(callback));
    }

    pub(crate) fn on_input(
        &self,
        callback: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>,
    ) {
        self.state.callbacks.input.set(Some(callback));
    }

    pub(crate) fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0
            .state
            .callbacks
            .active_status_change
            .set(Some(callback));
    }

    pub(crate) fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0
            .state
            .callbacks
            .hovered_status_change
            .set(Some(callback));
    }

    pub(crate) fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.state.callbacks.resize.set(Some(callback));
    }

    pub(crate) fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.state.callbacks.moved.set(Some(callback));
    }

    pub(crate) fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.state.callbacks.should_close.set(Some(callback));
    }

    pub(crate) fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.state.callbacks.close.set(Some(callback));
    }

    pub(crate) fn on_hit_test_window_control(
        &self,
        callback: Box<dyn FnMut() -> Option<WindowControlArea>>,
    ) {
        self.0
            .state
            .callbacks
            .hit_test_window_control
            .set(Some(callback));
    }

    pub(crate) fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.0
            .state
            .callbacks
            .appearance_changed
            .set(Some(callback));
    }

    pub(crate) fn draw(&self, scene: &Scene) {
        self.state
            .renderer
            .borrow_mut()
            .draw(scene, self.state.background_appearance.get())
            .log_err();
    }

    pub(crate) fn sprite_atlas(&self) -> Arc<DirectXAtlas> {
        self.state.renderer.borrow().sprite_atlas()
    }

    pub(crate) fn get_raw_handle(&self) -> HWND {
        self.0.hwnd
    }
}

#[implement(IDropTarget)]
struct WindowsDragDropHandler(pub(crate) Rc<WindowsWindowInner>);

impl WindowsDragDropHandler {
    fn handle_drag_drop(&self, input: PlatformInput) {
        if let Some(mut func) = self.0.state.callbacks.input.take() {
            func(input);
            self.0.state.callbacks.input.set(Some(func));
        }
    }
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for WindowsDragDropHandler_Impl {
    fn DragEnter(
        &self,
        pdataobj: windows::core::Ref<IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        unsafe {
            let idata_obj = pdataobj.ok()?;
            let config = FORMATETC {
                cfFormat: CF_HDROP.0,
                ptd: std::ptr::null_mut() as _,
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as _,
            };
            let cursor_position = POINT { x: pt.x, y: pt.y };
            if idata_obj.QueryGetData(&config as _) == S_OK {
                *pdweffect = DROPEFFECT_COPY;
                let Some(mut idata) = idata_obj.GetData(&config as _).log_err() else {
                    return Ok(());
                };
                if idata.u.hGlobal.is_invalid() {
                    return Ok(());
                }
                let hdrop = HDROP(idata.u.hGlobal.0);
                let mut paths = SmallVec::<[PathBuf; 2]>::new();
                with_file_names(hdrop, |file_name| {
                    if let Some(path) = PathBuf::from_str(&file_name).log_err() {
                        paths.push(path);
                    }
                });
                ReleaseStgMedium(&mut idata);
                let mut cursor_position = cursor_position;
                ScreenToClient(self.0.hwnd, &mut cursor_position)
                    .ok()
                    .log_err();
                let scale_factor = self.0.state.scale_factor.get();
                let input = PlatformInput::FileDrop(FileDropEvent::Entered {
                    position: logical_point(
                        cursor_position.x as f32,
                        cursor_position.y as f32,
                        scale_factor,
                    ),
                    paths: ExternalPaths(paths),
                });
                self.handle_drag_drop(input);
            } else {
                *pdweffect = DROPEFFECT_NONE;
            }
            self.0
                .drop_target_helper
                .DragEnter(self.0.hwnd, idata_obj, &cursor_position, *pdweffect)
                .log_err();
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let mut cursor_position = POINT { x: pt.x, y: pt.y };
        unsafe {
            *pdweffect = DROPEFFECT_COPY;
            self.0
                .drop_target_helper
                .DragOver(&cursor_position, *pdweffect)
                .log_err();
            ScreenToClient(self.0.hwnd, &mut cursor_position)
                .ok()
                .log_err();
        }
        let scale_factor = self.0.state.scale_factor.get();
        let input = PlatformInput::FileDrop(FileDropEvent::Pending {
            position: logical_point(
                cursor_position.x as f32,
                cursor_position.y as f32,
                scale_factor,
            ),
        });
        self.handle_drag_drop(input);

        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        unsafe {
            self.0.drop_target_helper.DragLeave().log_err();
        }
        let input = PlatformInput::FileDrop(FileDropEvent::Exited);
        self.handle_drag_drop(input);

        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: windows::core::Ref<IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let idata_obj = pdataobj.ok()?;
        let mut cursor_position = POINT { x: pt.x, y: pt.y };
        unsafe {
            *pdweffect = DROPEFFECT_COPY;
            self.0
                .drop_target_helper
                .Drop(idata_obj, &cursor_position, *pdweffect)
                .log_err();
            ScreenToClient(self.0.hwnd, &mut cursor_position)
                .ok()
                .log_err();
        }
        let scale_factor = self.0.state.scale_factor.get();
        let input = PlatformInput::FileDrop(FileDropEvent::Submit {
            position: logical_point(
                cursor_position.x as f32,
                cursor_position.y as f32,
                scale_factor,
            ),
        });
        self.handle_drag_drop(input);

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ClickState {
    button: Cell<MouseButton>,
    last_click: Cell<Instant>,
    last_position: Cell<Point<DevicePixels>>,
    double_click_spatial_tolerance_width: Cell<i32>,
    double_click_spatial_tolerance_height: Cell<i32>,
    double_click_interval: Cell<Duration>,
    pub(crate) current_count: Cell<usize>,
}

impl ClickState {
    pub(crate) fn new() -> Self {
        let double_click_spatial_tolerance_width = unsafe { GetSystemMetrics(SM_CXDOUBLECLK) };
        let double_click_spatial_tolerance_height = unsafe { GetSystemMetrics(SM_CYDOUBLECLK) };
        let double_click_interval = Duration::from_millis(unsafe { GetDoubleClickTime() } as u64);

        ClickState {
            button: Cell::new(MouseButton::Left),
            last_click: Cell::new(Instant::now()),
            last_position: Cell::new(Point::default()),
            double_click_spatial_tolerance_width: Cell::new(double_click_spatial_tolerance_width),
            double_click_spatial_tolerance_height: Cell::new(double_click_spatial_tolerance_height),
            double_click_interval: Cell::new(double_click_interval),
            current_count: Cell::new(0),
        }
    }

    /// update self and return the needed click count
    pub(crate) fn update(&self, button: MouseButton, new_position: Point<DevicePixels>) -> usize {
        if self.button.get() == button && self.is_double_click(new_position) {
            self.current_count.update(|it| it + 1);
        } else {
            self.current_count.set(1);
        }
        self.last_click.set(Instant::now());
        self.last_position.set(new_position);
        self.button.set(button);

        self.current_count.get()
    }

    pub(crate) fn system_update(&self, wparam: usize) {
        match wparam {
            // SPI_SETDOUBLECLKWIDTH
            29 => self
                .double_click_spatial_tolerance_width
                .set(unsafe { GetSystemMetrics(SM_CXDOUBLECLK) }),
            // SPI_SETDOUBLECLKHEIGHT
            30 => self
                .double_click_spatial_tolerance_height
                .set(unsafe { GetSystemMetrics(SM_CYDOUBLECLK) }),
            // SPI_SETDOUBLECLICKTIME
            32 => self
                .double_click_interval
                .set(Duration::from_millis(unsafe { GetDoubleClickTime() } as u64)),
            _ => {}
        }
    }

    #[inline]
    fn is_double_click(&self, new_position: Point<DevicePixels>) -> bool {
        let diff = self.last_position.get() - new_position;

        self.last_click.get().elapsed() < self.double_click_interval.get()
            && diff.x.0.abs() <= self.double_click_spatial_tolerance_width.get()
            && diff.y.0.abs() <= self.double_click_spatial_tolerance_height.get()
    }
}

#[allow(clippy::upper_case_acronyms)]
#[repr(C)]
struct WINDOWCOMPOSITIONATTRIBDATA {
    attrib: u32,
    pv_data: *mut std::ffi::c_void,
    cb_data: usize,
}

#[repr(C)]
struct AccentPolicy {
    accent_state: u32,
    accent_flags: u32,
    gradient_color: u32,
    animation_id: u32,
}

type Color = (u8, u8, u8, u8);

#[derive(Debug, Default, Clone)]
pub(crate) struct WindowBorderOffset {
    pub(crate) width_offset: Cell<i32>,
    pub(crate) height_offset: Cell<i32>,
}

impl WindowBorderOffset {
    pub(crate) fn update(&self, hwnd: HWND) -> crate::ui::anyhow::Result<()> {
        let window_rect = unsafe {
            let mut rect = std::mem::zeroed();
            GetWindowRect(hwnd, &mut rect)?;
            rect
        };
        let client_rect = unsafe {
            let mut rect = std::mem::zeroed();
            GetClientRect(hwnd, &mut rect)?;
            rect
        };
        self.width_offset
            .set((window_rect.right - window_rect.left) - (client_rect.right - client_rect.left));
        self.height_offset
            .set((window_rect.bottom - window_rect.top) - (client_rect.bottom - client_rect.top));
        Ok(())
    }
}

const WINDOW_CLASS_NAME: PCWSTR = w!("Jot::Window");

fn register_window_class(icon_handle: HICON) {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_procedure),
            hIcon: icon_handle,
            lpszClassName: PCWSTR(WINDOW_CLASS_NAME.as_ptr()),
            style: CS_HREDRAW | CS_VREDRAW,
            hInstance: get_module_handle().into(),
            hbrBackground: unsafe { CreateSolidBrush(COLORREF(0x00000000)) },
            ..Default::default()
        };
        unsafe { RegisterClassW(&wc) };
    });
}

unsafe extern "system" fn window_procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let window_params = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        let window_creation_context = window_params.lpCreateParams as *mut WindowCreateContext;
        let window_creation_context = unsafe { &mut *window_creation_context };
        return match WindowsWindowInner::new(window_creation_context, hwnd, window_params) {
            Ok(window_state) => {
                let weak = Box::new(Rc::downgrade(&window_state));
                unsafe { set_window_long(hwnd, GWLP_USERDATA, Box::into_raw(weak) as isize) };
                window_creation_context.inner = Some(Ok(window_state));
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            Err(error) => {
                window_creation_context.inner = Some(Err(error));
                LRESULT(0)
            }
        };
    }

    let ptr = unsafe { get_window_long(hwnd, GWLP_USERDATA) } as *mut Weak<WindowsWindowInner>;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let inner = unsafe { &*ptr };
    let result = if let Some(inner) = inner.upgrade() {
        if msg == WM_NCDESTROY {
            inner.dialog_owner.close();
        }
        inner.handle_msg(hwnd, msg, wparam, lparam)
    } else {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    };

    if msg == WM_NCDESTROY {
        unsafe { set_window_long(hwnd, GWLP_USERDATA, 0) };
        unsafe { drop(Box::from_raw(ptr)) };
    }

    result
}

pub(crate) fn set_window_min_size(window: isize, size: Option<Size<Pixels>>) {
    if let Some(window) = window_from_hwnd(HWND(window as _)) {
        window.state.min_size.set(size);
    }
}

pub(crate) fn window_from_hwnd(hwnd: HWND) -> Option<Rc<WindowsWindowInner>> {
    if hwnd.is_invalid() {
        return None;
    }

    let ptr = unsafe { get_window_long(hwnd, GWLP_USERDATA) } as *mut Weak<WindowsWindowInner>;
    if !ptr.is_null() {
        let inner = unsafe { &*ptr };
        inner.upgrade()
    } else {
        None
    }
}

fn get_module_handle() -> HMODULE {
    unsafe {
        let mut h_module = std::mem::zeroed();
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            windows::core::w!("JotModule"),
            &mut h_module,
        )
        .expect("Unable to get module handle"); // this should never fail

        h_module
    }
}

fn register_drag_drop(window: &Rc<WindowsWindowInner>) -> Result<()> {
    let window_handle = window.hwnd;
    let handler = WindowsDragDropHandler(window.clone());
    // The lifetime of `IDropTarget` is handled by Windows, it won't release until
    // we call `RevokeDragDrop`.
    // So, it's safe to drop it here.
    let drag_drop_handler: IDropTarget = handler.into();
    unsafe {
        RegisterDragDrop(window_handle, &drag_drop_handler)
            .context("unable to register drag-drop event")?;
    }
    Ok(())
}

fn calculate_window_rect(bounds: Bounds<DevicePixels>, border_offset: &WindowBorderOffset) -> RECT {
    // NOTE:
    // The reason we're not using `AdjustWindowRectEx()` here is
    // that the size reported by this function is incorrect.
    // You can test it, and there are similar discussions online.
    // See: https://stackoverflow.com/questions/12423584/how-to-set-exact-client-size-for-overlapped-window-winapi
    //
    // So we manually calculate these values here.
    let mut rect = RECT {
        left: bounds.left().0,
        top: bounds.top().0,
        right: bounds.right().0,
        bottom: bounds.bottom().0,
    };
    let left_offset = border_offset.width_offset.get() / 2;
    let top_offset = border_offset.height_offset.get() / 2;
    let right_offset = border_offset.width_offset.get() - left_offset;
    let bottom_offset = border_offset.height_offset.get() - top_offset;
    rect.left -= left_offset;
    rect.top -= top_offset;
    rect.right += right_offset;
    rect.bottom += bottom_offset;
    rect
}

fn calculate_client_rect(
    rect: RECT,
    border_offset: &WindowBorderOffset,
    scale_factor: f32,
) -> Bounds<Pixels> {
    let left_offset = border_offset.width_offset.get() / 2;
    let top_offset = border_offset.height_offset.get() / 2;
    let right_offset = border_offset.width_offset.get() - left_offset;
    let bottom_offset = border_offset.height_offset.get() - top_offset;
    let left = rect.left + left_offset;
    let top = rect.top + top_offset;
    let right = rect.right - right_offset;
    let bottom = rect.bottom - bottom_offset;
    let physical_size = size(DevicePixels(right - left), DevicePixels(bottom - top));
    Bounds {
        origin: logical_point(left as f32, top as f32, scale_factor),
        size: physical_size.to_pixels(scale_factor),
    }
}

fn retrieve_window_placement(
    hwnd: HWND,
    display: WindowsDisplay,
    initial_bounds: Bounds<Pixels>,
    border_offset: &WindowBorderOffset,
) -> Result<WINDOWPLACEMENT> {
    let mut placement = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    };
    unsafe { GetWindowPlacement(hwnd, &mut placement)? };
    // the bounds may be not inside the display
    let bounds = if display.check_given_bounds(initial_bounds) {
        initial_bounds
    } else {
        display.default_bounds()
    };
    // `bounds` is expressed in logical pixels for `display`, so it must be converted
    // to device pixels using that display's own scale factor. The window's current
    // scale factor can't be used here: `CreateWindowExW` was called with
    // `CW_USEDEFAULT`, so at this point the window may still be sitting on whichever
    // monitor Windows picked by default, which can have a different DPI than `display`
    // and would otherwise throw off the physical position (e.g. leaving the window
    // partially off-screen when moved to a monitor with a different scale factor).
    let bounds = bounds.to_device_pixels(display.scale_factor());
    placement.rcNormalPosition = calculate_window_rect(bounds, border_offset);
    Ok(placement)
}

fn dwm_set_window_composition_attribute(hwnd: HWND, backdrop_type: u32) {
    let mut version = unsafe { std::mem::zeroed() };
    let status = unsafe { windows::Wdk::System::SystemServices::RtlGetVersion(&mut version) };

    // DWMWA_SYSTEMBACKDROP_TYPE is available only on version 22621 or later
    // using SetWindowCompositionAttributeType as a fallback
    if !status.is_ok() || version.dwBuildNumber < 22621 {
        return;
    }

    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop_type as *const _ as *const _,
            std::mem::size_of_val(&backdrop_type) as u32,
        );
    }
}

fn set_window_composition_attribute(hwnd: HWND, color: Option<Color>, state: u32) {
    let mut version = unsafe { std::mem::zeroed() };
    let status = unsafe { windows::Wdk::System::SystemServices::RtlGetVersion(&mut version) };

    if !status.is_ok() || version.dwBuildNumber < 17763 {
        return;
    }

    unsafe {
        type SetWindowCompositionAttributeType =
            unsafe extern "system" fn(HWND, *mut WINDOWCOMPOSITIONATTRIBDATA) -> BOOL;
        let module_name = PCSTR::from_raw(c"user32.dll".as_ptr() as *const u8);
        if let Some(user32) = GetModuleHandleA(module_name)
            .context("Unable to get user32.dll handle")
            .log_err()
        {
            let func_name = PCSTR::from_raw(c"SetWindowCompositionAttribute".as_ptr() as *const u8);
            let Some(raw_set_window_composition_attribute) = GetProcAddress(user32, func_name)
            else {
                return;
            };
            let set_window_composition_attribute: SetWindowCompositionAttributeType =
                std::mem::transmute(raw_set_window_composition_attribute);
            let mut color = color.unwrap_or_default();
            let is_acrylic = state == 4;
            if is_acrylic && color.3 == 0 {
                color.3 = 1;
            }
            let accent = AccentPolicy {
                accent_state: state,
                accent_flags: if is_acrylic { 0 } else { 2 },
                gradient_color: (color.0 as u32)
                    | ((color.1 as u32) << 8)
                    | ((color.2 as u32) << 16)
                    | ((color.3 as u32) << 24),
                animation_id: 0,
            };
            let mut data = WINDOWCOMPOSITIONATTRIBDATA {
                attrib: 0x13,
                pv_data: &accent as *const _ as *mut _,
                cb_data: std::mem::size_of::<AccentPolicy>(),
            };
            let _ = set_window_composition_attribute(hwnd, &mut data as *mut _ as _);
        }
    }
}

// When the platform title bar is hidden, Windows may think that our application is meant to appear 'fullscreen'
// and will stop the taskbar from appearing on top of our window. Prevent this.
// https://devblogs.microsoft.com/oldnewthing/20250522-00/?p=111211
fn set_non_rude_hwnd(hwnd: HWND, non_rude: bool) {
    if non_rude {
        unsafe { SetPropW(hwnd, w!("NonRudeHWND"), Some(HANDLE(1 as _))) }.log_err();
    } else {
        unsafe { RemovePropW(hwnd, w!("NonRudeHWND")) }.log_err();
    }
}
