//! Single-instance arbitration and framed UTF-16 named-pipe handoff (I-1..I-5).
//!
//! Arbitration runs at startup *before* D3D/D2D/DWrite initialization so simultaneous multi-file
//! Explorer launches never race into duplicate processes. No-argument launches also hand off a
//! `NewWindow` command to the active same-user, same-session, same-elevation instance.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, atomic::AtomicBool};
use std::time::Duration;

use crate::platform::{
    PipeClaim, PipeServer, current_process_identity, send_framed_handoff, try_claim_pipe_server,
};

const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
const CMD_OPEN_PATHS: u8 = 1;
const CMD_NEW_WINDOW: u8 = 2;

/// Command delivered from a secondary Jot launch to the primary instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HandoffCommand {
    OpenPaths(Vec<PathBuf>),
    NewWindow,
}

enum Dispatcher {
    Buffered(Vec<HandoffCommand>),
    Live(Box<dyn Fn(HandoffCommand) + Send>),
}

/// Active primary-instance server. Buffers any handoffs arriving during startup until
/// [`attach_handler`](Self::attach_handler) connects `App`'s event poster.
pub(crate) struct InstanceServer {
    pipe: PipeServer,
    dispatcher: Arc<Mutex<Dispatcher>>,
}

impl InstanceServer {
    pub(crate) fn accepting_flag(&self) -> Arc<AtomicBool> {
        self.pipe.accepting_flag()
    }

    /// Attaches the live UI-thread event poster and immediately flushes any startup handoffs.
    pub(crate) fn attach_handler(&self, on_command: impl Fn(HandoffCommand) + Send + 'static) {
        let pending = {
            let mut guard = self.dispatcher.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::mem::replace(&mut *guard, Dispatcher::Live(Box::new(on_command)));
            match prev {
                Dispatcher::Buffered(items) => items,
                Dispatcher::Live(_) => Vec::new(),
            }
        };
        if !pending.is_empty() {
            let guard = self.dispatcher.lock().unwrap_or_else(|e| e.into_inner());
            if let Dispatcher::Live(handler) = &*guard {
                for cmd in pending {
                    handler(cmd);
                }
            }
        }
    }
}

/// Result of early startup single-instance arbitration.
pub(crate) enum Arbitration {
    /// This process owns the pipe and should initialize and run the UI.
    Primary(Option<InstanceServer>),
    /// An already-running instance accepted and acknowledged our handoff; this process should exit.
    HandedOff,
}

