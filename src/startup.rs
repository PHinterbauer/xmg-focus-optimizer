//! Startup entries: disabling background apps and delaying the ones that must
//! keep running.
//!
//! LG HUB and OneDrive are never disabled - they are only started a little
//! later, so the laptop reaches the desktop faster without losing them.

use std::path::Path;

use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
use winreg::RegKey;

use crate::errors::{AppError, AppResult};

const RUN_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";

/// Entries we stop from launching at sign-in.
pub const DISABLED: &[(&str, &str)] = &[
    (
        "MicrosoftEdgeAutoLaunch_A925A434363F5425A4BC01C4E9D62075",
        "Edge auto-launch",
    ),
    ("RobloxPlayerBeta", "Roblox"),
    ("Docker Desktop", "Docker Desktop"),
];

/// Entries that keep working but start later.
pub const DELAYED: &[(&str, u32, &str)] = &[
    ("OneDrive", 20, "OneDrive"),
    ("LGHUB", 10, "LG HUB"),
];

fn open_run(write: bool) -> AppResult<RegKey> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let access = if write { KEY_SET_VALUE } else { KEY_READ };
    hkcu.open_subkey_with_flags(RUN_KEY, access)
        .map_err(|e| AppError::Io(format!("{RUN_KEY}: {e}")))
}

pub fn read_value(name: &str) -> Option<String> {
    let k = open_run(false).ok()?;
    k.get_value::<String, _>(name).ok()
}

/// Every startup entry as (name, command).
pub fn list_all() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(k) = open_run(false) else { return out };
    // `enum_values` yields (name, RegValue) pairs; we only keep REG_SZ ones.
    for entry in k.enum_values().flatten() {
        let (name, value) = entry;
        if let Ok(v) = k.get_value::<String, _>(name.as_str()) {
            out.push((name, v));
        } else {
            let _ = value;
        }
    }
    out.sort();
    out
}

/// Saves every current startup entry so a restore can put them back.
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

/// Restores startup entries from a captured file.
///
/// Only value names made of plain, expected characters are written; anything
/// unexpected is skipped rather than passed to the registry.
pub fn restore_autostart(base: &Path) -> (usize, Vec<String>) {
    let mut ok = 0;
    let mut skipped = Vec::new();
    let Ok(text) = std::fs::read_to_string(base.join("original-autostart.txt")) else {
        return (0, skipped);
    };
    let Ok(key) = open_run(true) else {
        return (0, vec!["Run key not writable".into()]);
    };
    for line in text.lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if !valid_value_name(name) {
            skipped.push(format!("{name} (name rejected)"));
            continue;
        }
        if value.trim().is_empty() {
            skipped.push(format!("{name} (empty value)"));
            continue;
        }
        match key.set_value(name, &value) {
            Ok(()) => ok += 1,
            Err(_) => skipped.push(name.to_string()),
        }
    }
    (ok, skipped)
}

fn valid_value_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name.chars().all(|c| {
            c.is_ascii_alphanumeric() || c.is_ascii_whitespace() || "._-()".contains(c)
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupAction {
    Disable,
    Delay(u32),
}

/// Applies the startup policy.
pub fn apply(action: StartupAction, name: &str, delay_sec: u32) -> Result<String, String> {
    let Some(current) = read_value(name) else {
        return Ok("was not in startup - nothing to do".into());
    };
    let key = open_run(true).map_err(|e| e.to_string())?;

    match action {
        StartupAction::Disable => match key.delete_value(name) {
            Ok(()) => Ok("disabled".into()),
            Err(e) => Err(e.to_string()),
        },
        StartupAction::Delay(_) => {
            let exe = parse_exe(&current).ok_or_else(|| {
                format!("cannot read a program path from '{current}' - left unchanged")
            })?;
            if !std::path::Path::new(&exe).exists() {
                return Err(format!("{exe} no longer exists - left unchanged"));
            }
            let batch = startup_dir()
                .ok_or_else(|| "Startup folder not found".to_string())?
                .join(format!("XMG-start-{name}.cmd"));
            let content = format!(
                "@echo off\r\nping -n {} 127.0.0.1 >nul\r\nstart \"\" \"{}\"\r\n",
                delay_sec + 1,
                exe
            );
            std::fs::write(&batch, content).map_err(|e| e.to_string())?;
            key.set_value(name, &format!("\"{}\"", batch.display()))
                .map_err(|e| e.to_string())?;
            Ok(format!("starts {delay_sec}s after sign-in"))
        }
    }
}

/// Extracts the program path from a startup command line.
fn parse_exe(cmd: &str) -> Option<String> {
    let t = cmd.trim();
    if t.is_empty() {
        return None;
    }
    if let Some(rest) = t.strip_prefix('"') {
        return rest.split('"').next().map(|s| s.to_string()).filter(|s| !s.is_empty());
    }
    // Unquoted: take up to the first .exe
    let lower = t.to_ascii_lowercase();
    let idx = lower.find(".exe")?;
    Some(t[..idx + 4].trim().to_string())
}

fn startup_dir() -> Option<std::path::PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        std::path::PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Startup"),
    )
}

// --------------------------------------------------------- own autostart ---

const SELF_RUN_VALUE: &str = "XMGOptimizer";

/// Registers the app to launch at sign-in (hidden, straight to the tray).
pub fn set_self_autostart(enabled: bool) -> Result<bool, String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)
        .map_err(|e| e.to_string())?;

    if !enabled {
        return match key.delete_value(SELF_RUN_VALUE) {
            Ok(()) => Ok(true),
            Err(_) => Ok(false), // already absent
        };
    }

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy().into_owned();
    // --tray keeps the window hidden; --autostart tells the app that this
    // particular launch came from Windows rather than from a click, which is
    // what triggers the silent re-apply of the saved mode.
    let cmd = format!("\"{exe}\" --tray --autostart");
    key.set_value(SELF_RUN_VALUE, &cmd).map_err(|e| e.to_string())?;
    Ok(true)
}

pub fn self_autostart_enabled() -> bool {
    open_run(false)
        .ok()
        .and_then(|k| k.get_value::<String, _>(SELF_RUN_VALUE).ok())
        .is_some()
}

/// Brings the registry in line with the stored preference.
///
/// Returns `true` when a change was made, so the caller can mention it.
pub fn reconcile_autostart(want: bool) -> bool {
    let have = self_autostart_enabled();
    if have == want {
        return false;
    }
    matches!(set_self_autostart(want), Ok(true))
}