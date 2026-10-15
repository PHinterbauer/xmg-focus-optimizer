//! Reading and writing Windows power settings through `powercfg`.
//!
//! Every write is read back and compared, so the UI can report what actually
//! landed on the machine rather than what we hoped would.

use std::collections::HashMap;
use std::process::Command;

use crate::errors::{AppError, AppResult};
use crate::profile::PlanEntry;

/// Runs `powercfg` and returns stdout.
fn powercfg(args: &[&str]) -> AppResult<String> {
    let out = Command::new("powercfg")
        .args(args)
        .output()
        .map_err(|e| {
            crate::log_error!("powercfg could not be started: {e}");
            AppError::Power(format!("powercfg could not be started: {e}"))
        })?;
    crate::log::command("powercfg", args, &out);
    if !out.status.success() {
        crate::log_error!(
            "powercfg {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Runs `powercfg`, returning `Some(text)` only when it exited cleanly.
fn powercfg_ok(args: &[&str]) -> Option<String> {
    let out = Command::new("powercfg").args(args).output().ok()?;
    crate::log::command("powercfg", args, &out);
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

/// GUID of the currently active power scheme.
pub fn active_scheme() -> AppResult<String> {
    let txt = powercfg(&["/getactivescheme"])?;
    extract_guid(&txt).ok_or_else(|| {
        AppError::Power(format!("could not read the active scheme from: {txt}"))
    })
}

pub fn is_admin() -> bool {
    // `net session` needs no admin to run but fails without it; a cleaner test
    // is to ask the token directly.
    crate::sysutil::is_elevated()
}

/// `powercfg /list`, as (guid, name) pairs.
pub fn list_schemes() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(txt) = powercfg(&["/list"]) else {
        return out;
    };
    for line in txt.lines() {
        let t = line.trim();
        let Some(open) = t.find('(') else { continue };
        let Some(close) = t.rfind(')') else { continue };
        if close < open {
            continue;
        }
        let guid = t[..open].trim();
        let name = &t[open + 1..close];
        if guid.len() == 36 && guid.chars().filter(|c| *c == '-').count() == 4 {
            out.push((guid.to_string(), name.to_string()));
        }
    }
    out
}

/// Duplicates a scheme and returns the new GUID.
pub fn duplicate_scheme(base: &str) -> AppResult<String> {
    let txt = powercfg(&["/duplicatescheme", base])?;
    extract_guid(&txt).ok_or_else(|| {
        AppError::Power(format!("could not duplicate scheme {base}: {txt}"))
    })
}

pub fn rename_scheme(guid: &str, name: &str, description: &str) -> AppResult<()> {
    powercfg_ok(&["/changename", guid, name, description])
        .ok_or_else(|| AppError::Power(format!("could not rename {guid}")))?;
    Ok(())
}

pub fn delete_scheme(guid: &str) -> AppResult<()> {
    powercfg(&["/delete", guid]).map(|_| ())
}

pub fn set_active(guid: &str) -> AppResult<()> {
    powercfg(&["/setactive", guid]).map(|_| ())
}

/// Sets one setting for AC and DC. Returns `true` only when both writes stuck.
pub fn set_value(
    scheme: &str,
    subgroup: &str,
    setting: &str,
    ac: u32,
    dc: u32,
) -> AppResult<bool> {
    let ac_s = ac.to_string();
    let dc_s = dc.to_string();
    let ok_ac = powercfg_ok(&[
        "/setacvalueindex", scheme, subgroup, setting, ac_s.as_str(),
    ])
    .is_some();
    let ok_dc = powercfg_ok(&[
        "/setdcvalueindex", scheme, subgroup, setting, dc_s.as_str(),
    ])
    .is_some();
    Ok(ok_ac && ok_dc)
}

/// One setting read back from a scheme.
#[derive(Debug, Clone, Copy)]
pub struct Value {
    pub ac: u32,
    pub dc: u32,
}

/// Reads every setting of a scheme into a map keyed by setting GUID.
pub fn read_all(scheme: &str) -> HashMap<String, Value> {
    let mut map = HashMap::new();
    let Ok(txt) = powercfg(&["/qh", scheme]) else {
        return map;
    };

    let mut set_guid = String::new();
    let mut ac: Option<u32> = None;

    for line in txt.lines() {
        let l = line.trim();
        if let Some(g) = after(l, "Power Setting GUID:") {
            set_guid = g.to_ascii_lowercase();
            ac = None;
        } else if let Some(v) = hex_after(l, "Current AC Power Setting Index:") {
            ac = Some(v);
        } else if let Some(v) = hex_after(l, "Current DC Power Setting Index:") {
            if let (Some(a), Some(s)) = (ac, Some(set_guid.clone())) {
                if !s.is_empty() {
                    map.insert(s, Value { ac: a, dc: v });
                }
            }
        }
    }
    map
}

/// Compares what we wanted against what the scheme actually contains.
pub fn verify(scheme: &str, entries: &[PlanEntry]) -> (usize, Vec<String>) {
    let actual = read_all(scheme);
    let mut ok = 0usize;
    let mut bad = Vec::new();
    for e in entries {
        match actual.get(&e.setting.to_ascii_lowercase()) {
            Some(v) if v.ac == e.ac && v.dc == e.dc => ok += 1,
            Some(v) => bad.push(format!(
                "{}: got AC {}/DC {}, wanted AC {}/DC {}",
                e.label, v.ac, v.dc, e.ac, e.dc
            )),
            None => bad.push(format!("{}: not present in the scheme", e.label)),
        }
    }
    (ok, bad)
}

// ---------------------------------------------------------------- parsing ---

fn after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let p = line.find(key)? + key.len();
    let v = line[p..].trim();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

fn hex_after(line: &str, key: &str) -> Option<u32> {
    let v = after(line, key)?;
    let v = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X"))?;
    u32::from_str_radix(v, 16).ok()
}

/// Pulls the first GUID out of a blob of text.
pub fn extract_guid(txt: &str) -> Option<String> {
    let mut chars = txt.char_indices();
    while let Some((i, _)) = chars.next() {
        let rest = &txt[i..];
        if rest.len() >= 36 {
            let candidate = &rest[..36];
            if is_guid(candidate) {
                return Some(candidate.to_ascii_lowercase());
            }
        }
    }
    None
}

pub fn is_guid(s: &str) -> bool {
    s.len() == 36
        && s.chars().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

// ------------------------------------------------------------- power state ---

/// `true` when running on mains power.
pub fn on_ac_power() -> bool {
    crate::sysutil::on_ac_power()
}