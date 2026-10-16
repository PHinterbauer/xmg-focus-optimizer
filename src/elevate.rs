//! Getting elevated work done without a UAC prompt on every click.
//!
//! After the one-time setup there is a scheduled task registered with "highest
//! privileges". The GUI drops a job file and asks the task to run, so applying
//! a mode is instant and silent. If the task is missing, the app falls back to
//! a normal UAC prompt so nothing is ever blocked.

use crate::jobs::{self, Job, JobResult};
use crate::log::{log_error, log_info, log_warn};

pub const TASK_NAME: &str = "XMG-Optimizer-Worker";

/// Runs a `schtasks` subcommand silently. `true` when it succeeded.
///
/// Never shows a console window; see [`crate::sysutil::silent_command`].
fn run_task(args: &[&str]) -> bool {
    let out = match crate::sysutil::silent_command("schtasks").args(args).output() {
        Ok(o) => o,
        Err(e) => {
            log_error!("schtasks could not be started: {e}");
            return false;
        }
    };
    crate::log::command("schtasks", args, &out);
    out.status.success()
}

/// `true` when the elevated worker task is registered.
pub fn task_exists() -> bool {
    let args = ["/query", "/tn", TASK_NAME];
    let ok = crate::sysutil::silent_command("schtasks")
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    log_info!("scheduled task registered: {ok}");
    ok
}

/// `true` when the task exists, points at this exact executable, and is
/// allowed to run on battery.
///
/// Both halves matter. `schtasks /create` cannot express the battery policy,
/// so every task registered the old way carries Microsoft's defaults
/// (`DisallowStartIfOnBatteries` + `StopIfGoingOnBatteries`): on battery the
/// task refuses to start, and a running worker is killed mid-apply on
/// unplug. A stale path is the other classic: each release is downloaded
/// over the previous file, and a task registered by an older copy keeps
/// pointing at wherever that copy lived.
pub fn task_is_healthy() -> bool {
    let exe = match std::env::current_exe() {
        Ok(e) => e.to_string_lossy().into_owned(),
        Err(_) => return false,
    };
    let out = match crate::sysutil::silent_command("schtasks")
        .args(["/query", "/tn", TASK_NAME, "/xml"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return false,
    };
    let xml = decode_task_xml(&out.stdout);
    let healthy = parse_task_health(&xml, &exe);
    log_info!("scheduled task healthy: {healthy}");
    healthy
}

/// Decodes `schtasks /xml` output, which is UTF-16 with a BOM. Plain UTF-8
/// decoding would leave a NUL between every character and match nothing.
fn decode_task_xml(bytes: &[u8]) -> String {
    let words: Vec<u16> = if bytes.starts_with(&[0xFF, 0xFE]) {
        bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect()
    } else {
        return String::from_utf8_lossy(bytes).into_owned();
    };
    String::from_utf16_lossy(&words)
}

/// Checks a task definition against this executable. Pure string matching on
/// purpose: no XML library for three tags, and a missing tag must read as
/// unhealthy (the Scheduler defaults block battery starts).
pub(crate) fn parse_task_health(xml: &str, exe: &str) -> bool {
    let Some(cmd) = tag_content(xml, "Command") else {
        return false;
    };
    let cmd = cmd.trim().trim_matches('"');
    if !cmd.eq_ignore_ascii_case(exe) {
        return false;
    }
    // Both must be explicitly false; absent means the blocking default.
    tag_content(xml, "DisallowStartIfOnBatteries").is_some_and(|v| v.trim() == "false")
        && tag_content(xml, "StopIfGoingOnBatteries").is_some_and(|v| v.trim() == "false")
}

/// First `<Tag>…</Tag>` body, if any.
fn tag_content<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(open.as_str())? + open.len();
    let end = xml[start..].find(close.as_str())?;
    Some(&xml[start..start + end])
}

/// Removes the worker task.
pub fn unregister_task() -> bool {
    let args = ["/delete", "/tn", TASK_NAME, "/f"];
    let ok = crate::sysutil::silent_command("schtasks")
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        log_info!("scheduled task removed");
    }
    ok
}

/// Asks the registered task to run the worker. Returns `false` if there is no
/// task, in which case the caller should fall back to [`elevate_directly`].
pub fn trigger_worker() -> bool {
    run_task(&["/run", "/tn", TASK_NAME])
}

