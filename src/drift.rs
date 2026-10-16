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

    /// Strict variant for the worker's idempotence gate: every setting must
    /// equal the values for the *current* power source, not merely either
    /// column.
    ///
    /// The tolerant `fully_applied` is what the UI shows, so moving to battery
    /// does not nag about mains values still being in the scheme. But the
    /// worker must not use it to skip writes: after unplugging, the mains
    /// values read as "consistent" through the tolerance, and an auto-switch
    /// built on that gate would announce the mode while never writing the
    /// battery values. This gate is what actually flips the numbers.
    pub fn fully_applied_strict(&self) -> bool {
        self.managed
            && self.checks.iter().all(|c| match c.actual {
                Some(v) => v == c.expected,
                None => false,
            })
    }

    /// Which power source's numbers the scheme actually holds: the current
    /// one, the other one, or a mix. Used to annotate the headline, so
    /// "matches Quiet" also says *which* Quiet - mains values or battery
    /// values. A stale scheme after unplugging reads as the other source
    /// until the worker rewrites it.
    pub fn values_source(&self) -> &'static str {
        let mut current = 0;
        let mut other = 0;
        for c in &self.checks {
            // Rows where both columns agree (fixed pairs like the wake timers,
            // the refresh rate) carry no source information and abstain.
            if c.expected == c.alternative {
                continue;
            }
            match c.actual {
                Some(v) if v == c.expected => current += 1,
                Some(v) if v == c.alternative => other += 1,
                _ => return "mixed values",
            }
        }
        if current > 0 && other == 0 {
            if self.on_ac {
                "mains values"
            } else {
                "battery values"
            }
        } else if other > 0 && current == 0 {
            if self.on_ac {
                "battery values"
            } else {
                "mains values"
            }
        } else {
            "mixed values"
        }
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
///
/// `check_display` adds the refresh rate as one more row. It is a separate
/// flag (rather than always on) so disabling display control in Settings also
/// silences it instead of nagging forever about a value the app was told not
/// to touch.
pub fn check(mode: Mode, on_ac: bool, check_display: bool) -> DriftReport {
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

    let mut checks: Vec<SettingCheck> = now
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

    // The refresh rate is not a power setting, so it cannot come from the
    // plan rows above - but the consistency check would be lying by omission
    // without it. A mode has one rate regardless of power source, so expected
    // and alternative are the same pair. Skipped when the target cannot be
    // discovered (headless CI, unreachable display) rather than reported.
    if check_display {
        if let Some(want) = crate::display::hz_for_mode(mode) {
            let actual = crate::display::current_hz().map(|h| (h, h));
            let expected = (want, want);
            checks.push(SettingCheck {
                label: "Refresh rate",
                expected,
                alternative: expected,
                actual,
                consistent: actual == Some(expected),
            });
        }
    }

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