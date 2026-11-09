//! Windows Open and Save dialogs that treat map folders (directories named
//! `*.SC2Map`, the official editor's component format) as single files.
//!
//! The stock dialog always browses into a folder when the user opens it,
//! whether by double-click or by selecting it and pressing Open/Save. Here
//! a dialog event catches the dialog about to enter a map folder, picks
//! that folder instead, and closes the dialog.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use windows::core::{implement, Error, Interface, Result, HRESULT, HSTRING, PCWSTR};
use windows::Win32::Foundation::{E_ABORT, ERROR_CANCELLED};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, FileSaveDialog, IFileDialog, IFileDialogEvents, IFileDialogEvents_Impl,
    IFileOpenDialog, IFileSaveDialog, IShellItem, SHCreateItemFromParsingName,
    FDE_OVERWRITE_RESPONSE, FDE_SHAREVIOLATION_RESPONSE, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM,
    FOS_PATHMUSTEXIST, SIGDN_FILESYSPATH,
};

/// A file type choice: its label and extensions without the dot.
pub type Filter<'a> = (&'a str, &'a [&'a str]);

/// Is `path` a map folder?
pub fn is_map_folder(path: &Path) -> bool {
    path.is_dir()
        && path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().ends_with(".sc2map"))
}

/// Show an Open dialog. `Ok(None)` means cancelled; `Err` means the dialog
/// couldn't be shown (the caller falls back to the portable one).
pub fn open(title: &str, filters: &[Filter]) -> Result<Option<PathBuf>> {
    with_com(|| unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        dialog.SetOptions(dialog.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_FILEMUSTEXIST)?;
        run(&dialog.cast()?, title, filters, None)
    })
}

/// Show a Save dialog suggesting `file_name` in `dir` (or wherever the
/// dialog was last used).
pub fn save(title: &str, filters: &[Filter], file_name: &str, dir: Option<&Path>) -> Result<Option<PathBuf>> {
    with_com(|| unsafe {
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
        dialog.SetOptions(dialog.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)?;
        dialog.SetFileName(&HSTRING::from(file_name))?;
        if let Some(ext) = filters.first().and_then(|f| f.1.first()) {
            dialog.SetDefaultExtension(&HSTRING::from(*ext))?;
        }
        run(&dialog.cast()?, title, filters, dir)
    })
}

fn with_com<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    // Each dialog runs on its own worker thread, which needs an STA.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    init.ok()?;
    let out = f();
    unsafe { CoUninitialize() };
    out
}

unsafe fn run(dialog: &IFileDialog, title: &str, filters: &[Filter], dir: Option<&Path>) -> Result<Option<PathBuf>> {
    dialog.SetTitle(&HSTRING::from(title))?;
    // The specs point into these strings, which must outlive the dialog.
    let strings: Vec<(HSTRING, HSTRING)> = filters
        .iter()
        .map(|(name, exts)| {
            let spec = exts.iter().map(|e| format!("*.{e}")).collect::<Vec<_>>().join(";");
            (HSTRING::from(*name), HSTRING::from(spec))
        })
        .collect();
    let specs: Vec<COMDLG_FILTERSPEC> = strings
        .iter()
        .map(|(n, s)| COMDLG_FILTERSPEC { pszName: PCWSTR(n.as_ptr()), pszSpec: PCWSTR(s.as_ptr()) })
        .collect();
    if !specs.is_empty() {
        dialog.SetFileTypes(&specs)?;
    }

    // Never start inside a map folder (Windows reopens the last folder
    // used); opening there would immediately pick it.
    let start = match dir {
        Some(d) => Some(d.to_path_buf()),
        None => dialog.GetFolder().ok().and_then(|i| path_of(&i)),
    };
    if let Some(start) = start {
        let outside = start.ancestors().filter(|a| is_map_folder(a)).last().and_then(Path::parent).map(Path::to_path_buf);
        if let Some(folder) = outside.or(dir.map(Path::to_path_buf)) {
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(folder.as_os_str()), None) {
                dialog.SetFolder(&item)?;
            }
        }
    }

    let picked = Arc::new(Mutex::new(None));
    let events: IFileDialogEvents = Events { picked: picked.clone(), armed: Mutex::new(false) }.into();
    let cookie = dialog.Advise(&events)?;
    let shown = dialog.Show(None);
    let _ = dialog.Unadvise(cookie);
    let picked = picked.lock().unwrap().take();
    if picked.is_some() {
        return Ok(picked);
    }
    match shown {
        Ok(()) => Ok(dialog.GetResult().ok().and_then(|i| path_of(&i))),
        Err(e) if e.code() == HRESULT::from_win32(ERROR_CANCELLED.0) => Ok(None),
        Err(e) => Err(e),
    }
}

/// A shell item's file-system path (None for virtual places like This PC).
fn path_of(item: &IShellItem) -> Option<PathBuf> {
    unsafe {
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = PathBuf::from(OsString::from_wide(p.as_wide()));
        CoTaskMemFree(Some(p.0 as *const _));
        Some(path)
    }
}

#[implement(IFileDialogEvents)]
struct Events {
    picked: Arc<Mutex<Option<PathBuf>>>,
    /// Set once the dialog has shown its first folder, so the initial
    /// navigation never counts as the user opening a map folder.
    armed: Mutex<bool>,
}

impl IFileDialogEvents_Impl for Events_Impl {
    fn OnFolderChanging(&self, pfd: Option<&IFileDialog>, folder: Option<&IShellItem>) -> Result<()> {
        if !*self.armed.lock().unwrap() {
            return Ok(());
        }
        let (Some(dialog), Some(path)) = (pfd, folder.and_then(path_of)) else { return Ok(()) };
        if !is_map_folder(&path) {
            return Ok(());
        }
        *self.picked.lock().unwrap() = Some(path);
        unsafe { dialog.Close(HRESULT::from_win32(ERROR_CANCELLED.0))? };
        // Refuse the navigation so the dialog doesn't flash the folder.
        Err(Error::from(E_ABORT))
    }

    fn OnFolderChange(&self, _: Option<&IFileDialog>) -> Result<()> {
        *self.armed.lock().unwrap() = true;
        Ok(())
    }

    fn OnFileOk(&self, _: Option<&IFileDialog>) -> Result<()> {
        Ok(())
    }

    fn OnSelectionChange(&self, _: Option<&IFileDialog>) -> Result<()> {
        Ok(())
    }

    fn OnShareViolation(&self, _: Option<&IFileDialog>, _: Option<&IShellItem>) -> Result<FDE_SHAREVIOLATION_RESPONSE> {
        Ok(FDE_SHAREVIOLATION_RESPONSE(0))
    }

    fn OnTypeChange(&self, _: Option<&IFileDialog>) -> Result<()> {
        Ok(())
    }

    fn OnOverwrite(&self, _: Option<&IFileDialog>, _: Option<&IShellItem>) -> Result<FDE_OVERWRITE_RESPONSE> {
        Ok(FDE_OVERWRITE_RESPONSE(0))
    }
}
