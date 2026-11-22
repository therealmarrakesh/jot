use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::mpsc;

use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, GetLastError, HANDLE};
use windows::Win32::Storage::FileSystem::{PIPE_ACCESS_INBOUND, ReadFile};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
use windows::core::PCWSTR;

const PIPE_BUFFER_SIZE: u32 = 16 * 1024;
const MAX_PAYLOAD_BYTES: usize = 256 * 1024;

pub(crate) fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".into());
    let sanitized: String = user
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    let suffix = if sanitized.is_empty() {
        "default"
    } else {
        &sanitized
    };
    format!(r"\\.\pipe\Jot.Tabs.{suffix}")
}

fn encode_paths(paths: &[PathBuf]) -> Vec<u8> {
    let mut out = Vec::new();
    for path in paths {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.clone());
        out.extend_from_slice(abs.to_string_lossy().as_bytes());
        out.push(0);
    }
    out
}

fn decode_paths(bytes: &[u8]) -> Vec<PathBuf> {
    bytes
        .split(|&b| b == 0)
        .filter(|part| !part.is_empty())
        .filter_map(|part| std::str::from_utf8(part).ok())
        .map(PathBuf::from)
        .collect()
}

/// Attempts to hand `paths` to an already-running Jot instance.
/// Returns `true` if the running instance accepted them.
pub(crate) fn try_handoff(paths: &[PathBuf]) -> bool {
    try_handoff_to(&pipe_name(), paths)
}

fn try_handoff_to(pipe: &str, paths: &[PathBuf]) -> bool {
    if paths.is_empty() {
        return false;
    }
    let Ok(mut file) = OpenOptions::new().write(true).open(pipe) else {
        return false;
    };
    let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    let payload = encode_paths(paths);
    file.write_all(&payload).is_ok() && file.flush().is_ok()
}

/// Starts a lightweight background listener thread on the session's named pipe.
/// Incoming path batches are delivered via the returned channel.
pub(crate) fn start_listener(on_receive: impl Fn(Vec<PathBuf>) + Send + 'static) {
    start_listener_on(pipe_name(), on_receive);
}

fn start_listener_on(pipe: String, on_receive: impl Fn(Vec<PathBuf>) + Send + 'static) {
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
                    let take = (read as usize).min(MAX_PAYLOAD_BYTES.saturating_sub(payload.len()));
                    payload.extend_from_slice(&chunk[..take]);
                    if payload.len() >= MAX_PAYLOAD_BYTES {
                        break;
                    }
                }
                let _ = unsafe { DisconnectNamedPipe(handle) };
                let paths = decode_paths(&payload);
                if !paths.is_empty() {
                    on_receive(paths);
                }
            }
            let _ = unsafe { CloseHandle(handle) };
        });
}

#[cfg(test)]
fn _channel_smoke(pipe: &str) -> mpsc::Receiver<Vec<PathBuf>> {
    let (tx, rx) = mpsc::channel();
    start_listener_on(pipe.to_string(), move |paths| {
        let _ = tx.send(paths);
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn encodes_and_decodes_paths_round_trip() {
        let input = vec![
            PathBuf::from(r"C:\Temp\hello world.txt"),
            PathBuf::from(r"C:\Temp\日本語.txt"),
        ];
        let decoded = decode_paths(&encode_paths(&input));
        assert_eq!(decoded, input);
    }

    #[test]
    fn named_pipe_handoff_delivers_paths_to_running_listener() {
        let pipe = format!(r"\\.\pipe\Jot.Test.{}", std::process::id());
        let rx = _channel_smoke(&pipe);
        // Give the listener thread a moment to enter ConnectNamedPipe.
        std::thread::sleep(Duration::from_millis(20));

        let sent = vec![
            PathBuf::from(r"C:\Temp\first.txt"),
            PathBuf::from(r"C:\Temp\second file.txt"),
        ];
        assert!(try_handoff_to(&pipe, &sent));

        let received = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("listener should receive handed-off paths");
        assert_eq!(received, sent);
    }
}