/// Computes the SID-, session-, and elevation-scoped named-pipe path (I-2, I-3).
pub(crate) fn pipe_name() -> String {
    if let Some(id) = current_process_identity() {
        let elev = if id.elevated { "elev" } else { "std" };
        return format!(r"\\.\pipe\Jot.Tabs.S{}.{}.{}", id.session_id, elev, id.sid);
    }
    // Fallback if token queries are restricted.
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

fn encode_command(cmd: &HandoffCommand) -> (u8, Vec<u8>) {
    match cmd {
        HandoffCommand::NewWindow => (CMD_NEW_WINDOW, Vec::new()),
        HandoffCommand::OpenPaths(paths) => {
            let mut body = Vec::new();
            let count = u32::try_from(paths.len()).unwrap_or(u32::MAX);
            body.extend_from_slice(&count.to_le_bytes());
            for path in paths.iter().take(count as usize) {
                let abs = std::path::absolute(path).unwrap_or_else(|_| path.clone());
                let units: Vec<u16> = abs.as_os_str().encode_wide().collect();
                let len = u32::try_from(units.len()).unwrap_or(0);
                body.extend_from_slice(&len.to_le_bytes());
                for unit in units.into_iter().take(len as usize) {
                    body.extend_from_slice(&unit.to_le_bytes());
                }
            }
            (CMD_OPEN_PATHS, body)
        }
    }
}

fn decode_command(cmd: u8, body: &[u8]) -> Option<HandoffCommand> {
    match cmd {
        CMD_NEW_WINDOW => body.is_empty().then_some(HandoffCommand::NewWindow),
        CMD_OPEN_PATHS => {
            if body.len() < 4 {
                return None;
            }
            let count = u32::from_le_bytes(body[0..4].try_into().ok()?) as usize;
            let mut pos = 4usize;
            let mut paths = Vec::with_capacity(count.min(256));
            for _ in 0..count {
                if pos + 4 > body.len() {
                    return None;
                }
                let len_units = u32::from_le_bytes(body[pos..pos + 4].try_into().ok()?) as usize;
                pos += 4;
                let byte_len = len_units.checked_mul(2)?;
                if pos.checked_add(byte_len)? > body.len() {
                    return None;
                }
                let mut units = Vec::with_capacity(len_units);
                for chunk in body[pos..pos + byte_len].as_chunks::<2>().0 {
                    units.push(u16::from_le_bytes(*chunk));
                }
                pos += byte_len;
                if !units.is_empty() {
                    paths.push(PathBuf::from(OsString::from_wide(&units)));
                }
            }
            if pos != body.len() || paths.is_empty() {
                return None;
            }
            Some(HandoffCommand::OpenPaths(paths))
        }
        _ => None,
    }
}

/// Arbitrates single-instance ownership on the session pipe *before* platform/GPU init (I-1, I-4).
pub(crate) fn arbitrate(paths: &[PathBuf]) -> Arbitration {
    let cmd = if paths.is_empty() {
        HandoffCommand::NewWindow
    } else {
        HandoffCommand::OpenPaths(paths.to_vec())
    };
    arbitrate_on(&pipe_name(), &cmd)
}

pub(crate) fn arbitrate_on(pipe: &str, cmd: &HandoffCommand) -> Arbitration {
    for _ in 0..6 {
        let dispatcher = Arc::new(Mutex::new(Dispatcher::Buffered(Vec::new())));
        let disp_for_thread = Arc::clone(&dispatcher);
        match try_claim_pipe_server(pipe, MAX_PAYLOAD_BYTES, move |raw_cmd, body| {
            let Some(decoded) = decode_command(raw_cmd, body) else {
                return false;
            };
            let mut guard = disp_for_thread.lock().unwrap_or_else(|e| e.into_inner());
            match &mut *guard {
                Dispatcher::Buffered(buf) => buf.push(decoded),
                Dispatcher::Live(handler) => handler(decoded),
            }
            true
        }) {
            PipeClaim::Claimed(server) => {
                return Arbitration::Primary(Some(InstanceServer {
                    pipe: server,
                    dispatcher,
                }));
            }
            PipeClaim::AlreadyExists => {
                let (raw_cmd, body) = encode_command(cmd);
                if send_framed_handoff(pipe, raw_cmd, &body) {
                    return Arbitration::HandedOff;
                }
                // Existing primary may be shutting down; retry claiming briefly.
                std::thread::sleep(Duration::from_millis(25));
            }
            PipeClaim::Failed => return Arbitration::Primary(None),
        }
    }
    Arbitration::Primary(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn encodes_and_decodes_utf16_commands_round_trip() {
        let input = vec![
            PathBuf::from(r"C:\Temp\hello world.txt"),
            PathBuf::from(r"C:\Temp\日本語.txt"),
        ];
        let (cmd, body) = encode_command(&HandoffCommand::OpenPaths(input.clone()));
        assert_eq!(decode_command(cmd, &body), Some(HandoffCommand::OpenPaths(input)));

        let (nw_cmd, nw_body) = encode_command(&HandoffCommand::NewWindow);
        assert_eq!(
            decode_command(nw_cmd, &nw_body),
            Some(HandoffCommand::NewWindow)
        );
        // Truncated payload must be rejected cleanly.
        assert_eq!(decode_command(cmd, &body[..body.len() - 1]), None);
    }

    #[test]
    fn named_pipe_handoff_delivers_paths_and_new_window_concurrently() {
        let pipe = format!(r"\\.\pipe\Jot.Test.Framed.{}", std::process::id());
        let Arbitration::Primary(Some(server)) = arbitrate_on(&pipe, &HandoffCommand::NewWindow)
        else {
            panic!("first process must claim pipe ownership");
        };

        // Fire a NewWindow handoff BEFORE attach_handler to verify startup buffering (I-1).
        assert!(matches!(
            arbitrate_on(&pipe, &HandoffCommand::NewWindow),
            Arbitration::HandedOff
        ));

        let (tx, rx) = mpsc::channel();
        server.attach_handler(move |cmd| {
            let _ = tx.send(cmd);
        });
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            HandoffCommand::NewWindow
        );

        // Concurrent multi-file launches (simulating Explorer multi-select, I-1) all deliver cleanly.
        let mut threads = Vec::new();
        for i in 0..5 {
            let p = pipe.clone();
            threads.push(std::thread::spawn(move || {
                let paths = vec![PathBuf::from(format!(r"C:\Temp\multi_{i}_日本語.txt"))];
                matches!(
                    arbitrate_on(&p, &HandoffCommand::OpenPaths(paths)),
                    Arbitration::HandedOff
                )
            }));
        }
        for t in threads {
            assert!(t.join().unwrap(), "concurrent handoff must succeed");
        }

        let mut received = Vec::new();
        for _ in 0..5 {
            match rx.recv_timeout(Duration::from_secs(3)).unwrap() {
                HandoffCommand::OpenPaths(ps) => received.extend(ps),
                other => panic!("unexpected handoff command: {other:?}"),
            }
        }
        received.sort();
        assert_eq!(received.len(), 5);
        for (i, path) in received.iter().enumerate() {
            assert_eq!(
                path,
                &PathBuf::from(format!(r"C:\Temp\multi_{i}_日本語.txt"))
            );
        }

        // After stopping acceptance, handoff must be rejected cleanly (I-5).
        server.pipe.stop_accepting();
        let (raw_cmd, body) = encode_command(&HandoffCommand::NewWindow);
        assert!(
            !send_framed_handoff(&pipe, raw_cmd, &body),
            "stopped server must not acknowledge late handoffs"
        );
    }
}
