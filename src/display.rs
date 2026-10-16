//! Display refresh rate, driven through `ChangeDisplaySettingsEx`.
//!
//! Brightness deliberately does NOT live here: it is a `powercfg` setting and
//! flows through the normal plan/drift/backup machinery like the other
//! seventeen power values. Only the refresh rate needs raw Win32, because no
//! power setting controls it.
//!
//! Scope is the primary display only (`None` device). Multi-monitor setups
//! keep their secondary screens untouched, which is also what the safety story
//! requires: the app only ever rearranges what it can put back.
//!
//! Like the GPU limit, the refresh rate is covered by the *initial* backup
//! only (`original-display-hz.txt`), not by per-change snapshots. Snapshots
//! store `powercfg` rows and the restore parser only accepts those.

use std::sync::OnceLock;

use crate::errors::{AppError, AppResult};
use crate::profile::Mode;

/// Quiet refresh rate: 60 Hz. Chosen because every panel offers it, so the
/// write cannot fail for lack of support - and 60 Hz is the whole point of
/// Quiet on the display side.
pub const QUIET_HZ: u32 = 60;

#[derive(Debug, Clone)]
pub enum DisplayOutcome {
    Applied(u32),
    Unavailable(String),
    Failed(String),
}

/// Target refresh rate for a mode. `None` only when Performance cannot even
/// discover the maximum (no usable display enumeration at all).
pub fn hz_for_mode(mode: Mode) -> Option<u32> {
    match mode {
        Mode::Quiet => Some(QUIET_HZ),
        Mode::Performance => max_hz_cached(),
    }
}

/// Human label for the UI: Quiet always shows its fixed 60 Hz, Performance
/// shows the discovered maximum, or a bare "max" when discovery failed.
pub fn label_for_mode(mode: Mode) -> String {
    match mode {
        Mode::Quiet => format!("{QUIET_HZ} Hz"),
        Mode::Performance => match max_hz_cached() {
            Some(m) => format!("{m} Hz max"),
            None => "max".into(),
        },
    }
}

static MAX_HZ: OnceLock<Option<u32>> = OnceLock::new();

/// The panel's maximum refresh rate, discovered once. Display modes do not
/// change under a running session often enough to re-enumerate every time,
/// and each enumeration is a few dozen Win32 calls.
pub fn max_hz_cached() -> Option<u32> {
    *MAX_HZ.get_or_init(max_hz)
}

/// Applies the mode's refresh rate.
pub fn apply_for_mode(mode: Mode) -> DisplayOutcome {
    match hz_for_mode(mode) {
        None => DisplayOutcome::Unavailable("could not read the display modes".into()),
        Some(want) => {
            if !supported_rates().contains(&want) {
                return DisplayOutcome::Failed(format!(
                    "{want} Hz is not offered by this display - left unchanged"
                ));
            }
            match set_hz(want) {
                true => DisplayOutcome::Applied(want),
                false => DisplayOutcome::Failed("the display rejected the mode change".into()),
            }
        }
    }
}

/// Current refresh rate of the primary display, if Windows will tell us.
pub fn current_hz() -> Option<u32> {
    current_mode().map(|dm| dm.dmDisplayFrequency)
}

/// Every refresh rate the panel offers at its current resolution.
fn supported_rates() -> Vec<u32> {
    let Some(cur) = current_mode() else {
        return Vec::new();
    };
    let mut out: Vec<u32> = modes()
        .into_iter()
        .filter(|dm| dm.dmPelsWidth == cur.dmPelsWidth && dm.dmPelsHeight == cur.dmPelsHeight)
        .map(|dm| dm.dmDisplayFrequency)
        .filter(|hz| *hz >= 24 && *hz <= 480)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Highest refresh rate at the current resolution.
pub fn max_hz() -> Option<u32> {
    supported_rates().into_iter().max()
}

/// Sets the primary display to `hz`, persisting it so it survives reboots.
/// Normal users may change refresh rates without elevation, so this never
/// prompts - the worker only runs elevated because power settings need it.
pub fn set_hz(hz: u32) -> bool {
    use windows::Win32::Graphics::Gdi::*;
    unsafe {
        let mut dm = DEVMODEW::default();
        dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
        dm.dmDisplayFrequency = hz;
        dm.dmFields = DM_DISPLAYFREQUENCY;
        ChangeDisplaySettingsExW(
            windows::core::PCWSTR::null(),
            Some(&dm),
            None,
            CDS_UPDATEREGISTRY,
            None,
        ) == DISP_CHANGE_SUCCESSFUL
    }
}

fn current_mode() -> Option<windows::Win32::Graphics::Gdi::DEVMODEW> {
    use windows::Win32::Graphics::Gdi::*;
    unsafe {
        let mut dm = DEVMODEW::default();
        dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
        if EnumDisplaySettingsW(
            windows::core::PCWSTR::null(),
            ENUM_CURRENT_SETTINGS,
            &mut dm,
        )
        .0 == 0
        {
            return None;
        }
        (dm.dmDisplayFrequency > 0).then_some(dm)
    }
}

fn modes() -> Vec<windows::Win32::Graphics::Gdi::DEVMODEW> {
    use windows::Win32::Graphics::Gdi::*;
    let mut out = Vec::new();
    for i in 0..512u32 {
        unsafe {
            let mut dm = DEVMODEW::default();
            dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
            if EnumDisplaySettingsW(
                windows::core::PCWSTR::null(),
                ENUM_DISPLAY_SETTINGS_MODE(i),
                &mut dm,
            )
            .0 == 0
            {
                break;
            }
            out.push(dm);
        }
    }
    out
}

// ------------------------------------------------------------- backup glue ---

/// Stores the current rate so a restore can put it back. Mirrors the GPU
/// limit file: a bare number, validated on read.
pub fn capture(base: &std::path::Path) -> AppResult<()> {
    if let Some(hz) = current_hz() {
        std::fs::write(base.join("original-display-hz.txt"), format!("{hz}"))
            .map_err(|e| AppError::Io(e.to_string()))?;
    }
    Ok(())
}

/// Reads back the stored rate. Rejects anything that is not a plausible
/// refresh rate rather than trusting the file.
pub fn stored_hz(base: &std::path::Path) -> Option<u32> {
    let t = std::fs::read_to_string(base.join("original-display-hz.txt")).ok()?;
    let v: u32 = t.trim().parse().ok()?;
    (24..=480).contains(&v).then_some(v)
}
