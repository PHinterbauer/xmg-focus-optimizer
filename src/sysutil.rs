//! Small Windows helpers: elevation, power state, single-instance lock.

use std::sync::Mutex;

// ------------------------------------------------------------- elevation ---

/// `true` when this process holds an elevated (administrator) token.
pub fn is_elevated() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        use windows::Win32::Foundation::{CloseHandle, HANDLE};

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
    #[cfg(not(windows))]
    {
        false
    }
}

// ----------------------------------------------------------- power source ---

/// `true` on mains power, `false` on battery.
/// Falls back to "on AC" if Windows will not tell us, matching the prototype.
pub fn on_ac_power() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        let mut st = SYSTEM_POWER_STATUS::default();
        let ok = unsafe { GetSystemPowerStatus(&mut st) }.as_bool();
        if ok {
            // 1 = AC, 0 = battery, 255 = unknown
            return st.ACLineStatus != 0;
        }
    }
    true
}

/// Remaining battery percentage, when a battery is present.
pub fn battery_percent() -> Option<u8> {
    #[cfg(windows)]
    {
        use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        let mut st = SYSTEM_POWER_STATUS::default();
        if unsafe { GetSystemPowerStatus(&mut st) }.as_bool() && st.BatteryLifePercent != 255 {
            return Some(st.BatteryLifePercent);
        }
    }
    None
}

// -------------------------------------------------------- single instance ---

static INSTANCE: Mutex<Option<SingleInstance>> = Mutex::new(None);

pub struct SingleInstance {
    handle: windows::Win32::Foundation::HANDLE,
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

/// Tries to become the one running instance.
///
/// Returns `Ok(guard)` when we got the lock. An *abandoned* mutex (the previous
/// instance was killed) counts as acquired, because nobody else holds it -
/// refusing here would leave the app permanently unlaunchable.
#[cfg(windows)]
pub fn acquire_single_instance(name: &str) -> Result<SingleInstance, ()> {
    use windows::Win32::Foundation::{CompareStringOrdinal, HANDLE};
    use windows::Win32::System::Threading::{CreateMutexW, Mutex, WaitForSingleObject, INFINITE};

    let wide = crate::wide(name);
    unsafe {
        let handle: HANDLE = CreateMutexW(None, true, windows::core::PCWSTR(wide.as_ptr()));
        if handle.is_invalid() {
            // Could not even create it - fall through and let the app run.
            return Ok(SingleInstance { handle });
        }
        match WaitForSingleObject(handle, 0) {
            // WAIT_OBJECT_0 / WAIT_ABANDONED both mean the lock is ours now.
            0 => Ok(SingleInstance { handle }),
            _ => {
                let _ = CloseHandle(handle);
                Err(())
            }
        }
    }
    .map_err(|_| ())
}

/// Sends the "show yourself" signal to an already running instance.
#[cfg(windows)]
pub fn notify_existing_instance(name: &str) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::{CreateEventW, OpenEventW, SetEvent, EVENT_MODIFY_STATE, SYNCHRONIZE};

    let event_name = crate::wide(&format!("{name}-show-event"));
    unsafe {
        let h = OpenEventW(SYNCHRONIZE | EVENT_MODIFY_STATE, false, windows::core::PCWSTR(event_name.as_ptr()));
        if let Ok(h) = h {
            if !h.is_invalid() {
                let _ = SetEvent(h);
                let _ = windows::Win32::Foundation::CloseHandle(h);
            }
        }
        // Also make sure the event exists for future instances.
        let _ = CreateEventW(
            None,
            true,
            false,
            windows::core::PCWSTR(crate::wide(&format!("{name}-show")).as_ptr()),
        );
    }
}

/// Blocks until the "show" event is signalled, then clears it.
#[cfg(windows)]
pub fn wait_for_show_event(name: &str, timeout_ms: u32) {
    use windows::Win32::System::Threading::{
        CreateEventW, OpenEventW, ResetEvent, WaitForSingleObject, EVENT_MODIFY_STATE, SYNCHRONIZE,
    };

    let name = format!("{name}-show");
    unsafe {
        let h = OpenEventW(SYNCHRONIZE | EVENT_MODIFY_STATE, false, windows::core::PCWSTR(crate::wide(&name).as_ptr()));
        if let Ok(h) = h {
            if !h.is_invalid() {
                let _ = WaitForSingleObject(h, timeout_ms);
                let _ = ResetEvent(h);
                let _ = windows::Win32::Foundation::CloseHandle(h);
            }
        }
        // Ensure the event exists for the next launch.
        let _ = CreateEventW(None, true, false, windows::core::PCWSTR(crate::wide(&name).as_ptr()));
    }
}

/// Converts a Rust string to a NUL-terminated UTF-16 buffer.
#[cfg(windows)]
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}