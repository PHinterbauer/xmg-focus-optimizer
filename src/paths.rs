//! Where everything lives on disk.
//!
//! The most important rule in this whole program: **a backup directory is never
//! deleted, emptied or overwritten in place.** New "initial" backups are always
//! written as a *new* archived copy, so the previous one keeps existing.

use std::path::{Path, PathBuf};

use crate::errors::{AppError, AppResult};

/// Name of the file that marks a directory as a real backup set.
/// The legacy PowerShell tool used exactly this name, so its factory backup is
/// found and reused instead of being replaced.
pub const FACTORY_MARKER: &str = "original-settings.txt";

/// Per-user application directory (config, logs, job files). Kept deliberately
/// separate from the backup directory so that cleaning app state can never
/// endanger a backup.
pub fn app_data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("XMG-Optimizer")
}

pub fn config_path() -> PathBuf {
    app_data_dir().join("config.json")
}

pub fn log_path() -> PathBuf {
    app_data_dir().join("xmg-optimizer.log")
}

pub fn job_path() -> PathBuf {
    app_data_dir().join("job.json")
}

pub fn job_result_path() -> PathBuf {
    app_data_dir().join("job-result.json")
}

pub fn tray_config_path() -> PathBuf {
    app_data_dir().join("tray.json")
}

/// Candidate backup directories, most preferred first.
///
/// 1. A portable/portable-installed copy next to the executable.
/// 2. The unpack location used by the legacy single-file launcher.
/// 3. The original development checkout (`...\XMG-Focus-Optimization\extra`).
/// 4. A documented default next to the user's Documents folder.
///
/// The first candidate that already contains a factory backup wins, so an
/// upgrade never silently starts from a blank slate.
pub fn backup_dir_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.join("XMG-Backup"));
        }
    }

    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        v.push(PathBuf::from(local).join("XMGTool").join("XMG-Backup"));
    }

    if let Some(docs) = known_folder_documents() {
        v.push(docs.join("XMG-Focus-Optimization").join("extra").join("XMG-Backup"));
        v.push(docs.join("XMG-Focus-Optimization").join("XMG-Backup"));
        v.push(docs.join("XMG-Optimizer").join("XMG-Backup"));
    }

    v
}

/// Resolves the active backup directory, creating it if necessary.
///
/// Resolution is stable once a factory backup exists: the same directory keeps
/// being returned for the lifetime of the installation.
pub fn backup_dir() -> AppResult<PathBuf> {
    let candidates = backup_dir_candidates();

    // Prefer a candidate that already holds the legacy factory backup.
    for c in &candidates {
        if c.join(FACTORY_MARKER).is_file() {
            return ensure_backup_layout(c);
        }
    }

    // Otherwise create the layout next to the executable so a portable install
    // stays self-contained.
    let target = candidates
        .first()
        .cloned()
        .unwrap_or_else(|| app_data_dir().join("XMG-Backup"));
    ensure_backup_layout(&target)
}

fn ensure_backup_layout(dir: &Path) -> AppResult<PathBuf> {
    std::fs::create_dir_all(dir)
        .map_err(|e| AppError::Io(format!("backup dir {}: {e}", dir.display())))?;
    std::fs::create_dir_all(dir.join("snapshots")).map_err(|e| {
        AppError::Io(format!("snapshots dir {}: {e}", dir.join("snapshots").display()))
    })?;
    std::fs::create_dir_all(dir.join("initial-archive")).map_err(|e| {
        AppError::Io(format!("archive dir {}: {e}", dir.join("initial-archive").display()))
    })?;
    Ok(dir.to_path_buf())
}

/// `Documents` via the shell API, falling back to the USERPROFILE convention.
fn known_folder_documents() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use windows::core::PWSTR;
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::SHGetKnownFolderPath;

        // FOLDERID_Documents = {FDD39AD0-238F-46AF-ADB4-6C85480369C7}
        unsafe {
            let mut ptr: *mut PWSTR = std::ptr::null_mut();
            let hr = SHGetKnownFolderPath(
                windows::core::GUID::from_u128(0xfdd39ad0_238f_46af_adb4_6c85480369c7),
                Default::default(),
                windows::Win32::Foundation::HANDLE(0),
            );
            if hr.is_ok() && !ptr.is_null() {
                let text = windows::core::PCWSTR(ptr.0).to_string().unwrap_or_default();
                let _ = CoTaskMemFree(Some(ptr.cast()));
                if !text.is_empty() {
                    return Some(PathBuf::from(text));
                }
            }
        }
    }
    fallback_documents()
}

fn fallback_documents() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().eq_ignore_ascii_case("Documents"))
                .unwrap_or(false)
        })
}