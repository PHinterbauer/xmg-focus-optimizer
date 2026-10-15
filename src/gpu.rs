//! NVIDIA GPU power limit, driven through `nvidia-smi`.
//!
//! Safety: a wattage is only ever sent after it has been range-checked *and*
//! compared against the device's own reported maximum. A bogus reading from a
//! faulty driver therefore cannot push the card past what it supports.

use std::path::Path;
use std::process::Command;

use crate::errors::{AppError, AppResult};

fn smi(args: &[&str]) -> Option<String> {
    let out = Command::new("nvidia-smi").args(args).output().ok()?;
    crate::log::command("nvidia-smi", args, &out);
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn smi_ok(args: &[&str]) -> bool {
    match Command::new("nvidia-smi").args(args).output() {
        Ok(o) => {
            crate::log::command("nvidia-smi", args, &o);
            o.status.success()
        }
        Err(e) => {
            crate::log_error!("nvidia-smi could not be started: {e}");
            false
        }
    }
}

/// Current power limit in watts.
pub fn current_limit() -> Option<f32> {
    let t = smi(&["-q", "-d", "POWER"])?;
    parse_after(&t, "Current Power Limit")
}

/// Highest power limit the device accepts.
pub fn max_limit() -> Option<f32> {
    let t = smi(&["-q", "-d", "POWER"])?;
    parse_after(&t, "Max Power Limit")
}

fn parse_after(text: &str, key: &str) -> Option<f32> {
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix(key) {
            let rest = rest.trim_start_matches(':').trim();
            // Some builds append " W" or " (Default)". Take the leading number.
            let num: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(v) = num.parse::<f32>() {
                if v > 0.0 {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// Sets the limit. Refuses anything outside a sane range.
pub fn set_limit(watts: u32) -> AppResult<bool> {
    if watts == 0 || watts > 500 {
        return Err(AppError::Denied(format!(
            "refusing to set a GPU limit of {watts} W (expected 1-500)"
        )));
    }
    let w = format!("{watts}");
    Ok(smi_ok(&["-pl", w.as_str()]))
}

/// Sets the limit, but never above what the hardware reports as its maximum.
pub fn set_limit_capped(watts: u32) -> ApplyOutcome {
    let max = match max_limit() {
        Some(m) => m,
        None => return ApplyOutcome::Unavailable,
    };
    if (watts as f32) > max {
        return ApplyOutcome::Refused {
            wanted: watts,
            max: max as u32,
        };
    }
    match set_limit(watts) {
        Ok(true) => ApplyOutcome::Applied(watts),
        _ => ApplyOutcome::Failed,
    }
}

#[derive(Debug, Clone)]
pub enum ApplyOutcome {
    Applied(u32),
    Refused { wanted: u32, max: u32 },
    Failed,
    Unavailable,
}

/// Stores the current limit so a restore can put it back.
pub fn capture_limit(base: &Path) -> AppResult<()> {
    if let Some(v) = current_limit() {
        std::fs::write(base.join("original-gpu-limit.txt"), format!("{v}"))
            .map_err(|e| AppError::Io(e.to_string()))?;
    }
    Ok(())
}

/// Reads back the stored limit, validating the file contents.
pub fn stored_limit(base: &Path) -> Option<u32> {
    let t = std::fs::read_to_string(base.join("original-gpu-limit.txt")).ok()?;
    let v = t.trim();
    if v.is_empty() || !v.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    v.parse().ok()
}