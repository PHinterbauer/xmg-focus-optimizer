//! Small Windows helpers: elevation, power state, single-instance lock.

// ------------------------------------------------------------- elevation ---

/// `true` when this process holds an elevated (administrator) token.
pub fn is_elevated() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::Security::{
            GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
        };
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

        unsafe {
            let mut token = windows::Win32::Foundation::HANDLE::default();
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
        if unsafe { GetSystemPowerStatus(&mut st) }.as_bool() {
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

#[cfg(windows)]
pub struct SingleInstance {
    handle: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl Drop for SingleInstance {
    fn drop(&mut self) {
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
pub fn acquire_single_instance(_name: &str) -> Result<SingleInstance, ()> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};

    let wide = wide(_name);
    unsafe {
        let handle: HANDLE = CreateMutexW(None, true, windows::core::PCWSTR(wide.as_ptr()));
        if handle.is_invalid() {
            // Could not even create it - let the app run rather than refuse.
            return Ok(SingleInstance { handle });
        }
        match WaitForSingleObject(handle, 0) {
            // 0 = WAIT_OBJECT_0, 0x80 = WAIT_ABANDONED. Both mean we own it now.
            0 | 0x80 => Ok(SingleInstance { handle }),
            _ => {
                let _ = CloseHandle(handle);
                Err(())
            }
        }
    }
}

#[cfg(not(windows))]
pub struct SingleInstance;

#[cfg(not(windows))]
impl Drop for SingleInstance {
    fn drop(&mut self) {}
}

#[cfg(not(windows))]
pub fn acquire_single_instance(_name: &str) -> Result<SingleInstance, ()> {
    Ok(SingleInstance)
}

/// Signals an already running instance to bring its window forward.
#[cfg(windows)]
pub fn notify_existing_instance(name: &str) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::{
        OpenEventW, SetEvent, EVENT_MODIFY_STATE, SYNCHRONIZE,
    };

    let event_name = wide(&format!("{name}-show"));
    unsafe {
        match OpenEventW(
            SYNCHRONIZE | EVENT_MODIFY_STATE,
            false,
            windows::core::PCWSTR(event_name.as_ptr()),
        ) {
            Ok(h) if !h.is_invalid() => {
                let _ = SetEvent(h);
                let _ = windows::Win32::Foundation::CloseHandle(HANDLE(h.0));
            }
            _ => {}
        }
    }
}

#[cfg(not(windows))]
pub fn notify_existing_instance(_name: &str) {}

/// Converts a Rust string to a NUL-terminated UTF-16 buffer.
#[cfg(windows)]
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}