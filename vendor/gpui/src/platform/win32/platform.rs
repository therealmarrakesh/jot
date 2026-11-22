use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::anyhow::{Context as _, Result, anyhow};
use gpui::futures::channel::oneshot::Receiver;
use gpui::gpui_util::ResultExt;
use gpui::sync::RwLock;
use crate::smallvec::SmallVec;
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{Direct3D11::ID3D11Device, Gdi::*},
        System::{
            Com::*,
            LibraryLoader::*,
            Ole::*,
            Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
        },
        UI::{Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
    },
    core::*,
};

use crate::platform::win32::*;
use gpui::*;

struct TrackedWindow {
    handle: SafeHwnd,
    frame_signal: Arc<PlatformFrameSignal>,
}

impl TrackedWindow {
    fn as_raw(&self) -> HWND {
        self.handle.as_raw()
    }
}

pub struct WindowsPlatform {
    inner: Rc<WindowsPlatformInner>,
    raw_window_handles: Arc<RwLock<SmallVec<[TrackedWindow; 4]>>>,
    vsync_stop: RefCell<Option<Arc<AtomicBool>>>,
    icon: HICON,
    background_executor: BackgroundExecutor,
    foreground_executor: ForegroundExecutor,
    text_system: Arc<dyn PlatformTextSystem>,
    direct_write_text_system: Arc<DirectWriteTextSystem>,
    drop_target_helper: IDropTargetHelper,
    invalidate_devices: Arc<AtomicBool>,
    handle: HWND,
    disable_direct_composition: bool,
}

struct WindowsPlatformInner {
    state: WindowsPlatformState,
    raw_window_handles: std::sync::Weak<RwLock<SmallVec<[TrackedWindow; 4]>>>,
    validation_number: usize,
    main_receiver: PriorityQueueReceiver<RunnableVariant>,
    dispatcher: Arc<WindowsDispatcher>,
}

pub(crate) struct WindowsPlatformState {
    callbacks: PlatformCallbacks,
    pub(crate) current_cursor: Cell<Option<HCURSOR>>,
    pub(crate) cursor_visible: Arc<AtomicBool>,
    pub(crate) draw_coordinator: Rc<DrawCoordinator>,
    directx_devices: RefCell<Option<DirectXDevices>>,
}

#[derive(Default)]
struct PlatformCallbacks {
    quit: Cell<Option<Box<dyn FnMut() -> bool>>>,
    keyboard_layout_change: Cell<Option<Box<dyn FnMut()>>>,
    displays_changed: Cell<Option<Box<dyn FnMut()>>>,
}

impl WindowsPlatformState {
    fn new(directx_devices: Option<DirectXDevices>) -> Self {
        let callbacks = PlatformCallbacks::default();
        let current_cursor = load_cursor(CursorStyle::Arrow);

        Self {
            callbacks,
            current_cursor: Cell::new(current_cursor),
            cursor_visible: Arc::new(AtomicBool::new(true)),
            draw_coordinator: Rc::new(DrawCoordinator::new()),
            directx_devices: RefCell::new(directx_devices),
        }
    }
}

