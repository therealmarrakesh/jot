use std::ffi::{OsString, c_void};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::time::Duration;

use windows::Win32::{
    Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HLOCAL, LPARAM, LocalFree, WPARAM,
    },
    Security::{
        Authorization::ConvertSidToStringSidW, EqualSid, GetTokenInformation, PSID,
        TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenSessionId, TokenUser,
    },
    System::{
        DataExchange::COPYDATASTRUCT,
        Threading::{
            CreateMutexW, GetCurrentProcess, OpenProcess, OpenProcessToken,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
    UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, FindWindowExW, GetWindowThreadProcessId, HWND_MESSAGE,
        SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_COPYDATA,
    },
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

use super::WAKE_CLASS_NAME;

const TAG_NEW_WINDOW: usize = 0x4A4F_5401;
const TAG_OPEN_PATHS: usize = 0x4A4F_5402;
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
const SEND_TIMEOUT_MS: u32 = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HandoffCommand {
    OpenPaths(Vec<PathBuf>),
    NewWindow,
}

pub(crate) struct InstanceLock {
    _mutex: OwnedHandle,
    pub(crate) window_title: String,
}

pub(crate) enum Arbitration {
    Primary(Option<InstanceLock>),
    HandedOff,
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

struct LocalBuf(*mut c_void);

impl Drop for LocalBuf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            let _ = unsafe { LocalFree(Some(HLOCAL(self.0))) };
        }
    }
}

fn query_token_user_bytes(token: HANDLE) -> Option<Vec<u8>> {
    let mut needed = 0u32;
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u8; needed as usize];
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
    let user = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast::<TOKEN_USER>()) };
    Some(user.User.Sid)
}

fn sid_to_string(sid: PSID) -> Option<String> {
    let mut wide = PWSTR::null();
    unsafe { ConvertSidToStringSidW(sid, &mut wide).ok()? };
    let _guard = LocalBuf(wide.0.cast());
    Some(String::from_utf16_lossy(unsafe { wide.as_wide() }))
}

fn instance_key() -> String {
    if let Some(key) = query_process_scope_key() {
        return key;
    }
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".into());
    let sanitized: String = user
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if sanitized.is_empty() {
        "default".to_owned()
    } else {
        sanitized
    }
}

fn query_process_scope_key() -> Option<String> {
    let mut raw_token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token).ok()? };
    let token = OwnedHandle(raw_token);

    let user_buf = query_token_user_bytes(token.0)?;
    let sid = sid_to_string(sid_from_user_buf(&user_buf)?)?;

    let mut elev = TOKEN_ELEVATION::default();
    let mut elev_len = 0u32;
    let elevated = unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            Some((&raw mut elev).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut elev_len,
        )
        .is_ok()
    } && elev.TokenIsElevated != 0;

    let mut session_id = 0u32;
    let mut sess_len = 0u32;
    unsafe {
        let _ = GetTokenInformation(
            token.0,
            TokenSessionId,
            Some((&raw mut session_id).cast()),
            std::mem::size_of::<u32>() as u32,
            &mut sess_len,
        );
    }

    let elev_tag = if elevated { "elev" } else { "std" };
    Some(format!("S{session_id}.{elev_tag}.{sid}"))
}

fn process_belongs_to_current_user(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let mut my_token = HANDLE::default();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut my_token) }.is_err() {
        return false;
    }
    let my_token = OwnedHandle(my_token);
    let Some(my_buf) = query_token_user_bytes(my_token.0) else {
        return false;
    };
    let Some(my_sid) = sid_from_user_buf(&my_buf) else {
        return false;
    };

    let Ok(proc_handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
    else {
        return false;
    };
    let proc_handle = OwnedHandle(proc_handle);

    let mut target_token = HANDLE::default();
    if unsafe { OpenProcessToken(proc_handle.0, TOKEN_QUERY, &mut target_token) }.is_err() {
        return false;
    }
    let target_token = OwnedHandle(target_token);
    let Some(target_buf) = query_token_user_bytes(target_token.0) else {
        return false;
    };
    let Some(target_sid) = sid_from_user_buf(&target_buf) else {
        return false;
    };

    unsafe { EqualSid(my_sid, target_sid).is_ok() }
}

fn encode_payload(cmd: &HandoffCommand) -> (usize, Vec<u8>) {
    match cmd {
        HandoffCommand::NewWindow => (TAG_NEW_WINDOW, Vec::new()),
        HandoffCommand::OpenPaths(paths) => {
            let mut units = Vec::new();
            for (ix, path) in paths.iter().enumerate() {
                if ix > 0 {
                    units.push(0u16);
                }
                let abs = std::path::absolute(path).unwrap_or_else(|_| path.clone());
                units.extend(abs.as_os_str().encode_wide());
            }
            let mut bytes = Vec::with_capacity(units.len() * 2);
            for u in units {
                bytes.extend_from_slice(&u.to_le_bytes());
            }
            (TAG_OPEN_PATHS, bytes)
        }
    }
}