/// Registers the worker task. Must itself be running elevated.
///
/// Imported from an XML definition rather than built with `/sc onlogon`:
/// the `schtasks` command line cannot express the battery policy, and the
/// defaults block starts on battery and kill a running worker on unplug -
/// exactly the workload this task exists for. The XML keeps the same
/// identity as before (current user, highest privileges, logon trigger) and
/// only changes the power policy plus the executable path.
pub fn register_worker_task() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy().into_owned();
    // XML-escape the path; usernames routinely contain `&`.
    let action = format!("\"{}\"", exe.replace('&', "&amp;"));

    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n\
         <Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n\
         <RegistrationInfo><URI>\\{TASK_NAME}</URI></RegistrationInfo>\n\
         <Principals><Principal id=\"Author\"><LogonType>InteractiveToken</LogonType>\
         <RunLevel>HighestAvailable</RunLevel></Principal></Principals>\n\
         <Settings>\
         <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>\
         <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>\
         <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>\
         <AllowDemandStart>true</AllowDemandStart>\
         <Enabled>true</Enabled>\
         </Settings>\n\
         <Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers>\n\
         <Actions Context=\"Author\"><Exec><Command>{action}</Command>\
         <Arguments>--worker</Arguments></Exec></Actions>\n\
         </Task>\n"
    );

    let path = std::env::temp_dir().join("xmg-task.xml");
    std::fs::write(&path, xml.encode_utf16().flat_map(|c| c.to_le_bytes()))
        .map_err(|e| e.to_string())?;
    let out = crate::sysutil::silent_command("schtasks")
        .args([
            "/create",
            "/tn",
            TASK_NAME,
            "/xml",
            &path.to_string_lossy(),
            "/f",
        ])
        .output()
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&path);
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    log_info!("background task registered (battery-allowed)");
    Ok(())
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
        r.0 as isize > 32
    }
    #[cfg(not(windows))]
    {
        let _ = &exe;
        false
    }
}

/// Runs a job, preferring the silent scheduled task.
///
/// Returns `true` when something was actually launched. A `false` here used to
/// be swallowed, which left the UI's `busy` flag set forever and made every
/// later button look dead - the job was queued and nothing ever ran it.
pub fn submit(job: Job) -> bool {
    log_info!("submitting job: {job:?}");

    // Already elevated: run it right here on a background thread instead of
    // round-tripping through the task or a second UAC prompt. ShellExecute
    // with "runas" prompts even when already elevated, which is exactly the
    // spurious prompt from the bug report.
    if crate::sysutil::is_elevated() {
        log_info!("already elevated - running the job in-process");
        return submit_in_process(job);
    }

    jobs::clear_pending();

    if let Err(e) = jobs::write_job(&job) {
        log_error!("could not queue the job: {e}");
        jobs::write_result(&JobResult::fail(format!("Could not prepare the job: {e}")));
        return false;
    }

    if trigger_worker() {
        log_info!("worker triggered through the scheduled task");
        return true;
    }

    log_warn!("scheduled task did not start; falling back to a direct elevated run");
    if elevate_directly() {
        log_info!("direct elevated run started");
        return true;
    }

    // Neither route worked. Say so loudly rather than leaving the UI to hang.
    log_error!("could not start the worker by either route");
    jobs::write_result(&JobResult::fail(
        "The background task could not be started.\n\n\
         Run the app once as administrator so it can register its task:\n\
         right-click the executable > Run as administrator.",
    ));
    false
}

/// Runs a job without ever prompting. For background triggers (the charger
/// watcher) that must stay silent.
///
/// Returns `true` when the job was handed to the scheduled task. When the task
/// is missing this returns `false` and does nothing else - in particular it
/// never falls back to `elevate_directly`, whose UAC prompt is what used to
/// pop up on every plug and unplug.
pub fn submit_silent(job: Job) -> bool {
    log_info!("submitting silent job: {job:?}");

    if crate::sysutil::is_elevated() {
        log_info!("already elevated - running the silent job in-process");
        return submit_in_process(job);
    }

    jobs::clear_pending();

    if let Err(e) = jobs::write_job(&job) {
        log_error!("could not queue the silent job: {e}");
        return false;
    }

    if trigger_worker() {
        log_info!("silent worker triggered through the scheduled task");
        return true;
    }

    // Task missing: leave the queued file in place and report, so the next
    // foreground submit picks it up instead of losing it. The UI's Finish
    // setup button is the way back to a working task.
    log_error!("scheduled task missing - silent job left queued, no UAC shown");
    false
}

/// Executes the job on a background thread and reports through the result
/// slot, so the UI's normal `poll_events` path picks it up. Only called when
/// already elevated, so no privilege boundary is crossed.
fn submit_in_process(job: Job) -> bool {
    jobs::clear_pending();
    std::thread::Builder::new()
        .name("xmg-inprocess-worker".into())
        .spawn(move || {
            log_info!("in-process worker started");
            let result = crate::worker::execute(job).unwrap_or_else(|e| {
                log_error!("in-process job failed outright: {e}");
                JobResult::fail(e.to_string())
            });
            jobs::write_result(&result);
            log_info!("in-process worker finished: ok={}", result.ok);
        })
        .is_ok()
}

/// One-time setup: create the worker task if it is missing - or repair it if
/// it points elsewhere or still carries the battery-blocking defaults.
///
/// Returns `Ok(true)` when everything is ready afterwards.
pub fn ensure_setup() -> Result<bool, String> {
    if task_is_healthy() {
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
        if r.0 as isize > 32 {
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