impl WindowsPlatform {
    pub fn new() -> Result<Self> {
        unsafe {
            OleInitialize(None).context("unable to initialize Windows OLE")?;
        }
        let direct_write_text_system = Arc::new(
            DirectWriteTextSystem::new(None).context("Error creating DirectWriteTextSystem")?,
        );
        let text_system: Arc<dyn PlatformTextSystem> = direct_write_text_system.clone();

        let (main_sender, main_receiver) = PriorityQueueReceiver::new();
        let validation_number = {
            use std::hash::{BuildHasher as _, Hasher as _};
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(std::process::id() as u64);
            h.finish() as usize
        };
        let raw_window_handles = Arc::new(RwLock::new(SmallVec::new()));

        register_platform_window_class();
        let mut context = PlatformWindowCreateContext {
            inner: None,
            raw_window_handles: Arc::downgrade(&raw_window_handles),
            validation_number,
            main_sender: Some(main_sender),
            main_receiver: Some(main_receiver),
            directx_devices: None,
            dispatcher: None,
        };
        let result = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                PLATFORM_WINDOW_CLASS_NAME,
                None,
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                Some(&raw const context as *const _),
            )
        };
        let inner = context
            .inner
            .take()
            .context("CreateWindowExW did not run correctly")??;
        let dispatcher = context
            .dispatcher
            .take()
            .context("CreateWindowExW did not run correctly")?;
        let handle = result?;

        let disable_direct_composition = std::env::var(DISABLE_DIRECT_COMPOSITION)
            .is_ok_and(|value| value == "true" || value == "1");
        let background_executor = BackgroundExecutor::new(dispatcher.clone());
        let foreground_executor = ForegroundExecutor::new(dispatcher);

        let drop_target_helper: IDropTargetHelper = unsafe {
            CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER)
                .context("Error creating drop target helper.")?
        };
        let icon = load_icon().unwrap_or_default();

        Ok(Self {
            inner,
            handle,
            raw_window_handles,
            vsync_stop: RefCell::new(None),
            icon,
            background_executor,
            foreground_executor,
            text_system,
            direct_write_text_system,
            disable_direct_composition,
            drop_target_helper,
            invalidate_devices: Arc::new(AtomicBool::new(false)),
        })
    }

    pub(crate) fn window_from_hwnd(&self, hwnd: HWND) -> Option<Rc<WindowsWindowInner>> {
        self.raw_window_handles
            .read()
            .iter()
            .find(|entry| entry.as_raw() == hwnd)
            .and_then(|hwnd| window_from_hwnd(hwnd.as_raw()))
    }

    #[inline]
    fn post_message(&self, message: u32, wparam: WPARAM, lparam: LPARAM) {
        self.raw_window_handles
            .read()
            .iter()
            .for_each(|handle| unsafe {
                PostMessageW(Some(handle.as_raw()), message, wparam, lparam).log_err();
            });
    }

    fn generate_creation_info(&self) -> WindowCreationInfo {
        WindowCreationInfo {
            icon: self.icon,
            executor: self.foreground_executor.clone(),
            current_cursor: self.inner.state.current_cursor.get(),
            cursor_visible: self.inner.state.cursor_visible.clone(),
            drop_target_helper: self.drop_target_helper.clone(),
            validation_number: self.inner.validation_number,
            main_receiver: self.inner.main_receiver.clone(),
            platform_window_handle: self.handle,
            disable_direct_composition: self.disable_direct_composition,
            directx_devices: self.inner.state.directx_devices.borrow().clone().unwrap(),
            invalidate_devices: self.invalidate_devices.clone(),
            draw_coordinator: self.inner.state.draw_coordinator.clone(),
        }
    }

    fn find_current_active_window(&self) -> Option<HWND> {
        let active_window_hwnd = unsafe { GetActiveWindow() };
        if active_window_hwnd.is_invalid() {
            return None;
        }
        self.raw_window_handles
            .read()
            .iter()
            .find(|hwnd| hwnd.as_raw() == active_window_hwnd)
            .map(|hwnd| hwnd.as_raw())
    }

    /// Creates the DirectX devices that windows draw with, and starts vsync.
    fn attach_gpu(&self) -> Result<()> {
        let devices = DirectXDevices::new().context("Creating DirectX devices")?;
        self.direct_write_text_system.handle_gpu_lost(&devices)?;
        *self.inner.state.directx_devices.borrow_mut() = Some(devices);
        self.begin_vsync_thread();
        Ok(())
    }

    fn begin_vsync_thread(&self) {
        let Some(directx_devices) = self.inner.state.directx_devices.borrow().clone() else {
            return;
        };
        let stop = Arc::new(AtomicBool::new(false));
        *self.vsync_stop.borrow_mut() = Some(stop.clone());
        let mut directx_device = directx_devices;
        let platform_window: SafeHwnd = self.handle.into();
        let validation_number = self.inner.validation_number;
        let all_windows = Arc::downgrade(&self.raw_window_handles);
        let text_system = Arc::downgrade(&self.direct_write_text_system);
        let invalidate_devices = self.invalidate_devices.clone();

        std::thread::Builder::new()
            .name("VSyncProvider".to_owned())
            .spawn(move || {
                let vsync_provider = VSyncProvider::new();
                loop {
                    let signal_source = vsync_provider.wait_for_vsync();
                    let signal_at = PlatformFrameSignal::capture(gpui::scheduler::Instant::now);
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    if check_device_lost(&directx_device.device)
                        || invalidate_devices.fetch_and(false, Ordering::Acquire)
                    {
                        if let Err(err) = handle_gpu_device_lost(
                            &mut directx_device,
                            platform_window.as_raw(),
                            validation_number,
                            &all_windows,
                            &text_system,
                        ) {
                            panic!("Device lost: {err}");
                        }
                    }
                    let Some(all_windows) = all_windows.upgrade() else {
                        break;
                    };
                    for hwnd in all_windows.read().iter() {
                        unsafe {
                            if let Some(signal_at) = signal_at {
                                if IsWindowVisible(hwnd.as_raw()).as_bool()
                                    && !IsIconic(hwnd.as_raw()).as_bool()
                                {
                                    hwnd.frame_signal.record(signal_at, signal_source);
                                } else {
                                    // Hidden windows may not consume WM_PAINT until shown again.
                                    hwnd.frame_signal.take();
                                }
                            }
                            RedrawWindow(Some(hwnd.as_raw()), None, None, RDW_INVALIDATE)
                                .ok()
                                .log_err();
                        }
                    }
                }
            })
            .unwrap();
    }
}

