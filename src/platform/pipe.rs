//! Hardened Win32 named-pipe client/server for single-instance handoff (I-1..I-5).
//!
//! Security & reliability guarantees:
//! - Pipe name scoped by current user SID, session ID, and elevation status (I-2, I-3).
//! - Owner-only DACL (`D:(A;;GA;;;{sid})`), `PIPE_REJECT_REMOTE_CLIENTS`, and
//!   `FILE_FLAG_FIRST_PIPE_INSTANCE` for atomic primary arbitration (I-1, I-2).
//! - Client verifies `GetNamedPipeServerProcessId` + server user SID before writing, and calls
//!   `AllowSetForegroundWindow(server_pid)` instead of `ASFW_ANY` (I-2).
//! - Server uses overlapped I/O with bounded deadlines and never exits on transient errors (I-2, I-5).
//! - Framed wire protocol with magic, version, length, command byte, and 1-byte ack (I-4, I-5).

use std::ffi::c_void;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use windows::Win32::{
    Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_IO_PENDING, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
        GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, HLOCAL, LocalFree, WAIT_OBJECT_0,
    },
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1,
        },
        EqualSid, GetTokenInformation, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
        TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenSessionId, TokenUser,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE,
        FlushFileBuffers, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
    },
    System::{
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
            PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
            PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
        },
        Threading::{
            CreateEventW, GetCurrentProcess, OpenProcess, OpenProcessToken,
            PROCESS_QUERY_LIMITED_INFORMATION, ResetEvent, WaitForSingleObject,
        },
    },
    UI::WindowsAndMessaging::AllowSetForegroundWindow,
};
use windows::core::{PCWSTR, PWSTR};

const PIPE_BUFFER_SIZE: u32 = 16 * 1024;
const FRAME_MAGIC: [u8; 4] = *b"JOT1";
const FRAME_VERSION: u16 = 1;
const HEADER_LEN: usize = 12;
const ACK_OK: u8 = 0x01;
const ACK_REJECT: u8 = 0x00;
const READ_TIMEOUT: Duration = Duration::from_secs(2);
const WRITE_TIMEOUT: Duration = Duration::from_millis(500);
const CLIENT_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Owned Win32 `HANDLE` that closes on drop.
struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: `self.0` is a valid owned Win32 handle not closed elsewhere.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// Owned `LocalAlloc` pointer freed via `LocalFree` on drop.
struct LocalBuf(*mut c_void);

