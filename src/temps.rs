//! Live temperature readings for the header.
//!
//! Two very different sources, because Windows exposes no single CPU/GPU
//! temperature API:
//!
//! * GPU: `nvidia-smi --query-gpu=temperature.gpu`. Always works where an
//!   NVIDIA card is present, which is every machine this app targets.
//! * CPU: the `MSAcpi_ThermalZoneTemperature` WMI class. Present on some
//!   machines, absent on others - including the reference XMG Focus 15, where
//!   the class simply does not exist. Best effort: probe once, and if the
//!   class is absent stop asking for the rest of the process instead of
//!   spawning a PowerShell on every poll for a sensor that will never answer.
//!
//! Polling lives on its own thread and the UI only ever `try_lock`s the
//! latest reading, so a slow sensor can never stall a frame. Interval is a
//! deliberate 20 s: temperatures move slowly, and each poll is a process
//! spawn.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::log::{log_info, log_warn};

/// Seconds between polls. Temperatures move slowly; there is no reason to
/// spawn processes more often than this.
const POLL: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, Default)]
pub struct Temps {
    pub gpu: Option<f32>,
    pub cpu: Option<f32>,
}

/// Starts the poller. Returns the shared latest reading plus a flag that stops
/// the thread; set it on quit.
pub fn spawn() -> (Arc<Mutex<Temps>>, Arc<AtomicBool>) {
    let shared = Arc::new(Mutex::new(Temps::default()));
    let stop = Arc::new(AtomicBool::new(false));
    let shared2 = shared.clone();
    let stop2 = stop.clone();

    std::thread::Builder::new()
        .name("xmg-temps".into())
        .spawn(move || {
            log_info!("temperature poller started");
            let mut cpu_dead = false;
            while !stop2.load(Ordering::Relaxed) {
                let t = poll_once(&mut cpu_dead);
                if let Ok(mut g) = shared2.lock() {
                    *g = t;
                }
                // Sleep in small slices so quitting does not wait out a full
                // interval with the window already gone.
                for _ in 0..20 {
                    if stop2.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(POLL / 20);
                }
            }
            log_info!("temperature poller stopped");
        })
        .ok();

    (shared, stop)
}

fn poll_once(cpu_dead: &mut bool) -> Temps {
    Temps {
        gpu: poll_gpu(),
        cpu: if *cpu_dead {
            None
        } else {
            match poll_cpu_wmi() {
                // A clean empty answer means the class is absent on this
                // machine; it will not appear later, so stop asking.
                CpuProbe::Absent => {
                    log_info!("no CPU temperature sensor (MSAcpi WMI class absent)");
                    *cpu_dead = true;
                    None
                }
                CpuProbe::Found(v) => Some(v),
                // A spawn or parse failure might be transient, so keep trying.
                CpuProbe::Failed => None,
            }
        },
    }
}

fn poll_gpu() -> Option<f32> {
    let out = crate::sysutil::silent_command("nvidia-smi")
        .args(["--query-gpu=temperature.gpu", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_nvidia_temp(&String::from_utf8_lossy(&out.stdout))
}

/// First line that parses as a number. `nvidia-smi` answers `[N/A]` when the
/// card is unreachable, which must read as absent, not zero.
pub(crate) fn parse_nvidia_temp(text: &str) -> Option<f32> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .find_map(|l| l.parse::<f32>().ok())
        .filter(|v| *v > 0.0)
}

enum CpuProbe {
    Found(f32),
    /// The class answered cleanly with no data: absent for good.
    Absent,
    /// Anything else: spawn failure, nonzero exit, unparsable output.
    Failed,
}

fn poll_cpu_wmi() -> CpuProbe {
    let out = match crate::sysutil::silent_command("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-CimInstance -Namespace root/wmi \
             -Class MSAcpi_ThermalZoneTemperature \
             | Select-Object -ExpandProperty CurrentTemperature",
        ])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            log_warn!("CPU temp probe could not start powershell: {e}");
            return CpuProbe::Failed;
        }
    };
    if !out.status.success() {
        return CpuProbe::Failed;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    match parse_wmi_temp(&text) {
        Some(v) => CpuProbe::Found(v),
        // Clean run, no numbers: the class is not there.
        None if text.trim().is_empty() => CpuProbe::Absent,
        None => CpuProbe::Failed,
    }
}

/// WMI reports tenths of Kelvin, one value per line with a header line mixed
/// in depending on formatting. Takes the first sane reading.
pub(crate) fn parse_wmi_temp(text: &str) -> Option<f32> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|l| l.parse::<u32>().ok())
        // Tenths of Kelvin near room temperature and above; anything else is
        // a header, an error code, or nonsense.
        .filter(|v| (2000..=4000).contains(v))
        .map(|v| v as f32 / 10.0 - 273.15)
        .next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_temp_parses_plain_value() {
        assert_eq!(parse_nvidia_temp("50\n"), Some(50.0));
        assert_eq!(parse_nvidia_temp("  62.0  \n"), Some(62.0));
    }

    #[test]
    fn nvidia_temp_rejects_unavailable() {
        assert_eq!(parse_nvidia_temp("[N/A]\n"), None);
        assert_eq!(parse_nvidia_temp(""), None);
        assert_eq!(parse_nvidia_temp("0\n"), None);
    }

    #[test]
    fn wmi_temp_converts_tenths_of_kelvin() {
        // 3000 = 300.0 K = 26.85 C.
        let v = parse_wmi_temp("CurrentTemperature\r\n------------------\r\n              3000\r\n").unwrap();
        assert!((v - 26.85).abs() < 0.01, "got {v}");
        // Bare value works too.
        let v = parse_wmi_temp("3105\n").unwrap();
        assert!((v - 37.35).abs() < 0.01, "got {v}");
    }

    #[test]
    fn wmi_temp_rejects_garbage() {
        assert_eq!(parse_wmi_temp(""), None);
        assert_eq!(parse_wmi_temp("CurrentTemperature\n"), None);
        // 50000 would be 4700 C: not a room anyone works in.
        assert_eq!(parse_wmi_temp("50000\n"), None);
    }
}
