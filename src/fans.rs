//! Fan control through the vendor's own "Fan Speed Setting" app.
//!
//! # Why UI Automation
//!
//! The XMG FOCUS exposes fan curves through its embedded controller. Reaching
//! them directly would mean writing to undocumented EC/firmware structures -
//! `SetDCHU_Data` in `InsydeDCHU.dll`, for example, takes parameters whose
//! meaning is not published and a wrong call can leave the laptop unable to
//! boot. So the app drives the *same UI the user would click*, which is a
//! supported, reversible path.
//!
//! If the vendor ever renames a control, this module reports "not available"
//! instead of guessing.

use windows::core::BSTR;
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationRangeValuePattern, IUIAutomationSelectionItemPattern,
    IUIAutomationTogglePattern, ToggleState, TreeScope, UIA_InvokePatternId,
    UIA_RangeValuePatternId, UIA_SelectionItemPatternId, UIA_TogglePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsWindow, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

use crate::profile::{FanMode, Params};

/// Process name of the vendor app.
const APP_WINDOW_CLASS: &str = "FanSpeedSetting";
const APP_TITLE_HINT: &str = "Fan Speed";

#[derive(Debug, Clone)]
pub enum FanOutcome {
    /// Everything requested was applied and saved.
    Applied(String),
    /// The app or the controls are not present (vendor app not installed).
    Unavailable(String),
    /// The app is there but did not accept the change.
    Failed(String),
}

impl FanOutcome {
    pub fn message(&self) -> &str {
        match self {
            FanOutcome::Applied(s) | FanOutcome::Unavailable(s) | FanOutcome::Failed(s) => s,
        }
    }
    pub fn is_ok(&self) -> bool {
        matches!(self, FanOutcome::Applied(_))
    }
}

/// Applies the fan profile from the mode parameters.
pub fn apply(params: &Params) -> FanOutcome {
    let Some(hwnd) = find_app_window() else {
        return FanOutcome::Unavailable(
            "the XMG fan app was not found - fans left untouched".into(),
        );
    };

    // COM has to be initialised on this thread before any automation call.
    let com_ok = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let com_owned = com_ok.is_ok();
    if !com_owned {
        // RPC_E_CHANGED_MODE: COM is already up in another mode. Automation
        // still works for our purposes, so continue rather than failing.
    }

    let result = apply_inner(hwnd, params);

    if com_owned {
        unsafe { CoUninitialize() };
    }
    result
}

fn apply_inner(hwnd: HWND, params: &Params) -> FanOutcome {
    let ui: IUIAutomation = match unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) } {
        Ok(u) => u,
        Err(e) => return FanOutcome::Failed(format!("UI Automation unavailable: {e}")),
    };

    let root: IUIAutomationElement = match unsafe { ui.ElementFromHandle(hwnd) } {
        Ok(r) => r,
        Err(e) => return FanOutcome::Failed(format!("could not attach to the fan window: {e}")),
    };

    // The Save button only becomes enabled once the app is foreground.
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
    }
    std::thread::sleep(std::time::Duration::from_millis(400));

    let mut notes: Vec<String> = Vec::new();
    let mut changed = false;

    // 1) mode radio button -----------------------------------------------
    let wanted = params.fan_mode.control_id();
    match find_by_id(&root, wanted) {
        None => notes.push(format!("control '{wanted}' not found (vendor app version?)")),
        Some(el) => {
            let p: IUIAutomationSelectionItemPattern =
                match unsafe { el.GetCurrentPatternAs(UIA_SelectionItemPatternId) } {
                    Ok(p) => p,
                    Err(_) => {
                        notes.push("fan mode control is not selectable".into());
                        return FanOutcome::Failed(notes.join("; "));
                    }
                };
            let already = unsafe { p.CurrentIsSelected() }
                .map(|b| b.as_bool())
                .unwrap_or(false);
            if already {
                notes.push(format!("fan mode already {}", params.fan_mode.label()));
            } else if unsafe { p.Select() }.is_ok() {
                changed = true;
                notes.push(format!("fan mode -> {}", params.fan_mode.label()));
            } else {
                notes.push("fan mode could not be set".into());
            }
        }
    }

    // 2) noiseless toggle --------------------------------------------------
    match find_by_id(&root, "cb_noiseless") {
        None => notes.push("noiseless control not found".into()),
        Some(el) => {
            if let Ok(p) = unsafe { el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId) }
            {
                let is_on = unsafe { p.CurrentToggleState() }
                    .map(|s| s == ToggleState(1))
                    .unwrap_or(false);
                if is_on != params.noiseless && unsafe { p.Toggle() }.is_ok() {
                    changed = true;
                }
                notes.push(format!("noiseless {}", if params.noiseless { "on" } else { "off" }));
            }
        }
    }

    // 3) offset slider ----------------------------------------------------
    if let Some(el) = find_by_id(&root, "slider_FAN_AutoOffset") {
        if let Ok(p) =
            unsafe { el.GetCurrentPatternAs::<IUIAutomationRangeValuePattern>(UIA_RangeValuePatternId) }
        {
            let cur = unsafe { p.CurrentValue() }.unwrap_or(-1.0);
            if (cur - params.fan_offset as f64).abs() > 0.5 {
                if unsafe { p.SetValue(params.fan_offset as f64) }.is_ok() {
                    changed = true;
                    notes.push(format!("offset {} (was {})", params.fan_offset, cur as i64));
                } else {
                    notes.push("offset could not be set".into());
                }
            } else {
                notes.push(format!("offset {cur} unchanged"));
            }
        }
    }

    std::thread::sleep(std::time::Duration::from_millis(300));

    // 4) persist -----------------------------------------------------------
    if changed {
        if let Some(btn) = find_by_id(&root, "BTN_SAVE_Fan") {
            let enabled = unsafe { btn.CurrentIsEnabled() }.map(|b| b.as_bool()).unwrap_or(false);
            if enabled {
                if let Ok(p) = unsafe { btn.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) }
                {
                    if unsafe { p.Invoke() }.is_ok() {
                        notes.push("saved".into());
                    } else {
                        return FanOutcome::Failed(format!(
                            "save failed - {}",
                            notes.join("; ")
                        ));
                    }
                }
            }
        } else {
            notes.push("save button not found - change may not persist".into());
        }
    } else {
        notes.push("already set".into());
    }

    // 5) read back the actual RPM -----------------------------------------
    std::thread::sleep(std::time::Duration::from_millis(800));
    if let Some(rpm) = read_text(&root, "T_CPUFan_RPM_value") {
        notes.push(format!("CPU fan {rpm} RPM"));
    }

    let msg = notes.join("; ");
    if notes.iter().any(|n| n.contains("not found") || n.contains("could not") || n.contains("failed")) {
        FanOutcome::Failed(msg)
    } else {
        FanOutcome::Applied(msg)
    }
}