impl Drop for LocalBuf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` was allocated by a Win32 API that requires `LocalFree`.
            let _ = unsafe { LocalFree(Some(HLOCAL(self.0))) };
        }
    }
}

/// Process security identity used to scope the single-instance pipe name and verify peers.
pub(crate) struct ProcessIdentity {
    pub(crate) sid: String,
    pub(crate) session_id: u32,
    pub(crate) elevated: bool,
}

fn query_token_user_bytes(token: HANDLE) -> Option<Vec<u8>> {
    let mut needed = 0u32;
    // SAFETY: Querying required buffer length with `None` buffer is documented Win32 usage.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u8; needed as usize];
    // SAFETY: `buf` has length `needed` bytes and stays alive for the duration of the call.
    unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
        .ok()?;
    }
    Some(buf)
}

fn sid_from_user_buf(buf: &[u8]) -> Option<PSID> {
    if buf.len() < std::mem::size_of::<TOKEN_USER>() {
        return None;
    }
    // SAFETY: `GetTokenInformation(TokenUser)` initializes a `TOKEN_USER` header at offset 0.
    let user = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast::<TOKEN_USER>()) };
    Some(user.User.Sid)
}

fn sid_to_string(sid: PSID) -> Option<String> {
    let mut wide = PWSTR::null();
    // SAFETY: `sid` is a valid SID pointer owned by the caller's token buffer.
    unsafe { ConvertSidToStringSidW(sid, &mut wide).ok()? };
    let _guard = LocalBuf(wide.0.cast());
    // SAFETY: `ConvertSidToStringSidW` returns a NUL-terminated UTF-16 string.
    let slice = unsafe { wide.as_wide() };
    Some(String::from_utf16_lossy(slice))
}

/// Queries the current process's user SID, session ID, and elevation flag (I-2, I-3).
pub(crate) fn current_process_identity() -> Option<ProcessIdentity> {
    let mut raw_token = HANDLE::default();
    // SAFETY: `GetCurrentProcess()` returns a valid pseudo-handle.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token).ok()? };
    let token = OwnedHandle(raw_token);

    let user_buf = query_token_user_bytes(token.raw())?;
    let sid = sid_to_string(sid_from_user_buf(&user_buf)?)?;

    let mut elev = TOKEN_ELEVATION::default();
    let mut elev_len = 0u32;
    // SAFETY: `elev` is properly sized for `TOKEN_ELEVATION`.
    let elevated = unsafe {
        GetTokenInformation(
            token.raw(),
            TokenElevation,
            Some((&raw mut elev).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut elev_len,
        )
        .is_ok()
    } && elev.TokenIsElevated != 0;

    let mut session_id = 0u32;
    let mut sess_len = 0u32;
    // SAFETY: `session_id` is properly sized (`u32`) for `TokenSessionId`.
    unsafe {
        let _ = GetTokenInformation(
            token.raw(),
            TokenSessionId,
            Some((&raw mut session_id).cast()),
            std::mem::size_of::<u32>() as u32,
            &mut sess_len,
        );
    }

    Some(ProcessIdentity {
        sid,
        session_id,
        elevated,
    })
}

fn server_belongs_to_current_user(server_pid: u32) -> bool {
    if server_pid == 0 {
        return false;
    }
    let mut my_token = HANDLE::default();
    // SAFETY: `GetCurrentProcess()` is a valid pseudo-handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut my_token) }.is_err() {
        return false;
    }
    let my_token = OwnedHandle(my_token);
    let Some(my_buf) = query_token_user_bytes(my_token.raw()) else {
        return false;
    };
    let Some(my_sid) = sid_from_user_buf(&my_buf) else {
        return false;
    };

    // SAFETY: Opening the server process with minimal query rights to check its token SID.
    let Ok(proc_handle) =
        (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, server_pid) })
    else {
        return false;
    };
    let proc_handle = OwnedHandle(proc_handle);

    let mut srv_token = HANDLE::default();
    // SAFETY: `proc_handle` is a valid process handle opened with query rights.
    if unsafe { OpenProcessToken(proc_handle.raw(), TOKEN_QUERY, &mut srv_token) }.is_err() {
        return false;
    }
    let srv_token = OwnedHandle(srv_token);
    let Some(srv_buf) = query_token_user_bytes(srv_token.raw()) else {
        return false;
    };
    let Some(srv_sid) = sid_from_user_buf(&srv_buf) else {
        return false;
    };

    // SAFETY: Both `my_sid` and `srv_sid` point inside live `TOKEN_USER` buffers.
    unsafe { EqualSid(my_sid, srv_sid).is_ok() }
}

/// Builds an owner-only security descriptor granting `GENERIC_ALL` strictly to the current user SID.
fn create_owner_only_sd() -> Option<(PSECURITY_DESCRIPTOR, LocalBuf)> {
    let id = current_process_identity()?;
    let sddl = format!("D:(A;;GA;;;{})", id.sid);
    let wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `wide` is NUL-terminated UTF-16; `sd` receives a `LocalAlloc` descriptor on success.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(wide.as_ptr()),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )
        .ok()?;
    }
    let guard = LocalBuf(sd.0);
    Some((sd, guard))
}

/// Outcome of attempting to atomically claim the first server instance of a named pipe.
pub(crate) enum PipeClaim {
    /// We won primary ownership; the server thread is already listening on this pipe.
    Claimed(PipeServer),
    /// Another instance already owns this pipe name (`ERROR_ACCESS_DENIED`).
    AlreadyExists,
    /// Pipe creation failed for another reason (caller should fall back to standalone mode).
    Failed,
}

/// Running single-instance pipe server handle. Dropping or calling `stop_accepting()` stops
/// accepting new handoffs so late launches start a fresh instance cleanly (I-5).
pub(crate) struct PipeServer {
    accepting: Arc<AtomicBool>,
}

impl PipeServer {
    pub(crate) fn accepting_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.accepting)
    }

    pub(crate) fn stop_accepting(&self) {
        self.accepting.store(false, Ordering::Release);
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        self.stop_accepting();
    }
}

/// Atomically claims the first instance of `pipe` with an owner-only DACL and starts an overlapped
/// background listener thread immediately (I-1, I-2, I-5).
pub(crate) fn try_claim_pipe_server(
    pipe: &str,
    max_payload_bytes: usize,
    on_frame: impl Fn(u8, &[u8]) -> bool + Send + 'static,
) -> PipeClaim {
    let wide: Vec<u16> = pipe.encode_utf16().chain(Some(0)).collect();
    let sd_opt = create_owner_only_sd();
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd_opt.as_ref().map_or(std::ptr::null_mut(), |(sd, _)| sd.0),
        bInheritHandle: false.into(),
    };
    let sa_ptr = sd_opt.as_ref().map(|_| &raw const sa);

    // SAFETY: `wide` is NUL-terminated UTF-16 and `sa_ptr` (when `Some`) points to a valid
    // `SECURITY_ATTRIBUTES` whose security descriptor stays alive until after this call returns.
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(wide.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            PIPE_BUFFER_SIZE,
            PIPE_BUFFER_SIZE,
            0,
            sa_ptr,
        )
    };
    drop(sd_opt);

    if handle.is_invalid() {
        // SAFETY: `GetLastError` reads the calling thread's last-error value.
        let err = unsafe { GetLastError() };
        return if err == ERROR_ACCESS_DENIED {
            PipeClaim::AlreadyExists
        } else {
            PipeClaim::Failed
        };
    }

    let accepting = Arc::new(AtomicBool::new(true));
    let thread_accepting = Arc::clone(&accepting);
    // Win32 named-pipe `HANDLE`s are kernel handles valid across threads within the same process.
    let raw_handle = handle.0 as usize;
    let spawn_res = std::thread::Builder::new()
        .name("jot-instance-pipe".into())
        .spawn(move || {
            let pipe_handle = OwnedHandle(HANDLE(raw_handle as _));
            run_pipe_server_loop(
                pipe_handle.raw(),
                &thread_accepting,
                max_payload_bytes,
                on_frame,
            );
        });
    if spawn_res.is_err() {
        // SAFETY: Thread did not start, so we still own `handle` and must close it.
        let _ = unsafe { CloseHandle(handle) };
        return PipeClaim::Failed;
    }

    PipeClaim::Claimed(PipeServer { accepting })
}

fn run_pipe_server_loop(
    handle: HANDLE,
    accepting: &AtomicBool,
    max_payload_bytes: usize,
    on_frame: impl Fn(u8, &[u8]) -> bool,
) {
    // SAFETY: Creating an unnamed manual-reset Win32 event for overlapped pipe I/O.
    let Ok(ev_raw) = (unsafe { CreateEventW(None, true, false, None) }) else {
        return;
    };
    let event = OwnedHandle(ev_raw);

    let mut header = [0u8; HEADER_LEN];
    let mut body = Vec::new();

    'outer: while accepting.load(Ordering::Acquire) {
        // SAFETY: `event` is a valid manual-reset event handle.
        let _ = unsafe { ResetEvent(event.raw()) };
        let mut ov = OVERLAPPED {
            hEvent: event.raw(),
            ..Default::default()
        };
        // SAFETY: `handle` is an overlapped pipe handle and `ov` stays pinned on the stack until
        // either immediate completion or `GetOverlappedResult(..., true)` after `CancelIoEx`.
        let connect_res = unsafe { ConnectNamedPipe(handle, Some(&mut ov)) };
        let connected = match connect_res {
            Ok(()) => true,
            Err(_) => {
                // SAFETY: Reading thread-local error immediately after `ConnectNamedPipe`.
                let err = unsafe { GetLastError() };
                if err == ERROR_PIPE_CONNECTED {
                    true
                } else if err == ERROR_IO_PENDING {
                    loop {
                        // SAFETY: Waiting on our valid overlapped event with a 200ms poll interval.
                        let wait = unsafe { WaitForSingleObject(event.raw(), 200) };
                        if wait == WAIT_OBJECT_0 {
                            let mut transferred = 0u32;
                            // SAFETY: Event is signaled, so `GetOverlappedResult` reads completion.
                            break unsafe {
                                GetOverlappedResult(handle, &ov, &mut transferred, false).is_ok()
                            };
                        }
                        if !accepting.load(Ordering::Acquire) {
                            // SAFETY: Cancel pending I/O and wait for kernel completion before `ov` drops.
                            unsafe {
                                let _ = CancelIoEx(handle, Some(&ov));
                                let mut transferred = 0u32;
                                let _ = GetOverlappedResult(handle, &ov, &mut transferred, true);
                            }
                            break 'outer;
                        }
                    }
                } else {
                    false
                }
            }
        };

        if !connected {
            // SAFETY: Disconnect any half-open client before looping back (I-5).
            let _ = unsafe { DisconnectNamedPipe(handle) };
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }

        if !accepting.load(Ordering::Acquire) {
            let _ = write_all_overlapped(
                handle,
                event.raw(),
                &[ACK_REJECT],
                Instant::now() + WRITE_TIMEOUT,
            );
            // SAFETY: Flush ack and disconnect before shutting down.
            unsafe {
                let _ = FlushFileBuffers(handle);
                let _ = DisconnectNamedPipe(handle);
            }
            break;
        }

        let deadline = Instant::now() + READ_TIMEOUT;
        let mut ack = ACK_REJECT;
        if read_exact_overlapped(handle, event.raw(), &mut header, deadline)
            && header[0..4] == FRAME_MAGIC
            && u16::from_le_bytes([header[4], header[5]]) == FRAME_VERSION
        {
            let cmd = header[6];
            let body_len =
                u32::from_le_bytes([header[8], header[9], header[10], header[11]]) as usize;
            if body_len <= max_payload_bytes {
                body.resize(body_len, 0);
                if (body_len == 0 || read_exact_overlapped(handle, event.raw(), &mut body, deadline))
                    && accepting.load(Ordering::Acquire)
                    && on_frame(cmd, &body)
                {
                    ack = ACK_OK;
                }
            }
        }

        let _ = write_all_overlapped(
            handle,
            event.raw(),
            &[ack],
            Instant::now() + WRITE_TIMEOUT,
        );
        // SAFETY: Flush the 1-byte ack and disconnect the client while keeping the pipe handle open.
        unsafe {
            let _ = FlushFileBuffers(handle);
            let _ = DisconnectNamedPipe(handle);
        }
    }
}

fn read_exact_overlapped(
    handle: HANDLE,
    event: HANDLE,
    buf: &mut [u8],
    deadline: Instant,
) -> bool {
    let mut offset = 0usize;
    while offset < buf.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        // SAFETY: `event` is a valid manual-reset event handle.
        let _ = unsafe { ResetEvent(event) };
        let mut ov = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };
        let mut n = 0u32;
        // SAFETY: `buf[offset..]` and `ov` stay valid until I/O finishes or is cancelled+drained.
        let res = unsafe { ReadFile(handle, Some(&mut buf[offset..]), Some(&mut n), Some(&mut ov)) };
        match res {
            Ok(()) => {
                if n == 0 {
                    return false;
                }
                offset += n as usize;
            }
            Err(_) => {
                // SAFETY: Reading thread-local last error immediately after `ReadFile`.
                if unsafe { GetLastError() } != ERROR_IO_PENDING {
                    return false;
                }
                let timeout_ms = remaining.as_millis().min(u32::MAX as u128) as u32;
                // SAFETY: Waiting on our valid overlapped event up to the remaining deadline.
                if unsafe { WaitForSingleObject(event, timeout_ms) } != WAIT_OBJECT_0 {
                    // SAFETY: Cancel and drain overlapped read before `buf` / `ov` go out of scope.
                    unsafe {
                        let _ = CancelIoEx(handle, Some(&ov));
                        let _ = GetOverlappedResult(handle, &ov, &mut n, true);
                    }
                    return false;
                }
                // SAFETY: Event is signaled; query the completed byte count.
                if unsafe { GetOverlappedResult(handle, &ov, &mut n, false) }.is_err() || n == 0 {
                    return false;
                }
                offset += n as usize;
            }
        }
    }
    true
}

fn write_all_overlapped(handle: HANDLE, event: HANDLE, buf: &[u8], deadline: Instant) -> bool {
    let mut offset = 0usize;
    while offset < buf.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        // SAFETY: `event` is a valid manual-reset event handle.
        let _ = unsafe { ResetEvent(event) };
        let mut ov = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };
        let mut n = 0u32;
        // SAFETY: `buf[offset..]` and `ov` stay valid until I/O finishes or is cancelled+drained.
        let res = unsafe { WriteFile(handle, Some(&buf[offset..]), Some(&mut n), Some(&mut ov)) };
        match res {
            Ok(()) => {
                if n == 0 {
                    return false;
                }
                offset += n as usize;
            }
            Err(_) => {
                // SAFETY: Reading thread-local last error immediately after `WriteFile`.
                if unsafe { GetLastError() } != ERROR_IO_PENDING {
                    return false;
                }
                let timeout_ms = remaining.as_millis().min(u32::MAX as u128) as u32;
                // SAFETY: Waiting on our valid overlapped event up to the remaining deadline.
                if unsafe { WaitForSingleObject(event, timeout_ms) } != WAIT_OBJECT_0 {
                    // SAFETY: Cancel and drain overlapped write before `buf` / `ov` go out of scope.
                    unsafe {
                        let _ = CancelIoEx(handle, Some(&ov));
                        let _ = GetOverlappedResult(handle, &ov, &mut n, true);
                    }
                    return false;
                }
                // SAFETY: Event is signaled; query the completed byte count.
                if unsafe { GetOverlappedResult(handle, &ov, &mut n, false) }.is_err() || n == 0 {
                    return false;
                }
                offset += n as usize;
            }
        }
    }
    true
}

/// Sends a framed handoff command to an existing same-user pipe server, retrying with
/// `WaitNamedPipeW` on `ERROR_PIPE_BUSY`, verifying the server's user SID, granting foreground
/// permission strictly to `server_pid`, and awaiting a 1-byte `ACK_OK` (I-1, I-2, I-5).
pub(crate) fn send_framed_handoff(pipe: &str, cmd: u8, body: &[u8]) -> bool {
    let Ok(body_len) = u32::try_from(body.len()) else {
        return false;
    };
    let wide: Vec<u16> = pipe.encode_utf16().chain(Some(0)).collect();
    let deadline = Instant::now() + CLIENT_CONNECT_TIMEOUT;

    let handle = loop {
        // SAFETY: `wide` is a valid NUL-terminated UTF-16 pipe path.
        match unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )
        } {
            Ok(h) => break OwnedHandle(h),
            Err(err) => {
                let busy = err.code() == ERROR_PIPE_BUSY.to_hresult();
                if !busy {
                    return false;
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return false;
                }
                let wait_ms = remaining.as_millis().min(250) as u32;
                // SAFETY: `wide` is NUL-terminated UTF-16; waits briefly for a busy server instance.
                let _ = unsafe { WaitNamedPipeW(PCWSTR(wide.as_ptr()), wait_ms.max(10)) };
            }
        }
    };

    // Verify server PID belongs to the current user and grant foreground permission strictly to it.
    let mut server_pid = 0u32;
    // SAFETY: `handle.raw()` is a valid connected named-pipe handle.
    if unsafe { GetNamedPipeServerProcessId(handle.raw(), &mut server_pid) }.is_err() {
        return false;
    }
    if !server_belongs_to_current_user(server_pid) {
        return false;
    }
    // SAFETY: `server_pid` is the verified PID of the same-user Jot server process.
    let _ = unsafe { AllowSetForegroundWindow(server_pid) };

    // SAFETY: Creating an unnamed manual-reset event for bounded overlapped client I/O.
    let Ok(ev_raw) = (unsafe { CreateEventW(None, true, false, None) }) else {
        return false;
    };
    let event = OwnedHandle(ev_raw);

    let mut frame = Vec::with_capacity(HEADER_LEN + body.len());
    frame.extend_from_slice(&FRAME_MAGIC);
    frame.extend_from_slice(&FRAME_VERSION.to_le_bytes());
    frame.push(cmd);
    frame.push(0);
    frame.extend_from_slice(&body_len.to_le_bytes());
    frame.extend_from_slice(body);

    let io_deadline = Instant::now() + READ_TIMEOUT;
    if !write_all_overlapped(handle.raw(), event.raw(), &frame, io_deadline) {
        return false;
    }
    let mut ack = [0u8; 1];
    read_exact_overlapped(handle.raw(), event.raw(), &mut ack, io_deadline) && ack[0] == ACK_OK
}
