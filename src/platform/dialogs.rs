use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
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
        FileOpenDialog, FileSaveDialog, IFileDialog, IFileDialogCustomize, IFileOpenDialog,
        IFileSaveDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
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

unsafe fn configure_dialog(
    dialog: &IFileDialog,
    directory: Option<&Path>,
    file_types: &[(SharedStr, SharedStr)],
) -> Result<()> {
    if let Some(dir) = directory.filter(|d| !d.as_os_str().is_empty()) {
        let wide = HSTRING::from(dir.as_os_str());
        if let Ok(folder) =
            unsafe { SHCreateItemFromParsingName::<_, _, IShellItem>(&wide, None) }
        {
            let _ = unsafe { dialog.SetFolder(&folder) };
        }
    }
    if !file_types.is_empty() {
        let wide_types: Vec<(HSTRING, HSTRING)> = file_types
            .iter()
            .map(|(label, spec)| (HSTRING::from(label.as_str()), HSTRING::from(spec.as_str())))
            .collect();
        let specs: Vec<COMDLG_FILTERSPEC> = wide_types
            .iter()
            .map(|(label, spec)| COMDLG_FILTERSPEC {
                pszName: PCWSTR(label.as_ptr()),
                pszSpec: PCWSTR(spec.as_ptr()),
            })
            .collect();
        unsafe { dialog.SetFileTypes(&specs)? };
    }
    Ok(())
}

unsafe fn show_dialog(dialog: &IFileDialog, owner_raw: isize) -> Result<bool> {
    let owner = HWND(owner_raw as *mut _);
    let parent = (!owner.0.is_null()).then_some(owner);
    match unsafe { dialog.Show(parent) } {
        Ok(()) => Ok(true),
        Err(err) if is_cancelled(&err) => Ok(false),
        Err(err) => Err(err.into()),
    }
}

pub(crate) fn run_open_dialog(
    owner_raw: isize,
    options: OpenDialogOptions,
) -> Result<Option<Vec<PathBuf>>> {
    let _ole = OleGuard::init();
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        let mut flags = dialog.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST;
        if options.multiple {
            flags |= FOS_ALLOWMULTISELECT;
        }
        dialog.SetOptions(flags)?;
        configure_dialog(&dialog, options.directory.as_deref(), &options.file_types)?;

        if !show_dialog(&dialog, owner_raw)? {
            return Ok(None);
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
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
        let flags =
            dialog.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_OVERWRITEPROMPT;
        dialog.SetOptions(flags)?;

        if let Some(title) = &options.title {
            dialog.SetTitle(&HSTRING::from(title.as_str()))?;
        }
        configure_dialog(&dialog, Some(&options.directory), &options.file_types)?;
        if let Some(name) = &options.file_name {
            dialog.SetFileName(&HSTRING::from(name.as_str()))?;
        }
        if let Some(ext) = &options.default_extension {
            dialog.SetDefaultExtension(&HSTRING::from(ext.as_str()))?;
        }

        let customize: Option<IFileDialogCustomize> = options.choice.as_ref().and_then(|choice| {
            let cust = dialog.cast::<IFileDialogCustomize>().ok()?;
            let _ = cust.StartVisualGroup(ENCODING_COMBO_ID + 100, &HSTRING::from(choice.label.as_str()));
            let _ = cust.AddComboBox(ENCODING_COMBO_ID);
            for (ix, item) in choice.items.iter().enumerate() {
                let _ = cust.AddControlItem(ENCODING_COMBO_ID, ix as u32, &HSTRING::from(item.as_str()));
            }
            let _ = cust.SetSelectedControlItem(ENCODING_COMBO_ID, choice.selected as u32);
            let _ = cust.EndVisualGroup();
            Some(cust)
        });

        if !show_dialog(&dialog, owner_raw)? {
            return Ok(None);
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
