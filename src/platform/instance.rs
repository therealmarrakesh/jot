use std::ffi::{OsString, c_void};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::time::Duration;

use windows::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HLOCAL, LPARAM, WPARAM},
    Security::{
        Authorization::ConvertSidToStringSidW, EqualSid, GetTokenInformation, PSID,
        TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenUser,
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
use windows::core::{HSTRING, Owned, PCWSTR, PWSTR};

use super::pump::WAKE_CLASS_NAME;

const COPYDATA_TAG: usize = 0x4A4F_5401;
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
const SEND_TIMEOUT_MS: u32 = 2000;

pub(crate) struct InstanceLock {
    _mutex: Owned<HANDLE>,
    pub(super) window_title: String,
}

pub(crate) enum Arbitration {
    Primary(Option<InstanceLock>),
    HandedOff,
}

struct ProcessIdentity {
    user_buf: Vec<u8>,
    key: String,
}

impl ProcessIdentity {
    fn current() -> Option<Self> {
        let mut raw_token = HANDLE::default();
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token).ok()? };
        let token = unsafe { Owned::new(raw_token) };

        let user_buf = query_token_user_bytes(*token)?;
        let sid = sid_from_user_buf(&user_buf)?;
        let sid_str = sid_to_string(sid)?;

        let mut elev = TOKEN_ELEVATION::default();
        let mut elev_len = 0u32;
        let elevated = unsafe {
            GetTokenInformation(
                *token,
                TokenElevation,
                Some((&raw mut elev).cast()),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut elev_len,
            )
            .is_ok()
        } && elev.TokenIsElevated != 0;

        let elev_tag = if elevated { "elev" } else { "std" };
        Some(Self {
            user_buf,
            key: format!("{elev_tag}.{sid_str}"),
        })
    }

    fn matches_pid(&self, pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        let Some(my_sid) = sid_from_user_buf(&self.user_buf) else {
            return false;
        };
        let Ok(raw_proc) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
        else {
            return false;
        };
        let proc_handle = unsafe { Owned::new(raw_proc) };

        let mut raw_token = HANDLE::default();
        if unsafe { OpenProcessToken(*proc_handle, TOKEN_QUERY, &mut raw_token) }.is_err() {
            return false;
        }
        let target_token = unsafe { Owned::new(raw_token) };
        let Some(target_buf) = query_token_user_bytes(*target_token) else {
            return false;
        };
        let Some(target_sid) = sid_from_user_buf(&target_buf) else {
            return false;
        };
        unsafe { EqualSid(my_sid, target_sid).is_ok() }
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
    if buf.len() < size_of::<TOKEN_USER>() {
        return None;
    }
    let user = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast::<TOKEN_USER>()) };
    Some(user.User.Sid)
}

fn sid_to_string(sid: PSID) -> Option<String> {
    let mut wide = PWSTR::null();
    unsafe { ConvertSidToStringSidW(sid, &mut wide).ok()? };
    let _guard = unsafe { Owned::new(HLOCAL(wide.0.cast())) };
    Some(String::from_utf16_lossy(unsafe { wide.as_wide() }))
}

fn encode_paths(paths: &[PathBuf]) -> Vec<u8> {
    if paths.is_empty() {
        return Vec::new();
    }
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
    bytes
}

