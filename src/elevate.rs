//! Getting elevated work done without a UAC prompt on every click.
//!
//! After the one-time setup there is a scheduled task registered with "highest
//! privileges". The GUI drops a job file and asks the task to run, so applying
//! a mode is instant and silent. If the task is missing, the app falls back to
//! a normal UAC prompt so nothing is ever blocked.

use std::process::Command;

use crate::jobs::{self, Job, JobResult};

pub const TASK_NAME: &str = "XMG-Optimizer-Worker";

/// Runs a `schtasks` subcommand silently. `true` when it succeeded.
fn run_task(args: &[&str]) -> bool {
    Command::new("schtasks")
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `true` when the elevated worker task is registered.
pub fn task_exists() -> bool {
    Command::new("schtasks")
        .args(["/query", "/tn", TASK_NAME])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Removes the worker task.
pub fn unregister_task() -> bool {
    Command::new("schtasks")
        .args(["/delete", "/tn", TASK_NAME, "/f"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Asks the registered task to run the worker. Returns `false` if there is no
/// task, in which case the caller should fall back to [`elevate_directly`].
pub fn trigger_worker() -> bool {
    run_task(&["/run", "/tn", TASK_NAME])
}

/// Registers the worker task. Must itself be running elevated.
pub fn register_worker_task() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy().into_owned();
    let action = format!("\"{exe}\" --worker");

    let ok = Command::new("schtasks")
        .args([
            "/create",
            "/tn",
            TASK_NAME,
            "/tr",
            &action,
            "/sc",
            "onlogon",
            "/rl",
            "highest",
            "/f",
        ])
        .output()
        .map(|o| {
            if !o.status.success() {
                Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
            } else {
                Ok(())
            }
        })
        .map_err(|e| e.to_string())?;

    ok
}

/// Launches the worker with a UAC prompt. Used for the first run and whenever
/// the scheduled task is unavailable.
pub fn elevate_directly() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe = exe.to_string_lossy().into_owned();
    let verb = crate::sysutil::wide("runas");
    let file = crate::sysutil::wide(&exe);
    let args = crate::sysutil::wide("--worker");

    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
        let r = ShellExecuteW(
            None,
            windows::core::PCWSTR(verb.as_ptr()),
            windows::core::PCWSTR(file.as_ptr()),
            windows::core::PCWSTR(args.as_ptr()),
            None,
            SW_HIDE,
        );
        // ShellExecuteW returns a handle; anything above 32 means success.
        (r as isize) > 32
    }
    #[cfg(not(windows))]
    {
        let _ = &exe;
        false
    }
}

/// Runs a job, preferring the silent scheduled task.
pub fn submit(job: Job) {
    jobs::clear_pending();
    if let Err(e) = jobs::write_job(&job) {
        jobs::write_result(&JobResult::fail(format!("Could not prepare the job: {e}")));
        return;
    }
    if !trigger_worker() {
        elevate_directly();
    }
}

/// One-time setup: create the worker task if it is missing.
///
/// Returns `Ok(true)` when everything is ready afterwards.
pub fn ensure_setup() -> Result<bool, String> {
    if task_exists() {
        return Ok(true);
    }
    // Creating the task needs elevation. Spawn ourselves with --setup-elevated.
    let Ok(exe) = std::env::current_exe() else {
        return Err("cannot locate the executable".into());
    };
    let exe = exe.to_string_lossy().into_owned();
    let verb = crate::sysutil::wide("runas");
    let file = crate::sysutil::wide(&exe);
    let args = crate::sysutil::wide("--setup-elevated");

    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
        let r = ShellExecuteW(
            None,
            windows::core::PCWSTR(verb.as_ptr()),
            windows::core::PCWSTR(file.as_ptr()),
            windows::core::PCWSTR(args.as_ptr()),
            None,
            SW_HIDE,
        );
        if (r as isize) > 32 {
            // Give the elevated process a moment to create the task.
            for _ in 0..20 {
                if task_exists() {
                    return Ok(true);
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            return Err("the elevated setup did not finish in time".into());
        }
        Err("administrator rights are needed once, to register the background task".into())
    }
    #[cfg(not(windows))]
    {
        let _ = (&exe, &verb, &file, &args);
        Err("unsupported platform".into())
    }
}