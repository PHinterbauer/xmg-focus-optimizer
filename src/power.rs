//! Reading and writing Windows power settings through `powercfg`.
//!
//! Every write is read back and compared, so the UI can report what actually
//! landed on the machine rather than what we hoped would.

use std::collections::HashMap;

use crate::errors::{AppError, AppResult};
use crate::profile::PlanEntry;
use crate::log::{log_error};

/// Runs `powercfg` and returns stdout. Never shows a console window; see
/// [`crate::sysutil::silent_command`].
fn powercfg(args: &[&str]) -> AppResult<String> {
    let out = crate::sysutil::silent_command("powercfg")
        .args(args)
        .output()
        .map_err(|e| {
            log_error!("powercfg could not be started: {e}");
            AppError::Power(format!("powercfg could not be started: {e}"))
        })?;
    crate::log::command("powercfg", args, &out);
    if !out.status.success() {
        log_error!(
            "powercfg {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Runs `powercfg`, returning `Some(text)` only when it exited cleanly.
fn powercfg_ok(args: &[&str]) -> Option<String> {
    let out = crate::sysutil::silent_command("powercfg").args(args).output().ok()?;
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
///
/// The line looks like `Power Scheme GUID: <guid>  (Name)`. The GUID is pulled
/// out with [`extract_guid`] rather than by splitting on the parenthesis, which
/// is what used to leave the `Power Scheme GUID: ` prefix attached. That made
/// every entry fail the length check, so the list came back empty, every scheme
/// read as "unknown", and our own old schemes were never cleaned up.
pub fn list_schemes() -> Vec<(String, String)> {
    let Ok(txt) = powercfg(&["/list"]) else {
        return Vec::new();
    };
    parse_scheme_list(&txt)
}

/// Pure parser behind [`list_schemes`], kept separate so tests can feed it real
/// `powercfg` output without running anything.
pub(crate) fn parse_scheme_list(txt: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in txt.lines() {
        let t = line.trim();
        let Some(guid) = extract_guid(t) else { continue };
        let name = match (t.find('('), t.rfind(')')) {
            (Some(open), Some(close)) if close > open => t[open + 1..close].to_string(),
            _ => String::new(),
        };
        out.push((guid, name));
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
#[derive(Debug, Clone)]
pub struct Value {
    pub subgroup: String,
    pub ac: u32,
    pub dc: u32,
}

/// Reads every setting of a scheme into a map keyed by setting GUID.
///
/// Both the subgroup and the setting lines carry a trailing ` (Name)` after
/// the GUID, so the GUID is extracted with [`extract_guid`]. Matching the whole
/// line instead is what used to store keys like
/// `bc5038f7-… (CPU maximum state)`, which never equalled a bare GUID lookup -
/// every setting read as "not present".
pub fn read_all(scheme: &str) -> HashMap<String, Value> {
    let Ok(txt) = powercfg(&["/qh", scheme]) else {
        return HashMap::new();
    };
    parse_qh(&txt)
}

/// Pure parser behind [`read_all`], kept separate so tests can feed it real
/// `powercfg` output without running anything.
pub(crate) fn parse_qh(txt: &str) -> HashMap<String, Value> {
    let mut map = HashMap::new();
    let mut sub_guid = String::new();
    let mut set_guid = String::new();
    let mut ac: Option<u32> = None;

    for line in txt.lines() {
        let l = line.trim();
        if l.starts_with("Subgroup GUID:") {
            sub_guid = extract_guid(l).unwrap_or_default();
            set_guid.clear();
            ac = None;
        } else if l.starts_with("Power Setting GUID:") {
            set_guid = extract_guid(l).unwrap_or_default();
            ac = None;
        } else if let Some(v) = hex_after(l, "Current AC Power Setting Index:") {
            ac = Some(v);
        } else if let Some(v) = hex_after(l, "Current DC Power Setting Index:") {
            if let Some(a) = ac {
                if !set_guid.is_empty() {
                    map.insert(
                        set_guid.clone(),
                        Value {
                            subgroup: sub_guid.clone(),
                            ac: a,
                            dc: v,
                        },
                    );
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