fn translate_accelerator(msg: &MSG) -> Option<()> {
    if msg.message != WM_KEYDOWN && msg.message != WM_SYSKEYDOWN {
        return None;
    }

    let result = unsafe {
        SendMessageW(
            msg.hwnd,
            WM_GPUI_KEYDOWN,
            Some(msg.wParam),
            Some(msg.lParam),
        )
    };
    (result.0 == 0).then_some(())
}

impl Platform for WindowsPlatform {
    fn background_executor(&self) -> BackgroundExecutor {
        self.background_executor.clone()
    }

    fn foreground_executor(&self) -> ForegroundExecutor {
        self.foreground_executor.clone()
    }

    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.text_system.clone()
    }

    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(
            WindowsKeyboardLayout::new()
                .log_err()
                .unwrap_or(WindowsKeyboardLayout::unknown()),
        )
    }

    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        Rc::new(WindowsKeyboardMapper::new())
    }

    fn on_keyboard_layout_change(&self, callback: Box<dyn FnMut()>) {
        self.inner
            .state
            .callbacks
            .keyboard_layout_change
            .set(Some(callback));
    }

    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        self.attach_gpu()
            .unwrap_or_else(|error| panic!("failed to initialize DirectX: {error:#}"));
        on_finish_launching();

        let mut msg = MSG::default();
        unsafe {
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if translate_accelerator(&msg).is_none() {
                    _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }

        self.inner.with_callback(
            |callbacks| &callbacks.quit,
            |callback| {
                callback();
            },
        );
    }

    fn quit(&self) {
        self.foreground_executor()
            .spawn(async { unsafe { PostQuitMessage(0) } })
            .detach();
    }

    fn activate(&self, _ignoring_other_apps: bool) {}

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        WindowsDisplay::displays()
    }

    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        WindowsDisplay::primary_monitor().map(|display| Rc::new(display) as Rc<dyn PlatformDisplay>)
    }

    fn on_displays_changed(&self, callback: Box<dyn FnMut()>) {
        self.inner
            .state
            .callbacks
            .displays_changed
            .set(Some(callback));
    }

    fn active_window(&self) -> Option<AnyWindowHandle> {
        let active_window_hwnd = unsafe { GetActiveWindow() };
        self.window_from_hwnd(active_window_hwnd)
            .map(|inner| inner.handle)
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        options: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        let window = WindowsWindow::new(handle, options, self.generate_creation_info())?;
        let handle = window.get_raw_handle();
        self.raw_window_handles.write().push(TrackedWindow {
            handle: handle.into(),
            frame_signal: window.state.frame_signal.clone(),
        });

        Ok(Box::new(window))
    }

    fn window_appearance(&self) -> WindowAppearance {
        app_appearance().log_err().unwrap_or_default()
    }

    fn set_window_appearance(&self, appearance: Option<WindowAppearance>) {
        set_appearance_override(appearance);
        self.post_message(WM_GPUI_APPEARANCE_CHANGED, WPARAM(0), LPARAM(0));
    }

    fn prompt_for_paths(
        &self,
        options: PathPromptOptions,
    ) -> Receiver<Result<Option<Vec<PathBuf>>>> {
        let owner = self
            .find_current_active_window()
            .and_then(|hwnd| self.window_from_hwnd(hwnd))
            .map(|window| window.dialog_owner.clone());
        crate::platform::win32::dialog::show_dialog(owner, &self.foreground_executor, move |window| {
            file_open_dialog(options, Some(window))
        })
    }

    fn prompt_for_new_path(
        &self,
        directory: &Path,
        suggested_name: Option<&str>,
    ) -> Receiver<Result<Option<PathBuf>>> {
        let directory = directory.to_owned();
        let suggested_name = suggested_name.map(|s| s.to_owned());
        let owner = self
            .find_current_active_window()
            .and_then(|hwnd| self.window_from_hwnd(hwnd))
            .map(|window| window.dialog_owner.clone());
        crate::platform::win32::dialog::show_dialog(owner, &self.foreground_executor, move |window| {
            let options = SavePathOptions {
                directory,
                file_name: suggested_name,
                ..SavePathOptions::default()
            };
            file_save_dialog(&options, Some(window)).map(|result| result.map(|(path, _)| path))
        })
    }

    fn on_quit(&self, callback: Box<dyn FnMut() -> bool>) {
        self.inner.state.callbacks.quit.set(Some(callback));
    }

    fn set_cursor_style(&self, style: CursorStyle) {
        let hcursor = load_cursor(style);
        if self.inner.state.current_cursor.get().map(|c| c.0) != hcursor.map(|c| c.0) {
            self.post_message(
                WM_GPUI_CURSOR_STYLE_CHANGED,
                WPARAM(0),
                LPARAM(hcursor.map_or(0, |c| c.0 as isize)),
            );
            self.inner.state.current_cursor.set(hcursor);
        }
    }

    fn hide_cursor_until_mouse_moves(&self) {
        if !self
            .inner
            .state
            .cursor_visible
            .swap(false, Ordering::Relaxed)
        {
            return;
        }

        for handle in self.raw_window_handles.read().iter() {
            let Some(window) = window_from_hwnd(handle.as_raw()) else {
                continue;
            };
            if window.state.hovered.get() {
                unsafe { SetCursor(None) };
                break;
            }
        }
    }

    fn is_cursor_visible(&self) -> bool {
        self.inner.state.cursor_visible.load(Ordering::Relaxed)
    }

    fn should_auto_hide_scrollbars(&self) -> bool {
        should_auto_hide_scrollbars().log_err().unwrap_or(false)
    }

    fn write_to_clipboard(&self, item: ClipboardItem) {
        write_to_clipboard(item);
    }

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        read_from_clipboard()
    }
}

