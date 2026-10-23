//! The two optimisation modes and the exact Windows power-setting GUIDs.
//!
//! Every GUID in this file was verified against this specific machine with
//! `powercfg /qh` before it was written down. They are reproduced verbatim so
//! the Rust app sets precisely the same values the PowerShell prototype did.
//!
//! Note on how values are written: a power scheme stores one AC column and one
//! DC column. The app resolves the parameters for the *current* power source
//! and writes them into **both** columns, exactly like the prototype. When the
//! charger is pulled, the background watcher simply re-applies with the
//! battery parameters.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Quiet,
    Performance,
    Default,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Quiet => "Quiet",
            Mode::Performance => "Performance",
            Mode::Default => "Default",
        }
    }
    pub fn other(self) -> Mode {
        match self {
            Mode::Quiet => Mode::Performance,
            Mode::Performance => Mode::Quiet,
            Mode::Default => Mode::Default,
        }
    }
    pub fn tagline(self) -> &'static str {
        match self {
            Mode::Quiet => "Cool, silent and long-lasting",
            Mode::Performance => "Full power, full speed",
            Mode::Default => "Factory values, same on battery and mains",
        }
    }
    pub fn blurb(self) -> &'static str {
        match self {
            Mode::Quiet => "Caps the CPU, biases Windows toward the integrated GPU, throttles the \
                          RTX 3050 Ti and keeps the fans quiet. Ideal for battery life, heat and \
                          noise.",
            Mode::Performance => "Unlocks the CPU boost, drops every power saving, lifts the GPU \
                          watt limit to the hardware maximum and runs the fans on turbo so the \
                          machine never thermally throttles.",
            Mode::Default => "Restores the factory power values from the initial backup. \
                          Stays the same whether you are on battery or mains - no automatic \
                          switching.",
        }
    }
    pub fn parse(s: &str) -> Mode {
        if s.eq_ignore_ascii_case("performance") {
            Mode::Performance
        } else if s.eq_ignore_ascii_case("default") {
            Mode::Default
        } else {
            Mode::Quiet
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanMode {
    Automatic,
    Turbo,
    Maximum,
    Custom,
    Iqst,
}

impl FanMode {
    /// AutomationId of the matching radio button in the vendor app.
    pub fn control_id(self) -> &'static str {
        match self {
            FanMode::Automatic => "RB_FAN_auto",
            FanMode::Turbo => "RB_FAN_Turbo",
            FanMode::Maximum => "RB_FAN_max",
            FanMode::Custom => "RB_FAN_custom",
            FanMode::Iqst => "RB_FAN_IQST",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            FanMode::Automatic => "Automatic",
            FanMode::Turbo => "Turbo",
            FanMode::Maximum => "Maximum",
            FanMode::Custom => "Custom",
            FanMode::Iqst => "IQST",
        }
    }
}

/// Every tunable the two modes differ in.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    pub cpu_max: u32,
    pub cpu_min: u32,
    pub epp: u32,
    pub boost: u32,
    pub inc_thresh: u32,
    pub dec_thresh: u32,
    pub cooling: u32,
    pub igpu: u32,
    pub gpu_pref: u32,
    pub wifi: u32,
    pub standby: u32,
    pub hibernate: u32,
    pub display: u32,
    pub dim: u32,
    pub brightness: u32,
    pub gpu_watts: u32,
    pub fan_mode: FanMode,
    pub fan_offset: u32,
    pub noiseless: bool,
}

