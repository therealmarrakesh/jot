use std::{
    ffi::c_void,
    ptr::NonNull,
    sync::atomic::{AtomicBool, Ordering},
    thread::{ThreadId, current},
    time::Duration,
};

use crate::ui::gpui_util::ResultExt;
use windows::Win32::{
    Foundation::{FILETIME, LPARAM, WPARAM},
    System::Threading::{
        CloseThreadpoolTimer, CreateThreadpoolTimer, PTP_CALLBACK_INSTANCE, PTP_TIMER,
        SetThreadpoolTimer, TrySubmitThreadpoolCallback,
    },
    UI::WindowsAndMessaging::PostMessageW,
};

use crate::ui::{MainTaskQueue, Runnable};
use crate::win32::{HWND, SafeHwnd, WM_GPUI_TASK_DISPATCHED_ON_MAIN_THREAD};

pub(crate) struct WindowsDispatcher {
    pub(crate) wake_posted: AtomicBool,
    main_queue: MainTaskQueue,
    main_thread_id: ThreadId,
    pub(crate) platform_window_handle: SafeHwnd,
    validation_number: usize,
}

impl WindowsDispatcher {
    pub(crate) fn new(
        main_queue: MainTaskQueue,
        platform_window_handle: HWND,
        validation_number: usize,
    ) -> Self {
        Self {
            wake_posted: AtomicBool::new(false),
            main_queue,
            main_thread_id: current().id(),
            platform_window_handle: platform_window_handle.into(),
            validation_number,
        }
    }

    pub(crate) fn is_main_thread(&self) -> bool {
        current().id() == self.main_thread_id
    }

    pub(crate) fn dispatch(&self, runnable: Runnable) {
        // If the thread pool never runs our callback (e.g. during shutdown), leaking the runnable
        // avoids cancelling its task on the wrong thread.
        let context = runnable.into_raw().as_ptr() as *mut c_void;
        unsafe {
            TrySubmitThreadpoolCallback(Some(run_work_callback), Some(context), None).log_err();
        }
    }

    pub(crate) fn dispatch_on_main_thread(&self, runnable: Runnable) {
        match self.main_queue.push(runnable) {
            Ok(()) => {
                if !self.wake_posted.swap(true, Ordering::AcqRel) {
                    unsafe {
                        PostMessageW(
                            Some(self.platform_window_handle.as_raw()),
                            WM_GPUI_TASK_DISPATCHED_ON_MAIN_THREAD,
                            WPARAM(self.validation_number),
                            LPARAM(0),
                        )
                        .log_err();
                    }
                }
            }
            Err(runnable) => {
                // Queue is closed (app shutting down); forgetting avoids dropping a !Send future
                // off the main thread.
                std::mem::forget(runnable);
            }
        }
    }

    pub(crate) fn dispatch_after(&self, duration: Duration, runnable: Runnable) {
        let context = runnable.into_raw().as_ptr() as *mut c_void;
        unsafe {
            if let Ok(timer) = CreateThreadpoolTimer(Some(run_timer_callback), Some(context), None)
            {
                // Negative FILETIME expresses a relative delay in 100ns ticks.
                let ticks = (duration.as_nanos() / 100).min(i64::MAX as u128) as i64;
                let due = (-ticks) as u64;
                let due_time = FILETIME {
                    dwLowDateTime: due as u32,
                    dwHighDateTime: (due >> 32) as u32,
                };
                SetThreadpoolTimer(timer, Some(&due_time), 0, None);
            }
        }
    }

    #[inline(always)]
    pub(crate) fn execute_runnable(runnable: Runnable) {
        runnable.run();
    }
}

unsafe extern "system" fn run_work_callback(
    _instance: PTP_CALLBACK_INSTANCE,
    context: *mut c_void,
) {
    let runnable = unsafe { Runnable::from_raw(NonNull::new_unchecked(context as *mut ())) };
    WindowsDispatcher::execute_runnable(runnable);
}

unsafe extern "system" fn run_timer_callback(
    _instance: PTP_CALLBACK_INSTANCE,
    context: *mut c_void,
    timer: PTP_TIMER,
) {
    let runnable = unsafe { Runnable::from_raw(NonNull::new_unchecked(context as *mut ())) };
    WindowsDispatcher::execute_runnable(runnable);
    unsafe { CloseThreadpoolTimer(timer) };
}