impl WindowsPlatformInner {
    fn new(context: &mut PlatformWindowCreateContext) -> Result<Rc<Self>> {
        let state = WindowsPlatformState::new(context.directx_devices.take());
        Ok(Rc::new(Self {
            state,
            raw_window_handles: context.raw_window_handles.clone(),
            dispatcher: context
                .dispatcher
                .as_ref()
                .context("missing dispatcher")?
                .clone(),
            validation_number: context.validation_number,
            main_receiver: context
                .main_receiver
                .take()
                .context("missing main receiver")?,
        }))
    }

    /// Calls `project` to project to the corresponding callback field, removes it from callbacks, calls `f` with the callback and then puts the callback back.
    fn with_callback<T>(
        &self,
        project: impl Fn(&PlatformCallbacks) -> &Cell<Option<T>>,
        f: impl FnOnce(&mut T),
    ) {
        let callback = project(&self.state.callbacks).take();
        if let Some(mut callback) = callback {
            f(&mut callback);
            project(&self.state.callbacks).set(Some(callback));
        }
    }

    fn handle_msg(
        self: &Rc<Self>,
        handle: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let handled = match msg {
            WM_GPUI_CLOSE_ONE_WINDOW
            | WM_GPUI_TASK_DISPATCHED_ON_MAIN_THREAD
            | WM_GPUI_KEYBOARD_LAYOUT_CHANGED
            | WM_GPUI_DISPLAYS_CHANGED
            | WM_GPUI_GPU_DEVICE_LOST => self.handle_gpui_events(msg, wparam, lparam),
            WM_DISPLAYCHANGE => self.handle_display_change(handle),
            WM_QUERYENDSESSION => Some(1),
            WM_ENDSESSION if wparam.0 != 0 => self.handle_end_session(),
            _ => None,
        };
        if let Some(result) = handled {
            LRESULT(result)
        } else {
            unsafe { DefWindowProcW(handle, msg, wparam, lparam) }
        }
    }

