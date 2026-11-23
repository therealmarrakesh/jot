//! Clean-room Win32 Common Item Dialogs (`IFileOpenDialog` & `IFileSaveDialog` with custom
//! encoding dropdown) executed on a dedicated STA thread.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::thread;

use windows::Win32::{
    Foundation::{ERROR_CANCELLED, HWND},
    System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoTaskMemFree, CoUninitialize,
    },
    UI::Shell::{
        Common::COMDLG_FILTERSPEC,
        FOS_ALLOWMULTISELECT, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST,
        FOS_PICKFOLDERS, FileOpenDialog, FileSaveDialog, IFileDialogCustomize, IFileOpenDialog,
        IFileSaveDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    },
};
use windows::core::{HSTRING, Interface, PCWSTR};

use crate::win32::WindowHwnd;

use super::{PathPromptOptions, SavePathOptions};
use crate::ui::anyhow::Result;
use crate::ui::runtime::{ForegroundExecutor, futures::channel::oneshot};

const ENCODING_COMBO_ID: u32 = 1001;

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn path_to_wide_null(p: &Path) -> Vec<u16> {
    p.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

unsafe fn shell_item_path(item: &IShellItem) -> Result<PathBuf> {
    let pwstr = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH)? };
    let path = unsafe {
        let mut len = 0;
        while *pwstr.0.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(pwstr.0, len);
        let os = OsString::from_wide(slice);
        CoTaskMemFree(Some(pwstr.0.cast()));
        PathBuf::from(os)
    };
    Ok(path)
}

fn is_cancelled(err: &windows::core::Error) -> bool {
    err.code() == ERROR_CANCELLED.to_hresult()
}

pub(crate) fn prompt_for_paths(
    options: PathPromptOptions,
) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
    let (tx, rx) = oneshot::channel();
    thread::Builder::new()
        .name("jot-open-dialog".into())
        .spawn(move || {
            let result = run_open_dialog(options);
            let _ = tx.send(result);
        })
        .expect("failed to spawn open dialog thread");
    rx
}

fn run_open_dialog(options: PathPromptOptions) -> Result<Option<Vec<PathBuf>>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let result = (|| -> Result<Option<Vec<PathBuf>>> {
        let dialog: IFileOpenDialog =
            unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)? };
        let mut flags = unsafe { dialog.GetOptions()? } | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST;
        if options.multiple {
            flags |= FOS_ALLOWMULTISELECT;
        }
        if options.directories {
            flags |= FOS_PICKFOLDERS;
        }
        unsafe { dialog.SetOptions(flags)? };
        if let Some(prompt) = &options.prompt {
            let wide = HSTRING::from(prompt.as_str());
            unsafe { dialog.SetOkButtonLabel(&wide)? };
        }
        if let Err(err) = unsafe { dialog.Show(None) } {
            if is_cancelled(&err) {
                return Ok(None);
            }
            return Err(err.into());
        }
        let items = unsafe { dialog.GetResults()? };
        let count = unsafe { items.GetCount()? };
        let mut paths = Vec::with_capacity(count as usize);
        for ix in 0..count {
            let item = unsafe { items.GetItemAt(ix)? };
            paths.push(unsafe { shell_item_path(&item)? });
        }
        Ok(Some(paths))
    })();
    unsafe { CoUninitialize() };
    result
}

pub(crate) type SavePathResult = Result<Option<(PathBuf, Option<usize>)>>;

pub(crate) fn prompt_for_save_path(
    owner: WindowHwnd,
    _foreground: &ForegroundExecutor,
    options: SavePathOptions,
) -> oneshot::Receiver<SavePathResult> {
    let (tx, rx) = oneshot::channel();
    let owner_raw = owner.raw().0 as isize;
    thread::Builder::new()
        .name("jot-save-dialog".into())
        .spawn(move || {
            let result = run_save_dialog(HWND(owner_raw as *mut _), options);
            let _ = tx.send(result);
        })
        .expect("failed to spawn save dialog thread");
    rx
}

fn run_save_dialog(
    owner: HWND,
    options: SavePathOptions,
) -> Result<Option<(PathBuf, Option<usize>)>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let result = (|| -> Result<Option<(PathBuf, Option<usize>)>> {
        let dialog: IFileSaveDialog =
            unsafe { CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)? };
        let flags = unsafe { dialog.GetOptions()? }
            | FOS_FORCEFILESYSTEM
            | FOS_PATHMUSTEXIST
            | FOS_OVERWRITEPROMPT;
        unsafe { dialog.SetOptions(flags)? };

        if let Some(title) = &options.title {
            let wide = HSTRING::from(title.as_str());
            unsafe { dialog.SetTitle(&wide)? };
        }
        if !options.directory.as_os_str().is_empty() {
            let wide = path_to_wide_null(&options.directory);
            if let Ok(folder) =
                unsafe { SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(wide.as_ptr()), None) }
            {
                let _ = unsafe { dialog.SetFolder(&folder) };
            }
        }
        if let Some(name) = &options.file_name {
            let wide = HSTRING::from(name.as_str());
            unsafe { dialog.SetFileName(&wide)? };
        }
        if let Some(ext) = &options.default_extension {
            let wide = HSTRING::from(ext.as_str());
            unsafe { dialog.SetDefaultExtension(&wide)? };
        }

        // Keep wide buffers alive until `SetFileTypes` completes.
        let wide_types: Vec<(Vec<u16>, Vec<u16>)> = options
            .file_types
            .iter()
            .map(|(label, spec)| (to_wide_null(label), to_wide_null(spec)))
            .collect();
        if !wide_types.is_empty() {
            let specs: Vec<COMDLG_FILTERSPEC> = wide_types
                .iter()
                .map(|(label, spec)| COMDLG_FILTERSPEC {
                    pszName: PCWSTR(label.as_ptr()),
                    pszSpec: PCWSTR(spec.as_ptr()),
                })
                .collect();
            unsafe { dialog.SetFileTypes(&specs)? };
        }

        let customize: Option<IFileDialogCustomize> = if let Some(choice) = &options.choice {
            if let Ok(cust) = dialog.cast::<IFileDialogCustomize>() {
                let label = HSTRING::from(choice.label.as_str());
                let _ = unsafe { cust.StartVisualGroup(ENCODING_COMBO_ID + 100, &label) };
                let _ = unsafe { cust.AddComboBox(ENCODING_COMBO_ID) };
                for (ix, item) in choice.items.iter().enumerate() {
                    let text = HSTRING::from(item.as_str());
                    let _ = unsafe { cust.AddControlItem(ENCODING_COMBO_ID, ix as u32, &text) };
                }
                let _ = unsafe {
                    cust.SetSelectedControlItem(ENCODING_COMBO_ID, choice.selected as u32)
                };
                let _ = unsafe { cust.EndVisualGroup() };
                Some(cust)
            } else {
                None
            }
        } else {
            None
        };

        let parent = (!owner.0.is_null()).then_some(owner);
        if let Err(err) = unsafe { dialog.Show(parent) } {
            if is_cancelled(&err) {
                return Ok(None);
            }
            return Err(err.into());
        }

        let item = unsafe { dialog.GetResult()? };
        let path = unsafe { shell_item_path(&item)? };
        let selected = customize.and_then(|cust| {
            unsafe { cust.GetSelectedControlItem(ENCODING_COMBO_ID) }
                .ok()
                .map(|id| id as usize)
        });
        Ok(Some((path, selected)))
    })();
    unsafe { CoUninitialize() };
    result
}
