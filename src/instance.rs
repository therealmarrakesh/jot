//! Single-instance named-pipe handoff for forwarding CLI file paths to a running Jot process.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::mpsc;

use crate::platform::{allow_any_set_foreground_window, start_inbound_pipe_listener};

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
    allow_any_set_foreground_window();
    let payload = encode_paths(paths);
    file.write_all(&payload).is_ok() && file.flush().is_ok()
}

/// Starts a background listener thread on the session's named pipe.
pub(crate) fn start_listener(on_receive: impl Fn(Vec<PathBuf>) + Send + 'static) {
    start_listener_on(&pipe_name(), on_receive);
}

fn start_listener_on(pipe: &str, on_receive: impl Fn(Vec<PathBuf>) + Send + 'static) {
    start_inbound_pipe_listener(pipe, MAX_PAYLOAD_BYTES, move |payload| {
        let paths = decode_paths(payload);
        if !paths.is_empty() {
            on_receive(paths);
        }
    });
}

#[cfg(test)]
fn _channel_smoke(pipe: &str) -> mpsc::Receiver<Vec<PathBuf>> {
    let (tx, rx) = mpsc::channel();
    start_listener_on(pipe, move |paths| {
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