    fn handle_gpui_events(&self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<isize> {
        if wparam.0 != self.validation_number {
            crate::log::error!("Wrong validation number while processing message: {message}");
            return None;
        }
        match message {
            WM_GPUI_CLOSE_ONE_WINDOW => {
                self.close_one_window(HWND(lparam.0 as _));
                Some(0)
            }
            WM_GPUI_TASK_DISPATCHED_ON_MAIN_THREAD => self.run_foreground_task(),
            WM_GPUI_KEYBOARD_LAYOUT_CHANGED => self.handle_keyboard_layout_change(),
            WM_GPUI_DISPLAYS_CHANGED => self.handle_displays_changed(),
            WM_GPUI_GPU_DEVICE_LOST => self.handle_device_lost(lparam),
            _ => unreachable!(),
        }
    }

    fn handle_display_change(&self, handle: HWND) -> Option<isize> {
        // Broadcasts can be delivered while the app is mid-update, e.g. inside a
        // COM call, so report the change from the message loop instead.
        unsafe {
            PostMessageW(
                Some(handle),
                WM_GPUI_DISPLAYS_CHANGED,
                WPARAM(self.validation_number),
                LPARAM(0),
            )
            .log_err();
        }
        Some(0)
    }

    fn handle_displays_changed(&self) -> Option<isize> {
        self.with_callback(
            |callbacks| &callbacks.displays_changed,
            |callback| callback(),
        );
        Some(0)
    }

    fn handle_end_session(&self) -> Option<isize> {
        let mut shutdown_completed = false;
        self.with_callback(
            |callbacks| &callbacks.quit,
            |callback| shutdown_completed = callback(),
        );
        crate::log::logger().flush();
        if shutdown_completed {
            std::process::exit(0);
        }

        // Shutdown couldn't run synchronously, since the AppCell is already borrowed.
        // Windows may terminate the application as soon as we return from this handler, but if we post a WM_QUIT message now,
        // we may get to gracefully shut down the app before we're terminated by the OS.
        unsafe { PostQuitMessage(0) };
        Some(0)
    }

    fn close_one_window(&self, target_window: HWND) -> bool {
        let Some(all_windows) = self.raw_window_handles.upgrade() else {
            crate::log::error!("Failed to upgrade raw window handles");
            return false;
        };
        let mut lock = all_windows.write();
        let index = lock
            .iter()
            .position(|handle| handle.as_raw() == target_window)
            .unwrap();
        lock.remove(index);

        lock.is_empty()
    }

    #[inline]
    fn run_foreground_task(&self) -> Option<isize> {
        const MAIN_TASK_TIMEOUT: u128 = 10;

        let start = std::time::Instant::now();
        'tasks: loop {
            'timeout_loop: loop {
                if start.elapsed().as_millis() >= MAIN_TASK_TIMEOUT {
                    crate::log::debug!("foreground task timeout reached");
                    // we spent our budget on gpui tasks, we likely have a lot of work queued so drain system events first to stay responsive
                    // then quit out of foreground work to allow us to process other gpui events first before returning back to foreground task work
                    // if we don't we might not for example process window quit events
                    let mut msg = MSG::default();
                    let process_message = |msg: &_| {
                        if translate_accelerator(msg).is_none() {
                            _ = unsafe { TranslateMessage(msg) };
                            unsafe { DispatchMessageW(msg) };
                        }
                    };
                    let peek_msg = |msg: &mut _, msg_kind| unsafe {
                        PeekMessageW(msg, None, 0, 0, PM_REMOVE | msg_kind).as_bool()
                    };
                    // We need to process a paint message here as otherwise we will re-enter `run_foreground_task` before painting if we have work remaining.
                    // The reason for this is that windows prefers custom application message processing over system messages.
                    if peek_msg(&mut msg, PM_QS_PAINT) {
                        process_message(&msg);
                    }
                    while peek_msg(&mut msg, PM_QS_INPUT) {
                        process_message(&msg);
                    }
                    // Allow the main loop to process other gpui events before going back into `run_foreground_task`
                    unsafe {
                        if let Err(_) = PostMessageW(
                            Some(self.dispatcher.platform_window_handle.as_raw()),
                            WM_GPUI_TASK_DISPATCHED_ON_MAIN_THREAD,
                            WPARAM(self.validation_number),
                            LPARAM(0),
                        ) {
                            self.dispatcher.wake_posted.store(false, Ordering::Release);
                        };
                    }
                    break 'tasks;
                }
                let mut main_receiver = self.main_receiver.clone();
                match main_receiver.try_pop() {
                    Ok(Some(runnable)) => WindowsDispatcher::execute_runnable(runnable),
                    _ => break 'timeout_loop,
                }
            }

            // Someone could enqueue a Runnable here. The flag is still true, so they will not PostMessage.
            // We need to check for those Runnables after we clear the flag.
            self.dispatcher.wake_posted.store(false, Ordering::Release);
            let mut main_receiver = self.main_receiver.clone();
            match main_receiver.try_pop() {
                Ok(Some(runnable)) => {
                    self.dispatcher.wake_posted.store(true, Ordering::Release);

                    WindowsDispatcher::execute_runnable(runnable);
                }
                _ => break 'tasks,
            }
        }