/// Live fan readings, for the status panel.
pub fn read_rpm() -> Option<(String, String)> {
    let hwnd = find_app_window()?;
    unsafe {
        let _ = ShowWindow(hwnd, SW_RESTORE);
    }
    let com_ok = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let ui: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }.ok()?;
    let root = unsafe { ui.ElementFromHandle(hwnd) }.ok()?;
    let cpu = read_text(&root, "T_CPUFan_RPM_value");
    let gpu = read_text(&root, "T_GPUFan_RPM_value");
    if com_ok {
        unsafe { CoUninitialize() };
    }
    match (cpu, gpu) {
        (Some(c), Some(g)) => Some((c, g)),
        (Some(c), None) => Some((c, "-".into())),
        _ => None,
    }
}

// ------------------------------------------------------------------ helpers --

/// Locates the vendor app's main window.
fn find_app_window() -> Option<HWND> {
    // Prefer an exact top-level window match.
    unsafe {
        let class_w: Vec<u16> = crate::sysutil::wide(APP_WINDOW_CLASS);
        if let Some(h) = FindWindowW(windows::core::PCWSTR(class_w.as_ptr()), None) {
            if IsWindow(h).as_bool() {
                return Some(h);
            }
        }
    }
    None
}

/// Depth-first search of the element tree for a control id.
fn find_by_id(root: &IUIAutomationElement, id: &str) -> Option<IUIAutomationElement> {
    let ui: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }.ok()?;
    let cond = unsafe { ui.CreateTrueCondition() }.ok()?;
    let all = unsafe { root.FindAll(TreeScope(0x2 | 0x4), &cond) }.ok()?; // Descendants|Subtree
    let n = unsafe { all.Length() }.unwrap_or(0);

    for i in 0..n {
        let Ok(el) = (unsafe { all.GetElement(i) }) else {
            continue;
        };
        if let Ok(b) = unsafe { el.CurrentAutomationId() } {
            if b.to_string() == id {
                return Some(el);
            }
        }
    }
    None
}

/// Reads a control's Name property (used for the RPM readouts).
fn read_text(root: &IUIAutomationElement, id: &str) -> Option<String> {
    let el = find_by_id(root, id)?;
    unsafe { el.CurrentName() }.ok().map(|b: BSTR| b.to_string())
}

/// Maps the mode's fan setting for display in the UI.
pub fn describe(mode: FanMode, offset: u32, noiseless: bool) -> String {
    format!(
        "{} mode, offset {}, noiseless {}",
        mode.label(),
        offset,
        if noiseless { "on" } else { "off" }
    )
}