use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use windows::Win32::{
    Foundation::{ERROR_CANCELLED, HWND},
    System::{
        Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree},
        Ole::{OleInitialize, OleUninitialize},
    },
    UI::Shell::{
        Common::COMDLG_FILTERSPEC,
        FOS_ALLOWMULTISELECT, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST,
        FileOpenDialog, FileSaveDialog, IFileDialogCustomize, IFileOpenDialog, IFileSaveDialog,
        IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    },
};
use windows::core::{HSTRING, Interface, PCWSTR};

use crate::types::{Result, SharedStr};

const ENCODING_COMBO_ID: u32 = 1001;

#[derive(Clone, Debug, Default)]
pub(crate) struct OpenDialogOptions {
    pub(crate) multiple: bool,
    pub(crate) directory: Option<PathBuf>,
    pub(crate) file_types: Vec<(SharedStr, SharedStr)>,
}

#[derive(Clone, Debug)]
pub(crate) struct SaveChoice {
    pub(crate) label: SharedStr,
    pub(crate) items: Vec<SharedStr>,
    pub(crate) selected: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct SaveDialogOptions {
    pub(crate) title: Option<SharedStr>,
    pub(crate) directory: PathBuf,
    pub(crate) file_name: Option<String>,
    pub(crate) file_types: Vec<(SharedStr, SharedStr)>,
    pub(crate) default_extension: Option<SharedStr>,
    pub(crate) choice: Option<SaveChoice>,
}

pub(crate) type SaveDialogResult = Result<Option<(PathBuf, Option<usize>)>>;

struct OleGuard(bool);

impl OleGuard {
    fn init() -> Self {
        Self(unsafe { OleInitialize(None) }.is_ok())
    }
}

impl Drop for OleGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { OleUninitialize() };
        }
    }
}

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

pub(crate) fn run_open_dialog(
    owner_raw: isize,
    options: OpenDialogOptions,
) -> Result<Option<Vec<PathBuf>>> {
    let _ole = OleGuard::init();
    let owner = HWND(owner_raw as *mut _);
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        let mut flags = dialog.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST;
        if options.multiple {
            flags |= FOS_ALLOWMULTISELECT;
        }
        dialog.SetOptions(flags)?;

        if let Some(dir) = &options.directory
            && !dir.as_os_str().is_empty()
        {
            let wide = path_to_wide_null(dir);
            if let Ok(folder) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(wide.as_ptr()), None)
            {
                let _ = dialog.SetFolder(&folder);
            }
        }

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
            dialog.SetFileTypes(&specs)?;
        }

        let parent = (!owner.0.is_null()).then_some(owner);
        if let Err(err) = dialog.Show(parent) {
            if is_cancelled(&err) {
                return Ok(None);
            }
            return Err(err.into());
        }
        let items = dialog.GetResults()?;
        let count = items.GetCount()?;
        let mut paths = Vec::with_capacity(count as usize);
        for ix in 0..count {
            let item = items.GetItemAt(ix)?;
            paths.push(shell_item_path(&item)?);
        }
        Ok(Some(paths))
    }
}

pub(crate) fn run_save_dialog(owner_raw: isize, options: SaveDialogOptions) -> SaveDialogResult {
    let _ole = OleGuard::init();
    let owner = HWND(owner_raw as *mut _);
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
        let flags =
            dialog.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_OVERWRITEPROMPT;
        dialog.SetOptions(flags)?;

        if let Some(title) = &options.title {
            let wide = HSTRING::from(title.as_str());
            dialog.SetTitle(&wide)?;
        }
        if !options.directory.as_os_str().is_empty() {
            let wide = path_to_wide_null(&options.directory);
            if let Ok(folder) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(wide.as_ptr()), None)
            {
                let _ = dialog.SetFolder(&folder);
            }
        }
        if let Some(name) = &options.file_name {
            let wide = HSTRING::from(name.as_str());
            dialog.SetFileName(&wide)?;
        }
        if let Some(ext) = &options.default_extension {
            let wide = HSTRING::from(ext.as_str());
            dialog.SetDefaultExtension(&wide)?;
        }

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
            dialog.SetFileTypes(&specs)?;
        }

        let customize: Option<IFileDialogCustomize> = if let Some(choice) = &options.choice {
            if let Ok(cust) = dialog.cast::<IFileDialogCustomize>() {
                let label = HSTRING::from(choice.label.as_str());
                let _ = cust.StartVisualGroup(ENCODING_COMBO_ID + 100, &label);
                let _ = cust.AddComboBox(ENCODING_COMBO_ID);
                for (ix, item) in choice.items.iter().enumerate() {
                    let text = HSTRING::from(item.as_str());
                    let _ = cust.AddControlItem(ENCODING_COMBO_ID, ix as u32, &text);
                }
                let _ = cust.SetSelectedControlItem(ENCODING_COMBO_ID, choice.selected as u32);
                let _ = cust.EndVisualGroup();
                Some(cust)
            } else {
                None
            }
        } else {
            None
        };

        let parent = (!owner.0.is_null()).then_some(owner);
        if let Err(err) = dialog.Show(parent) {
            if is_cancelled(&err) {
                return Ok(None);
            }
            return Err(err.into());
        }

        let item = dialog.GetResult()?;
        let path = shell_item_path(&item)?;
        let selected = customize.and_then(|cust| {
            cust.GetSelectedControlItem(ENCODING_COMBO_ID)
                .ok()
                .map(|id| id as usize)
        });
        Ok(Some((path, selected)))
    }
}