        Some(0)
    }

    fn handle_keyboard_layout_change(&self) -> Option<isize> {
        self.with_callback(
            |callbacks| &callbacks.keyboard_layout_change,
            |callback| callback(),
        );
        Some(0)
    }

    fn handle_device_lost(&self, lparam: LPARAM) -> Option<isize> {
        let directx_devices = lparam.0 as *const DirectXDevices;
        let directx_devices = unsafe { &*directx_devices };
        let mut current = self.state.directx_devices.borrow_mut();
        if current.is_some() {
            *current = Some(directx_devices.clone());
        }

        Some(0)
    }
}

impl Drop for WindowsPlatform {
    fn drop(&mut self) {
        if let Some(stop) = self.vsync_stop.borrow_mut().take() {
            stop.store(true, Ordering::Release);
        }
        unsafe {
            DestroyWindow(self.handle)
                .context("Destroying platform window")
                .log_err();
            OleUninitialize();
        }
    }
}

pub(crate) struct WindowCreationInfo {
    pub(crate) icon: HICON,
    pub(crate) executor: ForegroundExecutor,
    pub(crate) current_cursor: Option<HCURSOR>,
    pub(crate) cursor_visible: Arc<AtomicBool>,
    pub(crate) drop_target_helper: IDropTargetHelper,
    pub(crate) validation_number: usize,
    pub(crate) main_receiver: PriorityQueueReceiver<RunnableVariant>,
    pub(crate) platform_window_handle: HWND,
    pub(crate) disable_direct_composition: bool,
    pub(crate) directx_devices: DirectXDevices,
    /// Flag to instruct the `VSyncProvider` thread to invalidate the directx devices
    /// as resizing them has failed, causing us to have lost at least the render target.
    pub(crate) invalidate_devices: Arc<AtomicBool>,
    /// Shared with [`WindowsPlatformState::draw_coordinator`] and every other window.
    pub(crate) draw_coordinator: Rc<DrawCoordinator>,
}

struct PlatformWindowCreateContext {
    inner: Option<Result<Rc<WindowsPlatformInner>>>,
    raw_window_handles: std::sync::Weak<RwLock<SmallVec<[TrackedWindow; 4]>>>,
    validation_number: usize,
    main_sender: Option<PriorityQueueSender<RunnableVariant>>,
    main_receiver: Option<PriorityQueueReceiver<RunnableVariant>>,
    directx_devices: Option<DirectXDevices>,
    dispatcher: Option<Arc<WindowsDispatcher>>,
}

fn file_open_dialog(
    options: PathPromptOptions,
    window: Option<HWND>,
) -> Result<Option<Vec<PathBuf>>> {
    let folder_dialog: IFileOpenDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL)? };

    let mut dialog_options = FOS_FILEMUSTEXIST;
    if options.multiple {
        dialog_options |= FOS_ALLOWMULTISELECT;
    }
    if options.directories {
        dialog_options |= FOS_PICKFOLDERS;
    }

    unsafe {
        folder_dialog.SetOptions(dialog_options)?;

        if let Some(prompt) = options.prompt {
            let prompt: &str = &prompt;
            folder_dialog.SetOkButtonLabel(&HSTRING::from(prompt))?;
        }

        if let Err(error) = folder_dialog.Show(window) {
            if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
                return Ok(None);
            }
            return Err(error.into());
        }
    }

    let results = unsafe { folder_dialog.GetResults()? };
    let file_count = unsafe { results.GetCount()? };
    if file_count == 0 {
        return Ok(None);
    }

    let mut paths = Vec::with_capacity(file_count as usize);
    for i in 0..file_count {
        let item = unsafe { results.GetItemAt(i)? };
        let path = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH)?.to_string()? };
        paths.push(PathBuf::from(path));
    }

    Ok(Some(paths))
}

pub struct SaveChoice {
    pub label: String,
    pub items: Vec<String>,
    pub selected: usize,
}

pub struct SavePathOptions {
    pub title: Option<String>,
    pub directory: PathBuf,
    pub file_name: Option<String>,
    pub file_types: Vec<(String, String)>,
    pub default_extension: Option<String>,
    pub choice: Option<SaveChoice>,
}

impl Default for SavePathOptions {
    fn default() -> Self {
        Self {
            title: None,
            directory: PathBuf::new(),
            file_name: None,
            file_types: vec![("All files".into(), "*.*".into())],
            default_extension: None,
            choice: None,
        }
    }
}