pub(super) fn decode_copydata(cds: &COPYDATASTRUCT) -> Option<Vec<PathBuf>> {
    if cds.dwData != COPYDATA_TAG {
        return None;
    }
    let len = cds.cbData as usize;
    if len == 0 {
        return Some(Vec::new());
    }
    if len > MAX_PAYLOAD_BYTES || !len.is_multiple_of(2) || cds.lpData.is_null() {
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
    (!paths.is_empty()).then_some(paths)
}

fn try_claim_mutex(name: &str) -> std::result::Result<Option<Owned<HANDLE>>, ()> {
    let wide = HSTRING::from(name);
    let handle = unsafe { CreateMutexW(None, false, &wide) }.map_err(|_| ())?;
    let err = unsafe { GetLastError() };
    let guard = unsafe { Owned::new(handle) };
    Ok((err != ERROR_ALREADY_EXISTS).then_some(guard))
}

fn send_handoff(
    identity: &ProcessIdentity,
    class_name: PCWSTR,
    window_title: &str,
    paths: &[PathBuf],
) -> bool {
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
    if !identity.matches_pid(pid) {
        return false;
    }
    let _ = unsafe { AllowSetForegroundWindow(pid) };

    let payload = encode_paths(paths);
    let Ok(cb_data) = u32::try_from(payload.len()) else {
        return false;
    };
    if payload.len() > MAX_PAYLOAD_BYTES {
        return false;
    }
    let cds = COPYDATASTRUCT {
        dwData: COPYDATA_TAG,
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
    let Some(identity) = ProcessIdentity::current() else {
        return Arbitration::Primary(None);
    };
    arbitrate_for(&identity, &identity.key, paths)
}

fn arbitrate_for(identity: &ProcessIdentity, key: &str, paths: &[PathBuf]) -> Arbitration {
    let mutex_name = format!(r"Local\Jot.Mutex.{key}");
    let window_title = format!("Jot.Handoff.{key}");
    for _ in 0..20 {
        match try_claim_mutex(&mutex_name) {
            Ok(Some(mutex)) => {
                return Arbitration::Primary(Some(InstanceLock {
                    _mutex: mutex,
                    window_title,
                }));
            }
            Ok(None) => {
                if send_handoff(identity, WAKE_CLASS_NAME, &window_title, paths) {
                    return Arbitration::HandedOff;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(()) => return Arbitration::Primary(None),
        }
    }
    Arbitration::Primary(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc;

    use crate::platform::{Jobs, Platform, PlatformEvent, PlatformHandler};

    #[test]
    fn encodes_and_decodes_copydata_round_trip() {
        let input = vec![
            PathBuf::from(r"C:\Temp\hello world.txt"),
            PathBuf::from(r"C:\Temp\日本語.txt"),
        ];
        let bytes = encode_paths(&input);
        let cds = COPYDATASTRUCT {
            dwData: COPYDATA_TAG,
            cbData: bytes.len() as u32,
            lpData: bytes.as_ptr() as *mut c_void,
        };
        assert_eq!(decode_copydata(&cds), Some(input));

        let nw = COPYDATASTRUCT {
            dwData: COPYDATA_TAG,
            cbData: 0,
            lpData: std::ptr::null_mut(),
        };
        assert_eq!(decode_copydata(&nw), Some(Vec::new()));

        let bad_tag = COPYDATASTRUCT {
            dwData: 0xDEAD_BEEF,
            cbData: 0,
            lpData: std::ptr::null_mut(),
        };
        assert_eq!(decode_copydata(&bad_tag), None);

        let bad_len = COPYDATASTRUCT {
            dwData: COPYDATA_TAG,
            cbData: (bytes.len() - 1) as u32,
            lpData: bytes.as_ptr() as *mut c_void,
        };
        assert_eq!(decode_copydata(&bad_len), None);
    }

    #[test]
    fn mutex_and_copydata_handoff_delivers_concurrently_and_rejects_after_stop() {
        struct Server {
            platform: Platform,
            tx: mpsc::Sender<Vec<PathBuf>>,
            ctrl: Rc<Jobs<bool>>,
            stop_tx: Option<mpsc::Sender<()>>,
        }

        impl PlatformHandler for Server {
            fn on_event(&mut self, event: PlatformEvent) -> bool {
                if let PlatformEvent::Launch { paths } = event {
                    let _ = self.tx.send(paths);
                }
                false
            }
            fn on_wake(&mut self) {
                while let Some(quit) = self.ctrl.pop() {
                    if quit {
                        self.platform.post_quit();
                    } else {
                        self.platform.disable_handoff();
                        if let Some(tx) = self.stop_tx.take() {
                            let _ = tx.send(());
                        }
                    }
                }
            }
            fn drive_frames(&mut self) {}
        }

        let identity = ProcessIdentity::current().expect("current process identity");
        let key = format!("Test.CopyData.{}", std::process::id());
        let window_title = format!("Jot.Handoff.{key}");

        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel();
        let (ctrl_tx, ctrl_rx) = mpsc::channel();

        let server_key = key.clone();
        let pump = std::thread::spawn(move || {
            let id = ProcessIdentity::current().expect("server process identity");
            let Arbitration::Primary(Some(lock)) = arbitrate_for(&id, &server_key, &[]) else {
                panic!("first process must claim mutex");
            };
            let platform = Platform::new(Some(lock)).expect("Platform::new");
            let ctrl = Rc::new(Jobs::new());
            ctrl.attach_platform(&platform);
            let _ = ctrl_tx.send(ctrl.wake_poster());
            let server = Rc::new(RefCell::new(Server {
                platform,
                tx,
                ctrl,
                stop_tx: Some(stop_tx),
            }));
            Platform::run(server, |_| {
                let _ = ready_tx.send(());
            });
        });
        let post_ctrl = ctrl_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        assert!(matches!(
            arbitrate_for(&identity, &key, &[]),
            Arbitration::HandedOff
        ));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Vec::<PathBuf>::new());

        let mut threads = Vec::new();
        for i in 0..5 {
            let k = key.clone();
            threads.push(std::thread::spawn(move || {
                let id = ProcessIdentity::current().unwrap();
                let paths = vec![PathBuf::from(format!(r"C:\Temp\multi_{i}_日本語.txt"))];
                matches!(arbitrate_for(&id, &k, &paths), Arbitration::HandedOff)
            }));
        }
        for t in threads {
            assert!(t.join().unwrap(), "concurrent handoff must succeed");
        }

        let mut received = Vec::new();
        for _ in 0..5 {
            received.extend(rx.recv_timeout(Duration::from_secs(3)).unwrap());
        }
        received.sort();
        assert_eq!(received.len(), 5);
        for (i, path) in received.iter().enumerate() {
            assert_eq!(
                path,
                &PathBuf::from(format!(r"C:\Temp\multi_{i}_日本語.txt"))
            );
        }

        post_ctrl(false);
        stop_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            !send_handoff(&identity, WAKE_CLASS_NAME, &window_title, &[]),
            "stopped server must reject late handoffs"
        );

        post_ctrl(true);
        let _ = pump.join();
    }
}