impl Params {
    pub fn for_mode(mode: Mode, on_ac: bool) -> Params {
        match (mode, on_ac) {
            // ---------------------------------------------------------- Quiet
            (Mode::Quiet, true) => Params {
                cpu_max: 70,
                cpu_min: 5,
                epp: 50,
                boost: 0,
                inc_thresh: 25,
                dec_thresh: 25,
                cooling: 0, // passive: throttle rather than spin up
                igpu: 0,    // maximum battery life
                gpu_pref: 1,
                wifi: 3,
                standby: 600,
                hibernate: 1200,
                display: 420,
                dim: 1,
                brightness: 70,
                gpu_watts: 25,
                fan_mode: FanMode::Automatic,
                fan_offset: 0,
                noiseless: true,
            },
            (Mode::Quiet, false) => Params {
                cpu_max: 60,
                cpu_min: 5,
                epp: 55,
                boost: 0,
                inc_thresh: 30,
                dec_thresh: 30,
                cooling: 0,
                igpu: 0,
                gpu_pref: 1,
                wifi: 3,
                standby: 420,
                hibernate: 900,
                display: 300,
                dim: 1,
                brightness: 70,
                gpu_watts: 15,
                fan_mode: FanMode::Automatic,
                fan_offset: 0,
                noiseless: true,
            },
            // -------------------------------------------------- Performance
            (Mode::Performance, true) => Params {
                cpu_max: 100,
                cpu_min: 5,
                epp: 0,
                boost: 2,
                inc_thresh: 5,
                dec_thresh: 5,
                cooling: 1, // active: cool immediately, never throttle
                igpu: 2,    // maximum performance
                gpu_pref: 0,
                wifi: 0,
                standby: 0,
                hibernate: 0,
                display: 0,
                dim: 0,
                brightness: 100,
                gpu_watts: 85, // device maximum
                fan_mode: FanMode::Turbo,
                fan_offset: 100,
                noiseless: false,
            },
            (Mode::Performance, false) => Params {
                cpu_max: 100,
                cpu_min: 5,
                epp: 0,
                boost: 2,
                inc_thresh: 5,
                dec_thresh: 5,
                cooling: 1,
                igpu: 2,
                gpu_pref: 0,
                wifi: 0,
                standby: 0,
                hibernate: 0,
                display: 0,
                dim: 0,
                brightness: 100,
                gpu_watts: 60,
                fan_mode: FanMode::Turbo,
                fan_offset: 100,
                noiseless: false,
            },
            // -------------------------------------------------- Default
            // Factory values from the initial backup: same on AC and DC,
            // no automatic switching between power sources.
            (Mode::Default, _) => Params {
                cpu_max: 100,
                cpu_min: 5,
                epp: 50,
                boost: 0,
                inc_thresh: 25,
                dec_thresh: 25,
                cooling: 1,
                igpu: 2,
                gpu_pref: 0,
                wifi: 0,
                standby: 600,
                hibernate: 1200,
                display: 600,
                dim: 1,
                brightness: 100,
                gpu_watts: 0, // 0 means "let Windows decide" / no limit
                fan_mode: FanMode::Automatic,
                fan_offset: 0,
                noiseless: false,
            },
        }
    }
}

/// One concrete `powercfg` write.
#[derive(Debug, Clone, Copy)]
pub struct PlanEntry {
    pub alias: &'static str,
    pub label: &'static str,
    pub subgroup: &'static str,
    pub setting: &'static str,
    pub ac: u32,
    pub dc: u32,
}

