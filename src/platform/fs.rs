use std::{
    ffi::OsString,
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
};

use windows::Win32::{
    Foundation::{
        ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION,
        ERROR_UNABLE_TO_MOVE_REPLACEMENT_2, HANDLE,
    },
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_HIDDEN, GetFileInformationByHandle,
        REPLACEFILE_IGNORE_ACL_ERRORS, REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW,
    },
    System::Com::CoTaskMemFree,
    UI::Shell::{FOLDERID_Documents, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
};
use windows::core::PCWSTR;

pub(crate) const HIDDEN_FILE_ATTRIBUTE: u32 = FILE_ATTRIBUTE_HIDDEN.0;

pub(crate) fn documents_dir() -> Option<PathBuf> {
    let path = unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None) }.ok()?;
    let documents = (!path.0.is_null()).then(|| {
        PathBuf::from(OsString::from_wide(unsafe { path.as_wide() }))
    });
    unsafe { CoTaskMemFree(Some(path.0.cast())) };
    documents
}

pub(crate) fn file_hard_link_count(path: &Path) -> Option<u32> {
    let file = std::fs::File::open(path).ok()?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.ok()?;
    Some(info.nNumberOfLinks)
}

pub(crate) enum ReplaceFileError {
    UnableToMoveReplacement(io::Error),
    SharingOrAccess(io::Error),
    Other(io::Error),
}

pub(crate) fn replace_file(
    target: &Path,
    replacement: &Path,
    backup: &Path,
) -> Result<(), ReplaceFileError> {
    let wide = |path: &Path| -> Vec<u16> { path.as_os_str().encode_wide().chain(Some(0)).collect() };
    let (replaced, replacement_w, backed_up) = (wide(target), wide(replacement), wide(backup));
    let res = unsafe {
        ReplaceFileW(
            PCWSTR(replaced.as_ptr()),
            PCWSTR(replacement_w.as_ptr()),
            PCWSTR(backed_up.as_ptr()),
            REPLACEFILE_IGNORE_MERGE_ERRORS | REPLACEFILE_IGNORE_ACL_ERRORS,
            None,
            None,
        )
    };
    let Err(err) = res else {
        return Ok(());
    };
    let code = err.code();
    let win32_err = (code.0 as u32) & 0xFFFF;
    let io_err = io::Error::from_raw_os_error(win32_err as i32);
    if code == ERROR_UNABLE_TO_MOVE_REPLACEMENT_2.to_hresult() {
        Err(ReplaceFileError::UnableToMoveReplacement(io_err))
    } else if code == ERROR_SHARING_VIOLATION.to_hresult()
        || code == ERROR_LOCK_VIOLATION.to_hresult()
        || code == ERROR_ACCESS_DENIED.to_hresult()
    {
        Err(ReplaceFileError::SharingOrAccess(io_err))
    } else {
        Err(ReplaceFileError::Other(io_err))
    }
}
