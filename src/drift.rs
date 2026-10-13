//! Drift detection.
//!
//! Compares, setting by setting, what the selected mode *should* produce
//! against what the active power scheme actually contains, and reports the
//! difference. The user then decides: re-apply, or leave the machine as it is.
//!
//! ## One subtlety worth knowing about
//!
//! A scheme stores one AC column and one DC column, and this app writes the
//! values for the *current* power source into **both** (see [`crate::profile`]).
//! So after a normal apply on mains, the scheme legitimately contains the
//! *mains* numbers even while the machine later moves to battery.
//!
//! Treating that as drift would produce a permanent false alarm. So a setting
//! counts as consistent when it matches the values for the current power source
//! **or** the values for the other one. Only a value matching neither - an
//! outside change, a different tool, or a hand-edited scheme - is reported.

use crate::power;
use crate::profile::{self, Mode};

#[derive(Debug, Clone)]
pub struct SettingCheck {
    pub label: &'static str,
    /// What the mode should produce right now (AC, DC).
    pub expected: (u32, u32),
    /// What the mode would produce on the other power source.
    pub alternative: (u32, u32),
    /// What the active scheme actually holds.
    pub actual: Option<(u32, u32)>,
    pub consistent: bool,
}

#[derive(Debug, Clone)]
pub struct DriftReport {
    pub mode: Mode,
    pub on_ac: bool,
    /// Name of the active power scheme.
    pub scheme_name: String,
    /// `true` when the active scheme is one this app created.
    pub managed: bool,
    pub checks: Vec<SettingCheck>,
}

impl DriftReport {
    pub fn drifted(&self) -> usize {
        self.checks.iter().filter(|c| !c.consistent).count()
    }

    pub fn total(&self) -> usize {
        self.checks.len()
    }

    pub fn is_clean(&self) -> bool {
        self.drifted() == 0
    }

    /// Every setting agrees, *and* the active scheme is one of ours. Only then
    /// does "nothing to do" make sense.
    pub fn fully_applied(&self) -> bool {
        self.is_clean() && self.managed
    }

    pub fn drifted_labels(&self) -> Vec<&'static str> {
        self.checks
            .iter()
            .filter(|c| !c.consistent)
            .map(|c| c.label)
            .collect()
    }
}

/// Scheme-name prefix this app creates.
const SCHEME_PREFIX: &str = "XMG Focus - ";

/// Inspects the active scheme against `mode`.
pub fn check(mode: Mode, on_ac: bool) -> DriftReport {
    let (scheme_guid, scheme_name, managed) = match power::active_scheme() {
        Ok(guid) => {
            let name = power::list_schemes()
                .into_iter()
                .find(|(g, _)| g.eq_ignore_ascii_case(&guid))
                .map(|(_, n)| n)
                .unwrap_or_else(|| "unknown scheme".into());
            (Some(guid), name.clone(), name.starts_with(SCHEME_PREFIX))
        }
        Err(_) => (None, "could not read the active scheme".into(), false),
    };

    let actual_map = match &scheme_guid {
        Some(g) => power::read_all(g),
        None => Default::default(),
    };

    let (_, now) = profile::plan(mode, on_ac);
    let (_, other) = profile::plan(mode, !on_ac);

    let checks = now
        .iter()
        .zip(other.iter())
        .map(|(a, b)| {
            let actual = actual_map
                .get(&a.setting.to_ascii_lowercase())
                .map(|v| (v.ac, v.dc));
            let expected = (a.ac, a.dc);
            let alternative = (b.ac, b.dc);
            let consistent = match actual {
                Some(v) => v == expected || v == alternative,
                None => false,
            };
            SettingCheck {
                label: a.label,
                expected,
                alternative,
                actual,
                consistent,
            }
        })
        .collect();

    DriftReport {
        mode,
        on_ac,
        scheme_name,
        managed,
        checks,
    }
}

/// Formats a value the way the UI shows it.
pub fn fmt_value(v: (u32, u32)) -> String {
    if v.0 == v.1 {
        if v.0 == 0 {
            "off".into()
        } else {
            v.0.to_string()
        }
    } else {
        format!("{} / {}", v.0, v.1)
    }
}