pub fn prompt_for_save_path(
    owner: isize,
    executor: &ForegroundExecutor,
    options: SavePathOptions,
) -> Receiver<Result<Option<(PathBuf, Option<usize>)>>> {
    let owner = window_from_hwnd(HWND(owner as _)).map(|window| window.dialog_owner.clone());
    crate::platform::win32::dialog::show_dialog(owner, executor, move |window| {
        file_save_dialog(&options, Some(window))
    })
}

const SAVE_CHOICE_GROUP: u32 = 1;
const SAVE_CHOICE_COMBO: u32 = 2;

fn file_save_dialog(
    options: &SavePathOptions,
    window: Option<HWND>,
) -> Result<Option<(PathBuf, Option<usize>)>> {
    let dialog: IFileSaveDialog = unsafe { CoCreateInstance(&FileSaveDialog, None, CLSCTX_ALL)? };
    let directory = &options.directory;
    if !directory.to_string_lossy().is_empty()
        && let Some(full_path) = directory
            .canonicalize()
            .context("failed to canonicalize directory")
            .log_err()
    {
        let full_path_raw = full_path.display().to_string();
        let full_path_string = full_path_raw
            .strip_prefix(r"\\?\")
            .unwrap_or(&full_path_raw);
        let path_item: IShellItem =
            unsafe { SHCreateItemFromParsingName(&HSTRING::from(full_path_string), None)? };
        unsafe {
            dialog
                .SetFolder(&path_item)
                .context("failed to set dialog folder")
                .log_err()
        };
    }

    if let Some(suggested_name) = &options.file_name {
        unsafe {
            dialog
                .SetFileName(&HSTRING::from(suggested_name))
                .context("failed to set file name")
                .log_err()
        };
    }
    if let Some(title) = &options.title {
        unsafe {
            dialog
                .SetTitle(&HSTRING::from(title))
                .context("failed to set dialog title")
                .log_err()
        };
    }
    if let Some(extension) = &options.default_extension {
        unsafe {
            dialog
                .SetDefaultExtension(&HSTRING::from(extension))
                .context("failed to set default extension")
                .log_err()
        };
    }

    let file_types: Vec<(HSTRING, HSTRING)> = options
        .file_types
        .iter()
        .map(|(name, spec)| (HSTRING::from(name), HSTRING::from(spec)))
        .collect();
    let specs: Vec<Common::COMDLG_FILTERSPEC> = file_types
        .iter()
        .map(|(name, spec)| Common::COMDLG_FILTERSPEC {
            pszName: PCWSTR(name.as_ptr()),
            pszSpec: PCWSTR(spec.as_ptr()),
        })
        .collect();
    let customize = options
        .choice
        .as_ref()
        .map(|choice| -> Result<IFileDialogCustomize> {
            let customize: IFileDialogCustomize = dialog.cast()?;
            unsafe {
                customize.StartVisualGroup(SAVE_CHOICE_GROUP, &HSTRING::from(&choice.label))?;
                customize.AddComboBox(SAVE_CHOICE_COMBO)?;
                for (ix, item) in choice.items.iter().enumerate() {
                    customize.AddControlItem(SAVE_CHOICE_COMBO, ix as u32, &HSTRING::from(item))?;
                }
                customize.SetSelectedControlItem(SAVE_CHOICE_COMBO, choice.selected as u32)?;
                customize.EndVisualGroup()?;
            }
            Ok(customize)
        })
        .transpose()?;

    unsafe {
        dialog.SetFileTypes(&specs)?;
        if let Err(error) = dialog.Show(window) {
            if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
                return Ok(None);
            }
            return Err(error.into());
        }
    }
    let selected = customize
        .map(|customize| unsafe { customize.GetSelectedControlItem(SAVE_CHOICE_COMBO) })
        .transpose()?
        .map(|ix| ix as usize);
    let shell_item = unsafe { dialog.GetResult()? };
    let file_path_string = unsafe {
        let pwstr = shell_item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let string = pwstr.to_string()?;
        CoTaskMemFree(Some(pwstr.0 as _));
        string
    };
    Ok(Some((PathBuf::from(file_path_string), selected)))
}

fn load_icon() -> Result<HICON> {
    let module = unsafe { GetModuleHandleW(None).context("unable to get module handle")? };
    let handle = unsafe {
        LoadImageW(
            Some(module.into()),
            windows::core::PCWSTR(1 as _),
            IMAGE_ICON,
            0,
            0,
            LR_DEFAULTSIZE | LR_SHARED,
        )
        .context("unable to load icon file")?
    };
    Ok(HICON(handle.0))
}