pub(super) fn decode_copydata(cds: &COPYDATASTRUCT) -> Option<HandoffCommand> {
    let len = cds.cbData as usize;
    match cds.dwData {
        TAG_NEW_WINDOW => (len == 0).then_some(HandoffCommand::NewWindow),
        TAG_OPEN_PATHS => {
            if len == 0 || len > MAX_PAYLOAD_BYTES || !len.is_multiple_of(2) || cds.lpData.is_null()
            {
                return None;
            }
            let bytes = unsafe { std::slice::from_raw_parts(cds.lpData.cast::<u8>(), len) };
            let mut units = Vec::with_capacity(len / 2);
            for pair in bytes.as_chunks::<2>().0 {
                units.push(u16::from_le_bytes(*pair));
            }
            let paths: Vec<PathBuf> = units
                .split(|&u| u == 0)
                .filter(|part| !part.is_empty())
                .map(|part| PathBuf::from(OsString::from_wide(part)))
                .collect();
            (!paths.is_empty()).then_some(HandoffCommand::OpenPaths(paths))
        }
        _ => None,
    }
}

enum MutexClaim {
    Claimed(OwnedHandle),
    AlreadyExists,
    Failed,
}

fn try_claim_mutex(name: &str) -> MutexClaim {
    let wide = HSTRING::from(name);
    let Ok(handle) = (unsafe { CreateMutexW(None, false, &wide) }) else {
        return MutexClaim::Failed;
    };
    let err = unsafe { GetLastError() };
    let guard = OwnedHandle(handle);
    if err == ERROR_ALREADY_EXISTS {
        MutexClaim::AlreadyExists
    } else {
        MutexClaim::Claimed(guard)
    }
}

fn send_handoff(class_name: PCWSTR, window_title: &str, cmd: &HandoffCommand) -> bool {
    let title = HSTRING::from(window_title);
    let Ok(target) = (unsafe { FindWindowExW(Some(HWND_MESSAGE), None, class_name, &title) })
    else {
        return false;
    };
    if target.0.is_null() {
        return false;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(target, Some(&mut pid)) };
    if !process_belongs_to_current_user(pid) {
        return false;
    }
    let _ = unsafe { AllowSetForegroundWindow(pid) };

    let (tag, payload) = encode_payload(cmd);
    let Ok(cb_data) = u32::try_from(payload.len()) else {
        return false;
    };
    if payload.len() > MAX_PAYLOAD_BYTES {
        return false;
    }
    let cds = COPYDATASTRUCT {
        dwData: tag,
        cbData: cb_data,
        lpData: if payload.is_empty() {
            std::ptr::null_mut()
        } else {
            payload.as_ptr() as *mut c_void
        },
    };
    let mut result = 0usize;
    let sent = unsafe {
        SendMessageTimeoutW(
            target,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM((&raw const cds) as isize),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            SEND_TIMEOUT_MS,
            Some(&mut result),
        )
    };
    sent.0 != 0 && result == 1
}

pub(crate) fn arbitrate(paths: &[PathBuf]) -> Arbitration {
    let cmd = if paths.is_empty() {
        HandoffCommand::NewWindow
    } else {
        HandoffCommand::OpenPaths(paths.to_vec())
    };
    let key = instance_key();
    arbitrate_for(&key, WAKE_CLASS_NAME, &cmd)
}

