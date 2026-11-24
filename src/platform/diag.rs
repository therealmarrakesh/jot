use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(not(test))]
use std::sync::atomic::{AtomicBool, Ordering};

use super::{message_box_error, output_debug_string};

pub(crate) fn debug_log(message: &str) {
    output_debug_string(&format!("[Jot] {message}\n"));
}

pub(crate) fn show_error_dialog(title: &str, message: &str) {
    message_box_error(title, message);
}

pub(crate) fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "Internal error".to_owned()
    }
}

pub(crate) fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = panic_message(info.payload());
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_owned());
        debug_log(&format!("PANIC at {loc}: {msg}"));
        prev(info);
    }));
}

pub(crate) fn ffi_guard<R>(context: &str, fallback: R, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(res) => res,
        Err(payload) => {
            let msg = panic_message(&*payload);
            debug_log(&format!("Caught panic at Win32 boundary ({context}): {msg}"));
            #[cfg(not(test))]
            {
                static SHOWN: AtomicBool = AtomicBool::new(false);
                if !SHOWN.swap(true, Ordering::AcqRel) {
                    show_error_dialog(
                        crate::APP_NAME,
                        &format!(
                            "Jot encountered an unexpected error and prevented a crash.\n\n\
                             Please save any open documents and restart Jot.\n\nDetails: {msg}"
                        ),
                    );
                }
            }
            fallback
        }
    }
}