/// Subgroup / setting GUID pairs, in the order the UI shows them.
///
/// `powercfg /setacvalueindex` rejects subgroup *aliases* on this machine, so
/// full GUIDs are mandatory - only setting names work as aliases.
pub const DEFS: &[(&str, &str, &str, &str)] = &[
    (
        "PROCTHROTTLEMAX",
        "CPU maximum state",
        "54533251-82be-4824-96c1-47b60b740d00",
        "bc5038f7-23e0-4960-96da-33abaf5935ec",
    ),
    (
        "PROCTHROTTLEMIN",
        "CPU minimum state",
        "54533251-82be-4824-96c1-47b60b740d00",
        "893dee8e-2bef-41e0-89c6-b55d0929964c",
    ),
    (
        "PERFEPP",
        "Energy efficiency preference",
        "54533251-82be-4824-96c1-47b60b740d00",
        "36687f9e-e3a5-4dbf-b1dc-15eb381c6863",
    ),
    (
        "PERFBOOSTMODE",
        "Turbo boost",
        "54533251-82be-4824-96c1-47b60b740d00",
        "be337238-0d82-4146-a960-4f3749d470c7",
    ),
    (
        "PERFINCTHRESHOLD",
        "Performance increase threshold",
        "54533251-82be-4824-96c1-47b60b740d00",
        "06cadf0e-64ed-448a-8927-ce7bf90eb35d",
    ),
    (
        "PERFDECTHRESHOLD",
        "Performance decrease threshold",
        "54533251-82be-4824-96c1-47b60b740d00",
        "12a0ab44-fe28-4fa9-b3bd-4b64f44960a6",
    ),
    (
        "SYSCOOLPOL",
        "System cooling policy",
        "54533251-82be-4824-96c1-47b60b740d00",
        "94d3a615-a899-4ac5-ae2b-e4d8f634367f",
    ),
    (
        "IGPU_PLAN",
        "Intel graphics power plan",
        "44f3beca-a7c0-460e-9df2-bb8b99e0cba6",
        "3619c3f2-afb2-4afc-b0e9-e7fef372de36",
    ),
    (
        "GPU_PREF",
        "GPU preference (integrated first)",
        "5fb4938d-1ee8-4b0f-9a3c-5036b0ab995c",
        "dd848b2a-8a5d-4451-9ae2-39cd41658f6c",
    ),
    (
        "WLAN_SAVE",
        "Wireless power saving",
        "19cbb8fa-5279-450e-9fac-8a3d5fedd0c1",
        "12bbebe6-58d6-4636-95bb-3217ef867c1a",
    ),
    (
        "VIDEOIDLE",
        "Turn off display",
        "7516b95f-f776-4464-8c53-06167f40cc99",
        "3c0bc021-c8a8-4e07-a973-6b14cbcb2b7e",
    ),
    (
        "VIDEODIM",
        "Dim display",
        "7516b95f-f776-4464-8c53-06167f40cc99",
        "17aaa29b-8b43-4b94-aafe-35f64daaf1ee",
    ),
    (
        "DISPLAYBRIGHT",
        "Display brightness",
        "7516b95f-f776-4464-8c53-06167f40cc99",
        "aded5e82-b909-4619-9949-f5d71dac0bcb",
    ),
    (
        "STANDBYIDLE",
        "Sleep timeout",
        "238c9fa8-0aad-41ed-83f4-97be242c8f20",
        "29f6c1db-86da-48c5-9fdb-f2b67b1f44da",
    ),
    (
        "HIBERNATEIDLE",
        "Hibernate timeout",
        "238c9fa8-0aad-41ed-83f4-97be242c8f20",
        "9d7815a6-7ee4-497e-8888-515a05f02364",
    ),
    (
        "RTCWAKE",
        "Wake on timer",
        "238c9fa8-0aad-41ed-83f4-97be242c8f20",
        "bd3b718a-0680-4d9d-8ab2-e1d2b4ac806d",
    ),
    (
        "LIDACTION",
        "Lid close action",
        "4f971e89-eebd-4455-a8de-9e59040e7347",
        "5ca83367-6e45-459f-a27b-476b1d01c936",
    ),
];

/// Builds the concrete list of writes for a mode under the current power state.
pub fn plan(mode: Mode, on_ac: bool) -> (Params, Vec<PlanEntry>) {
    let p = Params::for_mode(mode, on_ac);

    let entries = DEFS
        .iter()
        .map(|(alias, label, sub, set)| {
            // Two settings keep an explicit AC/DC pair; everything else mirrors
            // the single resolved value into both columns.
            let (ac, dc) = match *alias {
                "RTCWAKE" => (2, 0),
                "LIDACTION" => (1, 1),
                "PROCTHROTTLEMAX" => (p.cpu_max, p.cpu_max),
                "PROCTHROTTLEMIN" => (p.cpu_min, p.cpu_min),
                "PERFEPP" => (p.epp, p.epp),
                "PERFBOOSTMODE" => (p.boost, p.boost),
                "PERFINCTHRESHOLD" => (p.inc_thresh, p.inc_thresh),
                "PERFDECTHRESHOLD" => (p.dec_thresh, p.dec_thresh),
                "SYSCOOLPOL" => (p.cooling, p.cooling),
                "IGPU_PLAN" => (p.igpu, p.igpu),
                "GPU_PREF" => (p.gpu_pref, p.gpu_pref),
                "WLAN_SAVE" => (p.wifi, p.wifi),
                "VIDEOIDLE" => (p.display, p.display),
                "VIDEODIM" => (p.dim, p.dim),
                "DISPLAYBRIGHT" => (p.brightness, p.brightness),
                "STANDBYIDLE" => (p.standby, p.standby),
                "HIBERNATEIDLE" => (p.hibernate, p.hibernate),
                _ => (0, 0),
            };
            PlanEntry {
                alias,
                label,
                subgroup: sub,
                setting: set,
                ac,
                dc,
            }
        })
        .collect();

    (p, entries)
}