fn arbitrate_for(key: &str, class_name: PCWSTR, cmd: &HandoffCommand) -> Arbitration {
    let mutex_name = format!(r"Local\Jot.Mutex.{key}");
    let window_title = format!("Jot.Handoff.{key}");
    for _ in 0..20 {
        match try_claim_mutex(&mutex_name) {
            MutexClaim::Claimed(mutex) => {
                return Arbitration::Primary(Some(InstanceLock {
                    _mutex: mutex,
                    window_title,
                }));
            }
            MutexClaim::AlreadyExists => {
                if send_handoff(class_name, &window_title, cmd) {
                    return Arbitration::HandedOff;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            MutexClaim::Failed => return Arbitration::Primary(None),
        }
    }
    Arbitration::Primary(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    use windows::Win32::{
        Foundation::{HWND, LRESULT},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
            MSG, PostMessageW, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE,
            WM_CLOSE, WNDCLASSW,
        },
    };
    use windows::core::w;

    const TEST_CLASS: PCWSTR = w!("JotTestHandoffWindow");
    static TEST_TX: OnceLock<std::sync::Mutex<Option<mpsc::Sender<HandoffCommand>>>> =
        OnceLock::new();
    static TEST_ACCEPTING: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn test_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_COPYDATA => {
                if !TEST_ACCEPTING.load(Ordering::Acquire) {
                    return LRESULT(0);
                }
                let cds = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
                let Some(cmd) = decode_copydata(cds) else {
                    return LRESULT(0);
                };
                if let Some(tx) = TEST_TX
                    .get_or_init(|| std::sync::Mutex::new(None))
                    .lock()
                    .unwrap()
                    .as_ref()
                {
                    let _ = tx.send(cmd);
                }
                LRESULT(1)
            }
            WM_CLOSE => {
                let _ = unsafe { DestroyWindow(hwnd) };
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    #[test]
    fn encodes_and_decodes_copydata_round_trip() {
        let input = vec![
            PathBuf::from(r"C:\Temp\hello world.txt"),
            PathBuf::from(r"C:\Temp\日本語.txt"),
        ];
        let (tag, bytes) = encode_payload(&HandoffCommand::OpenPaths(input.clone()));
        let cds = COPYDATASTRUCT {
            dwData: tag,
            cbData: bytes.len() as u32,
            lpData: bytes.as_ptr() as *mut c_void,
        };
        assert_eq!(decode_copydata(&cds), Some(HandoffCommand::OpenPaths(input)));

        let nw = COPYDATASTRUCT {
            dwData: TAG_NEW_WINDOW,
            cbData: 0,
            lpData: std::ptr::null_mut(),
        };
        assert_eq!(decode_copydata(&nw), Some(HandoffCommand::NewWindow));

        let bad_len = COPYDATASTRUCT {
            dwData: tag,
            cbData: (bytes.len() - 1) as u32,
            lpData: bytes.as_ptr() as *mut c_void,
        };
        assert_eq!(decode_copydata(&bad_len), None);
    }

    #[test]
    fn mutex_and_copydata_handoff_delivers_concurrently_and_rejects_after_stop() {
        let key = format!("Test.CopyData.{}", std::process::id());
        let Arbitration::Primary(Some(lock)) =
            arbitrate_for(&key, TEST_CLASS, &HandoffCommand::NewWindow)
        else {
            panic!("first process must claim mutex");
        };

        let (tx, rx) = mpsc::channel();
        *TEST_TX
            .get_or_init(|| std::sync::Mutex::new(None))
            .lock()
            .unwrap() = Some(tx);
        TEST_ACCEPTING.store(true, Ordering::Release);

        let (hwnd_tx, hwnd_rx) = mpsc::channel::<isize>();
        let title = lock.window_title.clone();
        let pump = std::thread::spawn(move || {
            let hinstance = unsafe { GetModuleHandleW(None).unwrap() };
            let wc = WNDCLASSW {
                lpfnWndProc: Some(test_wnd_proc),
                hInstance: hinstance.into(),
                lpszClassName: TEST_CLASS,
                ..Default::default()
            };
            unsafe { RegisterClassW(&wc) };
            let wide_title = HSTRING::from(&title);
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    TEST_CLASS,
                    &wide_title,
                    WINDOW_STYLE(0),
                    0,
                    0,
                    0,
                    0,
                    Some(HWND_MESSAGE),
                    None,
                    Some(hinstance.into()),
                    None,
                )
                .unwrap()
            };
            let _ = hwnd_tx.send(hwnd.0 as isize);
            let mut msg = MSG::default();
            while unsafe { GetMessageW(&mut msg, Some(hwnd), 0, 0) }.0 > 0 {
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        });
        let hwnd_raw = hwnd_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        assert!(matches!(
            arbitrate_for(&key, TEST_CLASS, &HandoffCommand::NewWindow),
            Arbitration::HandedOff
        ));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            HandoffCommand::NewWindow
        );

        let mut threads = Vec::new();
        for i in 0..5 {
            let k = key.clone();
            threads.push(std::thread::spawn(move || {
                let paths = vec![PathBuf::from(format!(r"C:\Temp\multi_{i}_日本語.txt"))];
                matches!(
                    arbitrate_for(&k, TEST_CLASS, &HandoffCommand::OpenPaths(paths)),
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

        TEST_ACCEPTING.store(false, Ordering::Release);
        assert!(
            !send_handoff(TEST_CLASS, &lock.window_title, &HandoffCommand::NewWindow),
            "stopped server must reject late handoffs"
        );

        unsafe {
            let _ = PostMessageW(
                Some(HWND(hwnd_raw as *mut _)),
                WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            );
        }
        let _ = pump.join();
        drop(lock);
    }
}
