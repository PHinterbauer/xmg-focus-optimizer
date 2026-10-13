//! XMG Focus Optimizer
//!
//! Two safe performance modes for the SCHENKER XMG FOCUS 15.
//!
//! Modes:      `quiet` | `performance`
//! Commands:   `apply`, `restore`, `snapshot`, `factory`, `help`
//! Internals:  `--worker`, `--setup-elevated`, `--tray`

mod backup;
mod config;
mod drift;
mod elevate;
mod errors;
mod fans;
mod gpu;
mod icon;
mod jobs;
mod monitor;
mod paths;
mod power;
mod profile;
mod startup;
mod sysutil;
#[cfg(test)]
mod tests;
mod tray;
mod ui;
mod worker;

use std::sync::mpsc::channel;

use config::Settings;
use jobs::Job;
use profile::Mode;

const MUTEX_NAME: &str = "Local\\XMGOptimizerSingleton";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let lowered: Vec<String> = args.iter().map(|a| a.to_ascii_lowercase()).collect();

    // ---------------------------------------------------------- worker mode --
    if lowered.iter().any(|a| a == "--worker") {
        run_worker();
        return;
    }

    // Elevated one-time setup, launched via ShellExecute runas.
    if lowered.iter().any(|a| a == "--setup-elevated") {
        match elevate::register_worker_task() {
            Ok(()) => worker::log("background task registered"),
            Err(e) => worker::log(&format!("task registration failed: {e}")),
        }
        return;
    }

    // ---------------------------------------------------------- CLI commands --
    if let Some(cmd) = lowered.first() {
        match cmd.as_str() {
            "help" | "--help" | "-h" | "/?" => {
                print_help();
                return;
            }
            "apply" | "restore" | "snapshot" | "factory" => {
                run_cli_command(cmd, &lowered);
                return;
            }
            _ => {}
        }
    }

    // ----------------------------------------------------------------- GUI --
    // `--autostart` marks a launch that came from the Run key, which is the
    // signal to silently restore the saved mode.
    run_gui(
        lowered.iter().any(|a| a == "--tray"),
        lowered.iter().any(|a| a == "--autostart"),
    );
}

fn run_worker() {
    let job = match jobs::read_job() {
        Ok(j) => j,
        Err(_) => {
            // Nothing queued (for example the task fired at log-on). Exit quietly.
            return;
        }
    };
    let result = worker::execute(job).unwrap_or_else(|e| jobs::JobResult::fail(e.to_string()));
    jobs::write_result(&result);
    jobs::clear_pending();
}

fn run_cli_command(cmd: &str, args: &[String]) {
    let settings = Settings::load();

    let job = match cmd {
        "apply" => {
            let mode = args
                .iter()
                .position(|a| a == "quiet" || a == "performance")
                .and_then(|i| args.get(i + 1).map(|s| Mode::parse(s)))
                .unwrap_or(settings.mode);
            Job::Apply {
                mode,
                control_fans: settings.control_fans,
                control_gpu: settings.control_gpu_limit,
            }
        }
        "restore" => {
            let source = if args.iter().any(|a| a == "snapshot") {
                // most recent snapshot
                match backup::list_snapshots() {
                    Ok(list) => match list.first() {
                        Some(s) => backup::RestoreSource::Snapshot(s.id.clone()),
                        None => backup::RestoreSource::Factory,
                    },
                    Err(_) => backup::RestoreSource::Factory,
                }
            } else {
                backup::RestoreSource::Factory
            };
            Job::Restore { source }
        }
        "snapshot" => Job::Snapshot {
            reason: "cli".into(),
            manual: true,
        },
        "factory" => Job::CreateFactoryBackup {
            note: "cli".into(),
        },
        _ => return,
    };

    if !sysutil::is_elevated() {
        jobs::clear_pending();
        if let Err(e) = jobs::write_job(&job) {
            eprintln!("could not prepare the job: {e}");
            return;
        }
        if !elevate::elevate_directly() {
            eprintln!("administrator rights are required - right-click and run as administrator");
        }
        return;
    }

    let r = worker::execute(job).unwrap_or_else(|e| jobs::JobResult::fail(e.to_string()));
    println!("{}", r.summary);
    for d in &r.details {
        println!("  {d}");
    }
    for w in &r.warnings {
        println!("  ! {w}");
    }
}

fn print_help() {
    println!(
        "\
XMG Focus Optimizer - safe performance modes for the SCHENKER XMG FOCUS

  XMG-Optimizer.exe                  open the window
  XMG-Optimizer.exe --tray           start hidden in the notification area
  XMG-Optimizer.exe apply quiet      apply a mode   (quiet | performance)
  XMG-Optimizer.exe restore          restore the initial backup
  XMG-Optimizer.exe restore snapshot restore the most recent snapshot
  XMG-Optimizer.exe snapshot         save a snapshot of the current state
  XMG-Optimizer.exe factory          archive the current backup and make a new one
  XMG-Optimizer.exe help             this text

Commands that change settings need administrator rights. The window registers
a background task once so the common case never prompts."
    );
}

fn run_gui(start_hidden: bool, autostarted: bool) {
    // Only one instance at a time. A second launch just reveals the first.
    let _guard = match sysutil::acquire_single_instance(MUTEX_NAME) {
        Ok(g) => g,
        Err(()) => {
            sysutil::notify_existing_instance(MUTEX_NAME);
            return;
        }
    };

    let settings = Settings::load();

    let (tx_tray, rx_tray) = channel::<tray::TrayCommand>();
    let (tx_monitor, rx_monitor) = channel::<monitor::MonitorEvent>();
    let stop_watch = monitor::spawn(settings.clone(), tx_monitor.clone());

    let hidden = start_hidden || settings.start_minimized_to_tray;

    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("XMG Optimizer")
            .with_inner_size([860.0, 720.0])
            .with_min_inner_size([660.0, 580.0])
            .with_decorations(false)
            .with_resizable(true),
        ..Default::default()
    };

    let settings_for_app = settings.clone();
    let run = eframe::run_native(
        "XMG Optimizer",
        native,
        Box::new(move |cc| {
            let app: Box<dyn eframe::App> = Box::new(ui::App::new(
                cc.egui_ctx.clone(),
                settings_for_app,
                autostarted,
                rx_tray,
                rx_monitor,
                tx_tray,
                tx_monitor,
                stop_watch,
                hidden,
            ));
            Ok(app)
        }),
    );

    match run {
        Ok(()) => {
            if let Err(e) = settings.save() {
                eprintln!("could not save settings: {e}");
            }
        }
        Err(e) => {
            // Fall back to a plain message so a graphics problem is not silent.
            eprintln!("the window could not be created: {e}");
        }
    }
}