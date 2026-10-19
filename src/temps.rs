//! Live temperature readings for the header.
//!
//! Two very different sources, because Windows exposes no single CPU/GPU
//! temperature API:
//!
//! * GPU: `nvidia-smi --query-gpu=temperature.gpu`. Always works where an
//!   NVIDIA card is present, which is every machine this app targets.
//! * CPU: best effort through documented WMI only, best first:
//!   LibreHardwareMonitor (`root/LibreHardwareMonitor`), its predecessor
//!   OpenHardwareMonitor, the firmware `MSAcpi_ThermalZoneTemperature` zone,
//!   then the `Win32_PerfFormattedData_Counters_ThermalZoneInformation`
//!   counter. Absent on some machines - including the reference XMG Focus 15,
//!   where the MSAcpi class simply does not exist.
//!
//! What is deliberately NOT here: reading the embedded controller directly
//! (WinRing0, `SetDCHU_Data` in `InsydeDCHU.dll`, `nvdrsdb0.bin` and friends).
//! Those interfaces are unpublished, and a wrong call can leave a laptop
//! unbootable. See README "Safety is the design".
//!
//! Polling lives on its own thread and the UI only ever `try_lock`s the
//! latest reading, so a slow sensor can never stall a frame. Interval is a
//! deliberate 20 s: temperatures move slowly, and each poll is a process
//! spawn. The header always shows both pills; a sensor with no data reads
//! "–" with a tooltip instead of vanishing, and no value is ever invented.

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
            let mut cpu_skip = 0u32;
            while !stop2.load(Ordering::Relaxed) {
                let t = poll_once(&mut cpu_skip);
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

/// Polls to skip after an empty CPU answer before probing again. WMI thermal
/// zones flake - they vanish across sleep/wake, under load, or behind a
/// denied namespace - and one empty answer used to kill the CPU pill for the
/// rest of the process. At 20 s per poll this retries roughly every 10 min:
/// cheap enough to leave running, prompt enough to heal by itself.
const CPU_RETRY_EVERY: u32 = 30;

fn poll_once(cpu_skip: &mut u32) -> Temps {
    Temps {
        gpu: poll_gpu(),
        cpu: if *cpu_skip > 0 {
            *cpu_skip -= 1;
            None
        } else {
            match poll_cpu() {
                CpuProbe::Found(v) => Some(v),
                // Empty answer or failure alike: back off and try again
                // later. A dead sensor costs one silent probe per backoff
                // window; a flaky one heals on its own.
                CpuProbe::Absent | CpuProbe::Failed => {
                    *cpu_skip = CPU_RETRY_EVERY;
                    None
                }
            }
        },
    }
}

/// CPU sources, best first: LibreHardwareMonitor (real package temps, if the
/// user runs it), its predecessor OpenHardwareMonitor, then the firmware
/// MSAcpi zone, then the thermal-zone performance counter. The first answer
/// wins; a missing class is not an error. Direct EC access stays forbidden
/// (see the module docs), so when none of these answer the CPU pill reads
/// "–" instead of guessing.
fn poll_cpu() -> CpuProbe {
    if let Some(v) =
        poll_lhm("LibreHardwareMonitor").or_else(|| poll_lhm("OpenHardwareMonitor"))
    {
        return CpuProbe::Found(v);
    }
    if let CpuProbe::Found(v) = poll_cpu_wmi() {
        return CpuProbe::Found(v);
    }
    poll_cpu_thermal_info()
}

/// Queries one HardwareMonitor WMI namespace for CPU temperatures. Emits
/// `Name|Value` lines and lets [`pick_cpu_temp`] choose; anything failing
/// (namespace absent, service stopped) is simply `None`.
fn poll_lhm(namespace: &str) -> Option<f32> {
    let ns = format!("root/{namespace}");
    let out = crate::sysutil::silent_command("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Get-CimInstance -Namespace {ns} -Class Sensor \
                 | Where-Object {{ $_.SensorType -eq 'Temperature' }} \
                 | ForEach-Object {{ \"$($_.Name)|$($_.Value)\" }}"
            ),
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    pick_cpu_temp(&String::from_utf8_lossy(&out.stdout))
}

/// Picks the CPU temperature from `Name|Value` lines: the package sensor
/// first, then any CPU-named sensor. Values outside a plausible range are
/// ignored rather than displayed.
pub(crate) fn pick_cpu_temp(text: &str) -> Option<f32> {
    let mut fallback = None;
    for line in text.lines() {
        let (name, value) = match line.split_once('|') {
            Some(p) => p,
            None => continue,
        };
        let Ok(v) = value.trim().parse::<f32>() else {
            continue;
        };
        if !(0.0..=125.0).contains(&v) {
            continue;
        }
        if name.trim() == "CPU Package" {
            return Some(v);
        }
        if fallback.is_none() && name.to_ascii_lowercase().contains("cpu") {
            fallback = Some(v);
        }
    }
    fallback
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

/// Last-resort CPU source: the documented thermal-zone performance counter.
/// Same unit as the MSAcpi zone (tenths of Kelvin), so the same parser
/// applies - and a counter reporting plain Celsius is rejected as implausible
/// rather than displayed. A missing class is simply `Failed`, which feeds the
/// normal backoff like every other absent sensor.
fn poll_cpu_thermal_info() -> CpuProbe {
    let out = match crate::sysutil::silent_command("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-CimInstance \
             -Class Win32_PerfFormattedData_Counters_ThermalZoneInformation \
             | ForEach-Object { $_.HighPrecisionTemperature; $_.Temperature }",
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

    #[test]
    fn lhm_prefers_the_package_sensor() {
        let out = "GPU Core|48.0\nCPU Package|52.5\nCPU Core #1|61.0\n";
        assert_eq!(pick_cpu_temp(out), Some(52.5));
    }

    #[test]
    fn lhm_falls_back_to_any_cpu_sensor() {
        let out = "GPU Core|48.0\nCPU Core #1|61.0\n";
        assert_eq!(pick_cpu_temp(out), Some(61.0));
    }

    #[test]
    fn lhm_rejects_non_cpu_and_absurd_values() {
        assert_eq!(pick_cpu_temp("GPU Core|48.0\n"), None);
        assert_eq!(pick_cpu_temp(""), None);
        assert_eq!(pick_cpu_temp("CPU Package|999.0\n"), None);
        assert_eq!(pick_cpu_temp("CPU Package|hot\n"), None);
    }

    #[test]
    fn thermal_counter_shares_the_tenths_of_kelvin_parser() {
        // Two counter lines: the first sane one wins.
        let v = parse_wmi_temp("3075\n3080\n").unwrap();
        assert!((v - 34.35).abs() < 0.01, "got {v}");
        // Plain Celsius from a counter that reports differently must not
        // display as a temperature - it reads as absent, never as invented.
        assert_eq!(parse_wmi_temp("45\n46\n"), None);
    }
}
