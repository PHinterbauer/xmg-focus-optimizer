//! Small Windows helpers: elevation, power state, single-instance lock.
//!
//! Note the convention throughout: in `windows` 0.62 most Win32 entry points
//! return `Result<T>`, and several constants are newtypes whose payload lives
//! in `.0`.

use windows::Win32::Foundation::{CloseHandle, HANDLE};

// ------------------------------------------------------------- elevation ---

/// `true` when this process holds an elevated (administrator) token.
pub fn is_elevated() -> bool {
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut u8),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

// ----------------------------------------------------------- power source ---

/// `true` on mains power, `false` on battery.
/// Falls back to "on AC" if Windows will not tell us, matching the prototype.
pub fn on_ac_power() -> bool {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut st = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut st) }.is_ok() {
        // 1 = AC, 0 = battery, 255 = unknown
        return st.ACLineStatus != 0;
    }
    true
}

/// Remaining battery percentage, when a battery is present.
pub fn battery_percent() -> Option<u8> {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut st = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut st) }.is_ok() && st.BatteryLifePercent != 255 {
        return Some(st.BatteryLifePercent);
    }
    None
}

// -------------------------------------------------------- single instance ---

pub struct SingleInstance {
    handle: HANDLE,
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Tries to become the one running instance.
///
/// Returns `Ok(guard)` when we got the lock. An *abandoned* mutex (the previous
/// instance was killed) counts as acquired, because nobody else holds it -
/// refusing here would leave the app permanently unlaunchable.
pub fn acquire_single_instance(name: &str) -> Result<SingleInstance, ()> {
    use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};

    let wide = wide(name);
    unsafe {
        let Ok(handle) = CreateMutexW(None, true, windows::core::PCWSTR(wide.as_ptr())) else {
            // Could not even create it - let the app run rather than refuse.
            return Ok(SingleInstance {
                handle: HANDLE::default(),
            });
        };
        if handle.is_invalid() {
            return Ok(SingleInstance { handle });
        }
        // WAIT_OBJECT_0 = 0, WAIT_ABANDONED = 0x80. Both mean we own it now.
        match WaitForSingleObject(handle, 0).0 {
            0 | 0x80 => Ok(SingleInstance { handle }),
            _ => {
                let _ = CloseHandle(handle);
                Err(())
            }
        }
    }
}

/// Signals an already running instance to bring its window forward.
pub fn notify_existing_instance(name: &str) {
    use windows::Win32::Security::SYNCHRONIZE;
    use windows::Win32::System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE};

    let event_name = wide(&format!("{name}-show"));
    unsafe {
        if let Ok(h) = OpenEventW(
            SYNCHRONIZE | EVENT_MODIFY_STATE,
            false,
            windows::core::PCWSTR(event_name.as_ptr()),
        ) {
            let _ = SetEvent(h);
            let _ = CloseHandle(h);
        }
    }
}

/// Converts a Rust string to a NUL-terminated UTF-16 buffer.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}