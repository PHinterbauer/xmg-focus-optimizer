//! Background watcher: notices the charger being plugged in or pulled out and
//! re-applies the matching settings without the user touching anything.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use crate::config::Settings;
use crate::jobs::{self, Job};
use crate::profile::Mode;
use crate::sysutil;

#[derive(Debug, Clone)]
pub enum MonitorEvent {
    PowerChanged { on_ac: bool, mode: Mode, triggered: bool },
    Error(String),
}

const POLL: Duration = Duration::from_secs(8);

/// Starts the watcher. The returned flag stops it.
pub fn spawn(settings: Settings, tx: Sender<MonitorEvent>) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();

    std::thread::Builder::new()
        .name("xmg-power-watch".into())
        .spawn(move || {
            let mut last_on_ac = sysutil::on_ac_power();

            while !stop2.load(Ordering::Relaxed) {
                std::thread::sleep(POLL);

                let on_ac = sysutil::on_ac_power();
                if on_ac == last_on_ac {
                    continue;
                }
                last_on_ac = on_ac;

                // Re-read settings each time so a change in the UI takes effect
                // without a restart.
                let live = Settings::load();
                let _ = &settings;

                if !live.auto_switch_on_power_change {
                    let _ = tx.send(MonitorEvent::PowerChanged {
                        on_ac,
                        mode: live.mode,
                        triggered: false,
                    });
                    continue;
                }

                jobs::submit(Job::Apply {
                    mode: live.mode,
                    control_fans: live.control_fans,
                    control_gpu: live.control_gpu_limit,
                });

                let _ = tx.send(MonitorEvent::PowerChanged {
                    on_ac,
                    mode: live.mode,
                    triggered: true,
                });
            }
        })
        .ok();

    stop
}