//! Inbound Win32 named-pipe server for single-instance handoff.

use windows::Win32::{
    Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, GetLastError, HANDLE},
    Storage::FileSystem::{PIPE_ACCESS_INBOUND, ReadFile},
    System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
        PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    },
    UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow},
};
use windows::core::PCWSTR;

const PIPE_BUFFER_SIZE: u32 = 16 * 1024;

pub(crate) fn allow_any_set_foreground_window() {
    let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
}

pub(crate) fn start_inbound_pipe_listener(
    pipe: &str,
    max_payload_bytes: usize,
    on_message: impl Fn(&[u8]) + Send + 'static,
) {
    let wide: Vec<u16> = pipe.encode_utf16().chain(Some(0)).collect();
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(wide.as_ptr()),
            PIPE_ACCESS_INBOUND,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            0,
            PIPE_BUFFER_SIZE,
            0,
            None,
        )
    };
    if handle.is_invalid() {
        return;
    }

    let raw_handle = handle.0 as usize;
    let _ = std::thread::Builder::new()
        .name("jot-instance-pipe".into())
        .spawn(move || {
            let handle = HANDLE(raw_handle as _);
            let mut payload = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let connected = unsafe { ConnectNamedPipe(handle, None) }.is_ok()
                    || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                if !connected {
                    break;
                }
                payload.clear();
                loop {
                    let mut read = 0u32;
                    let ok =
                        unsafe { ReadFile(handle, Some(&mut chunk), Some(&mut read), None) }.is_ok();
                    if !ok || read == 0 {
                        break;
                    }
                    let take = (read as usize).min(max_payload_bytes.saturating_sub(payload.len()));
                    payload.extend_from_slice(&chunk[..take]);
                    if payload.len() >= max_payload_bytes {
                        break;
                    }
                }
                let _ = unsafe { DisconnectNamedPipe(handle) };
                if !payload.is_empty() {
                    on_message(&payload);
                }
            }
            let _ = unsafe { CloseHandle(handle) };
        });
}