#[inline]
fn should_auto_hide_scrollbars() -> Result<bool> {
    let mut dynamic_scrollbars: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Control Panel\Accessibility"),
            w!("DynamicScrollbars"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut dynamic_scrollbars).cast()),
            Some(&mut size),
        )
    };
    Ok(status.0 != 0 || dynamic_scrollbars != 0)
}

fn check_device_lost(device: &ID3D11Device) -> bool {
    let device_state = unsafe { device.GetDeviceRemovedReason() };
    match device_state {
        Ok(_) => false,
        Err(err) => {
            crate::log::error!("DirectX device lost detected: {:?}", err);
            true
        }
    }
}

fn handle_gpu_device_lost(
    directx_devices: &mut DirectXDevices,
    platform_window: HWND,
    validation_number: usize,
    all_windows: &std::sync::Weak<RwLock<SmallVec<[TrackedWindow; 4]>>>,
    text_system: &std::sync::Weak<DirectWriteTextSystem>,
) -> Result<()> {
    // Here we wait a bit to ensure the system has time to recover from the device lost state.
    // If we don't wait, the final drawing result will be blank.
    std::thread::sleep(std::time::Duration::from_millis(350));

    *directx_devices = try_to_recover_from_device_lost(|| {
        DirectXDevices::new().context("Failed to recreate new DirectX devices after device lost")
    })?;
    crate::log::info!("DirectX devices successfully recreated.");

    let lparam = LPARAM(directx_devices as *const _ as _);
    unsafe {
        SendMessageW(
            platform_window,
            WM_GPUI_GPU_DEVICE_LOST,
            Some(WPARAM(validation_number)),
            Some(lparam),
        );
    }

    if let Some(text_system) = text_system.upgrade() {
        text_system.handle_gpu_lost(&directx_devices)?;
    }
    if let Some(all_windows) = all_windows.upgrade() {
        for window in all_windows.read().iter() {
            unsafe {
                SendMessageW(
                    window.as_raw(),
                    WM_GPUI_GPU_DEVICE_LOST,
                    Some(WPARAM(validation_number)),
                    Some(lparam),
                );
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        for window in all_windows.read().iter() {
            unsafe {
                SendMessageW(
                    window.as_raw(),
                    WM_GPUI_FORCE_UPDATE_WINDOW,
                    Some(WPARAM(validation_number)),
                    None,
                );
            }
        }
    }
    Ok(())
}

const PLATFORM_WINDOW_CLASS_NAME: PCWSTR = w!("Jot::PlatformWindow");

fn register_platform_window_class() {
    let wc = WNDCLASSW {
        lpfnWndProc: Some(window_procedure),
        lpszClassName: PCWSTR(PLATFORM_WINDOW_CLASS_NAME.as_ptr()),
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };
}

unsafe extern "system" fn window_procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let params = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        let creation_context = params.lpCreateParams as *mut PlatformWindowCreateContext;
        let creation_context = unsafe { &mut *creation_context };

        let Some(main_sender) = creation_context.main_sender.take() else {
            creation_context.inner = Some(Err(anyhow!("missing main sender")));
            return LRESULT(0);
        };
        creation_context.dispatcher = Some(Arc::new(WindowsDispatcher::new(
            main_sender,
            hwnd,
            creation_context.validation_number,
        )));

        return match WindowsPlatformInner::new(creation_context) {
            Ok(inner) => {
                let weak = Box::new(Rc::downgrade(&inner));
                unsafe { set_window_long(hwnd, GWLP_USERDATA, Box::into_raw(weak) as isize) };
                creation_context.inner = Some(Ok(inner));
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            Err(error) => {
                creation_context.inner = Some(Err(error));
                LRESULT(0)
            }
        };
    }

    let ptr = unsafe { get_window_long(hwnd, GWLP_USERDATA) } as *mut Weak<WindowsPlatformInner>;
    if ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let inner = unsafe { &*ptr };
    let result = if let Some(inner) = inner.upgrade() {
        if cfg!(debug_assertions) {
            let inner = std::panic::AssertUnwindSafe(inner);
            match std::panic::catch_unwind(|| { inner }.handle_msg(hwnd, msg, wparam, lparam)) {
                Ok(result) => result,
                Err(_) => std::process::abort(),
            }
        } else {
            inner.handle_msg(hwnd, msg, wparam, lparam)
        }
    } else {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    };

    if msg == WM_NCDESTROY {
        unsafe { set_window_long(hwnd, GWLP_USERDATA, 0) };
        unsafe { drop(Box::from_raw(ptr)) };
    }

    result
}
