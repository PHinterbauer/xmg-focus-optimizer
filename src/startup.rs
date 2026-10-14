//! Reading the list of startup entries.
//!
//! # Only this app's own entry is ever written
//!
//! Applying a mode used to delete background apps from the Run key and to
//! rewrite the ones that must keep working so they launched later through a
//! generated `.cmd` wrapper. That was wrong: a mode is a request about power,
//! fans and the GPU, and it silently rearranged the user's sign-in as a side
//! effect. No mode touches startup any more.
//!
//! What the app may still do is add *itself* to the Run key, so it can come back
//! at sign-in - the user asked for that explicitly, and it is its own entry.
//!
//! The two are kept apart on purpose. [`list_all`] and [`capture_autostart`]
//! open the key read-only. The write path is [`open_run_self`], and every
//! function on this side of the line writes the constant [`SELF_RUN_VALUE`] and
//! nothing else - none of them takes a name to write. There is no way to reach
//! another program's entry from this module. What launches at sign-in stays
//! exactly as the user arranged it in Task Manager.

use std::path::Path;

use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
use winreg::RegKey;

use crate::errors::{AppError, AppResult};

const RUN_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";

fn open_run() -> AppResult<RegKey> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey_with_flags(RUN_KEY, KEY_READ)
        .map_err(|e| AppError::Io(format!("{RUN_KEY}: {e}")))
}

/// Every startup entry as (name, command).
pub fn list_all() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(k) = open_run() else { return out };
    // `enum_values` yields (name, RegValue) pairs; we only keep REG_SZ ones.
    for (name, _) in k.enum_values().flatten() {
        if let Ok(v) = k.get_value::<String, _>(name.as_str()) {
            out.push((name, v));
        }
    }
    out.sort();
    out
}

/// Records every current startup entry so there is a written copy of the
/// sign-in state as it was. This only writes to the backup folder; the Run key
/// is opened read-only.
pub fn capture_autostart(base: &Path) -> AppResult<()> {
    let entries = list_all();
    let body: String = entries
        .iter()
        .map(|(n, v)| format!("{n}={v}"))
        .collect::<Vec<_>>()
        .join("\r\n");
    if !body.is_empty() {
        std::fs::write(base.join("original-autostart.txt"), body)
            .map_err(|e| AppError::Io(format!("original-autostart.txt: {e}")))?;
    }
    Ok(())
}

// ------------------------------------------------------------ own autostart ---
//
// Everything below this line writes exactly one registry value - the app's
// own - and nothing else. Note that none of these functions accepts a value
// name: that is deliberate, because a name parameter is how a "startup
// optimisation" feature grows back. Another program's entry is out of reach
// from here by construction.

/// The only Run value this app writes.
const SELF_RUN_VALUE: &str = "XMGOptimizer";

/// Write access to the Run key, for this app's own value only.
fn open_run_self() -> Result<RegKey, String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)
        .map_err(|e| format!("{RUN_KEY}: {e}"))
}

/// Adds or removes the app's own entry. `true` when the key now matches.
pub fn set_self_autostart(enabled: bool) -> Result<bool, String> {
    let key = open_run_self()?;

    if !enabled {
        return Ok(key.delete_value(SELF_RUN_VALUE).is_ok());
    }

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // --tray keeps the window hidden; --autostart asks for a silent re-apply of
    // the saved mode rather than showing a window at every sign-in.
    let cmd = format!("\"{}\" --tray --autostart", exe.to_string_lossy());
    key.set_value(SELF_RUN_VALUE, &cmd)
        .map_err(|e| e.to_string())?;
    Ok(true)
}

pub fn self_autostart_enabled() -> bool {
    open_run()
        .ok()
        .and_then(|k| k.get_value::<String, _>(SELF_RUN_VALUE).ok())
        .is_some()
}

/// Brings the app's own entry in line with the stored preference.
///
/// Returns `true` when something was changed, so the caller can mention it.
/// This reconciles *only* the app's own entry; no other entry is read or
/// written here.
pub fn reconcile_autostart(want: bool) -> bool {
    if self_autostart_enabled() == want {
        return false;
    }
    matches!(set_self_autostart(want), Ok(true))
}