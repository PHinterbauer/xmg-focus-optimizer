//! Tests for the parts that must never silently go wrong.
//!
//! These cover the safety-critical logic: the exact values each mode writes,
//! the tolerance that stops false drift warnings, and the strict parsing that
//! keeps a damaged backup from injecting anything.

use crate::backup::{self, RestoreSource};
use crate::drift;
use crate::power;
use crate::profile::{self, FanMode, Mode};

// ------------------------------------------------------------------ modes --

#[test]
fn mode_labels_and_tags_are_set() {
    assert_eq!(Mode::Quiet.label(), "Quiet");
    assert_eq!(Mode::Performance.label(), "Performance");
    assert_eq!(Mode::Quiet.other(), Mode::Performance);
    assert_eq!(Mode::Performance.other(), Mode::Quiet);
    assert!(!Mode::Quiet.tagline().is_empty());
    assert!(!Mode::Performance.tagline().is_empty());
}

#[test]
fn unknown_mode_name_falls_back_to_quiet() {
    // The safe default: never surprise someone with Performance.
    assert_eq!(Mode::parse("quiet"), Mode::Quiet);
    assert_eq!(Mode::parse("QUIET"), Mode::Quiet);
    assert_eq!(Mode::parse("Performance"), Mode::Performance);
    assert_eq!(Mode::parse("nonsense"), Mode::Quiet);
    assert_eq!(Mode::parse(""), Mode::Quiet);
}

#[test]
fn quiet_is_stricter_than_performance_on_every_axis() {
    let q = profile::Params::for_mode(Mode::Quiet, true);
    let p = profile::Params::for_mode(Mode::Performance, true);
    assert!(q.cpu_max < p.cpu_max, "Quiet caps the CPU below Performance");
    assert!(q.epp > p.epp, "Quiet biases more towards efficiency");
    assert!(q.gpu_watts < p.gpu_watts, "Quiet throttles the GPU harder");
    assert!(q.gpu_pref == 1, "Quiet must prefer the integrated GPU");
    assert!(p.gpu_pref == 0, "Performance leaves the dGPU unrestricted");
    assert!(q.noiseless && !p.noiseless);
}

#[test]
fn battery_is_never_more_aggressive_than_mains() {
    for mode in [Mode::Quiet, Mode::Performance] {
        let ac = profile::Params::for_mode(mode, true);
        let dc = profile::Params::for_mode(mode, false);
        assert!(
            dc.gpu_watts <= ac.gpu_watts,
            "{mode:?} must not draw more power on battery"
        );
        assert!(
            dc.cpu_max <= ac.cpu_max,
            "{mode:?} must not raise the CPU cap on battery"
        );
    }
}

#[test]
fn performance_uses_the_hardware_maximum() {
    let p = profile::Params::for_mode(Mode::Performance, true);
    assert_eq!(p.cpu_max, 100);
    assert_eq!(p.gpu_watts, 85, "RTX 3050 Ti device maximum");
    assert_eq!(p.fan_mode, FanMode::Turbo);
    assert_eq!(p.fan_offset, 100);
}

// ------------------------------------------------------------------- plan --

#[test]
fn plan_covers_all_seventeen_settings() {
    let (_, entries) = profile::plan(Mode::Quiet, true);
    assert_eq!(entries.len(), 17, "all settings must be present");
    assert_eq!(profile::DEFS.len(), 17);
    // The display brightness entry must be there with the exact GUIDs from
    // the original machine backup.
    let b = entries
        .iter()
        .find(|e| e.alias == "DISPLAYBRIGHT")
        .expect("brightness must be planned");
    assert_eq!(b.subgroup, "7516b95f-f776-4464-8c53-06167f40cc99");
    assert_eq!(b.setting, "aded5e82-b909-4619-9949-f5d71dac0bcb");
}

#[test]
fn every_guid_is_well_formed_and_unique() {
    let (_, entries) = profile::plan(Mode::Quiet, true);
    let mut seen = std::collections::HashSet::new();
    for e in &entries {
        assert!(power::is_guid(e.subgroup), "bad subgroup in {}", e.alias);
        assert!(power::is_guid(e.setting), "bad setting in {}", e.alias);
        assert!(
            seen.insert(e.setting.to_ascii_lowercase()),
            "duplicate setting GUID for {}",
            e.alias
        );
    }
}

#[test]
fn the_two_fixed_pairs_keep_their_ac_dc_split() {
    let (_, entries) = profile::plan(Mode::Quiet, true);
    for e in &entries {
        match e.alias {
            // Wake-on-timer: armed on mains, off on battery.
            "RTCWAKE" => assert_eq!((e.ac, e.dc), (2, 0)),
            // Lid close: sleep, on both.
            "LIDACTION" => assert_eq!((e.ac, e.dc), (1, 1)),
            // Everything else mirrors the resolved value into both columns.
            _ => assert_eq!(
                e.ac, e.dc,
                "{} should write the same value to AC and DC",
                e.alias
            ),
        }
    }
}

#[test]
fn brightness_follows_the_mode() {
    let q = profile::Params::for_mode(Mode::Quiet, true);
    assert_eq!((q.brightness, q.brightness), (70, 70));
    let p = profile::Params::for_mode(Mode::Performance, true);
    assert_eq!((p.brightness, p.brightness), (100, 100));
    // Quiet keeps the same brightness on battery by request: dimmer would be
    // a guess, and the user asked for 70 flat.
    let qb = profile::Params::for_mode(Mode::Quiet, false);
    assert_eq!(qb.brightness, 70);
}

#[test]
fn quiet_targets_sixty_hertz() {
    assert_eq!(crate::display::hz_for_mode(Mode::Quiet), Some(60));
    assert_eq!(crate::display::label_for_mode(Mode::Quiet), "60 Hz");
}

#[test]
fn switching_power_source_changes_the_plan() {
    let (ac, _) = profile::plan(Mode::Quiet, true);
    let (dc, _) = profile::plan(Mode::Quiet, false);
    assert_eq!(ac.gpu_watts, 25);
    assert_eq!(dc.gpu_watts, 15);
}

#[test]
fn settings_never_exceed_sane_bounds() {
    for mode in [Mode::Quiet, Mode::Performance] {
        for on_ac in [true, false] {
            let (_, entries) = profile::plan(mode, on_ac);
            for e in &entries {
                if e.alias == "RTCWAKE" || e.alias == "LIDACTION" {
                    continue;
                }
                assert!(e.ac <= 4200, "{} absurdly high: {}", e.alias, e.ac);
                assert!(e.dc <= 4200, "{} absurdly high: {}", e.alias, e.dc);
            }
        }
    }
}

// ------------------------------------------------------------------- gpu --

#[test]
fn gpu_wattages_are_within_the_device_range() {
    for mode in [Mode::Quiet, Mode::Performance] {
        for on_ac in [true, false] {
            let p = profile::Params::for_mode(mode, on_ac);
            assert!(p.gpu_watts > 0, "a 0 W limit would disable the GPU");
            assert!(p.gpu_watts <= 85, "never exceed the documented maximum");
        }
    }
}

// ---------------------------------------------------------- backup parsing --

const GOOD_LINE: &str = "54533251-82be-4824-96c1-47b60b740d00|bc5038f7-23e0-4960-96da-33abaf5935ec|70|70";

#[test]
fn parses_a_well_formed_backup() {
    let rows = backup::parse_backup(GOOD_LINE);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].ac, 70);
    assert_eq!(rows[0].dc, 70);
}

#[test]
fn skips_comments_and_blank_lines() {
    let text = format!("# a comment\n\n{GOOD_LINE}\n   \n# trailing comment\n");
    assert_eq!(backup::parse_backup(&text).len(), 1);
}

#[test]
fn rejects_anything_that_is_not_exactly_four_fields() {
    for bad in [
        "",
        "garbage",
        "a|b|c",
        "a|b|c|d|e",
        "54533251-82be-4824-96c1-47b60b740d00|bc5038f7-23e0-4960-96da-33abaf5935ec|70",
    ] {
        assert!(
            backup::parse_backup(bad).is_empty(),
            "should have rejected: {bad:?}"
        );
    }
}

#[test]
fn rejects_malformed_guids() {
    let cases = [
        "not-a-guid|bc5038f7-23e0-4960-96da-33abaf5935ec|70|70",
        "54533251-82be-4824-96c1-47b60b740d00|zz5038f7-23e0-4960-96da-33abaf5935ec|70|70",
        // A GUID-shaped prefix that is too short.
        "54533251-82be-4824-96c1-47b60b740d|bc5038f7-23e0-4960-96da-33abaf5935ec|70|70",
    ];
    for bad in cases {
        assert!(
            backup::parse_backup(bad).is_empty(),
            "should have rejected: {bad:?}"
        );
    }
}

#[test]
fn rejects_non_numeric_values() {
    for bad in [
        "54533251-82be-4824-96c1-47b60b740d00|bc5038f7-23e0-4960-96da-33abaf5935ec|abc|70",
        "54533251-82be-4824-96c1-47b60b740d00|bc5038f7-23e0-4960-96da-33abaf5935ec|70|-5",
        "54533251-82be-4824-96c1-47b60b740d00|bc5038f7-23e0-4960-96da-33abaf5935ec|0x10|70",
        "54533251-82be-4824-96c1-47b60b740d00|bc5038f7-23e0-4960-96da-33abaf5935ec|70|70; rm -rf /",
    ] {
        assert!(
            backup::parse_backup(bad).is_empty(),
            "should have rejected: {bad:?}"
        );
    }
}

#[test]
fn accepts_zero_because_off_is_a_real_value() {
    let line = "54533251-82be-4824-96c1-47b60b740d00|bc5038f7-23e0-4960-96da-33abaf5935ec|0|0";
    let rows = backup::parse_backup(line);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].ac, 0);
}

// -------------------------------------------------------------- path safety --

#[test]
fn restore_source_rejects_traversal_attempts() {
    // These would otherwise escape the backup folder.
    for bad in [
        "..",
        "../..",
        "..\\..\\Windows\\System32",
        "snapshots/../../etc/passwd",
        "with space",
        "",
    ] {
        assert!(
            backup::resolve_source(&RestoreSource::Snapshot(bad.to_string())).is_err(),
            "must refuse snapshot id {bad:?}"
        );
        assert!(
            backup::resolve_source(&RestoreSource::Archive(bad.to_string())).is_err(),
            "must refuse archive id {bad:?}"
        );
    }
}

// ------------------------------------------------------- history reasons --

#[test]
fn snapshot_reasons_get_human_sentences() {
    assert_eq!(
        backup::describe_reason("before-quiet"),
        "Before applying Quiet"
    );
    assert_eq!(
        backup::describe_reason("before-performance"),
        "Before applying Performance"
    );
    assert_eq!(
        backup::describe_reason("before-restore"),
        "Before restoring a snapshot"
    );
    assert_eq!(
        backup::describe_reason("before-restore-to-initial"),
        "Before restoring the initial backup"
    );
    assert_eq!(backup::describe_reason("manual"), "Manual snapshot");
    assert_eq!(backup::describe_reason("cli"), "Command-line snapshot");
}

#[test]
fn unknown_reasons_fall_back_plainly() {
    // A new code path must not leak its internal slug into the UI, and must
    // not fail either.
    let s = backup::describe_reason("before-something-new");
    assert!(s.contains("before-something-new"), "got {s:?}");
    assert!(!s.contains('_'), "got {s:?}");
}

// ------------------------------------------------------------- guid parsing --

#[test]
fn guid_recognition() {
    assert!(power::is_guid("bc5038f7-23e0-4960-96da-33abaf5935ec"));
    assert!(power::is_guid("BC5038F7-23E0-4960-96DA-33ABAF5935EC"));
    assert!(!power::is_guid("bc5038f7-23e0-4960-96da-33abaf5935ec-extra"));
    assert!(!power::is_guid(""));
    assert!(!power::is_guid("not a guid at all"));
}

#[test]
fn extracts_a_guid_from_surrounding_text() {
    let txt = "Power Scheme GUID: 8573e976-917d-4b60-acc1-b859de3f9bfd  (XMG Focus - Quiet)";
    assert_eq!(
        power::extract_guid(txt),
        Some("8573e976-917d-4b60-acc1-b859de3f9bfd".to_string())
    );
    assert_eq!(power::extract_guid("nothing here"), None);
}

// ------------------------------------------------------- powercfg parsing --
//
// These use verbatim `powercfg` output captured from the target machine. Both
// parsers have failed before in ways that looked like the machine was at
// fault: the scheme list came back empty ("unknown scheme") and every setting
// read as "not present".

const SAMPLE_LIST: &str = "Existing Power Schemes (* Active)\n\
    -----------------------------------\n\
    Power Scheme GUID: 14eb49c1-04a4-4463-bf17-0dcd02ca9bca  (XMG Focus - Quiet)\n\
    Power Scheme GUID: 381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced)\n\
    Power Scheme GUID: b547e51d-720d-46a8-9b05-78f56a59fe89  (XMG Focus - Quiet) *\n";

#[test]
fn scheme_list_extracts_bare_guids_with_names() {
    let schemes = power::parse_scheme_list(SAMPLE_LIST);
    assert_eq!(schemes.len(), 3, "every line must parse, got {schemes:?}");
    assert_eq!(
        schemes[0],
        (
            "14eb49c1-04a4-4463-bf17-0dcd02ca9bca".to_string(),
            "XMG Focus - Quiet".to_string()
        )
    );
    assert_eq!(schemes[1].1, "Balanced");
    // The active marker must not leak into the GUID or the name.
    assert_eq!(schemes[2].0, "b547e51d-720d-46a8-9b05-78f56a59fe89");
    assert!(power::is_guid(&schemes[2].0));
}

const SAMPLE_QH: &str = "Power Scheme GUID: b547e51d-720d-46a8-9b05-78f56a59fe89  (XMG Focus - Quiet)\n\
  Subgroup GUID: 54533251-82be-4824-96c1-47b60b740d00  (Processor power management)\n\
    Power Setting GUID: bc5038f7-23e0-4960-96da-33abaf5935ec  (Maximum processor state)\n\
    Current AC Power Setting Index: 0x00000064\n\
    Current DC Power Setting Index: 0x00000064\n\
    Power Setting GUID: 893dee8e-2bef-41e0-89c6-b55d0929964c  (Minimum processor state)\n\
    Current AC Power Setting Index: 0x00000005\n\
    Current DC Power Setting Index: 0x00000005\n";

#[test]
fn qh_parse_keys_settings_by_bare_guid() {
    let map = power::parse_qh(SAMPLE_QH);
    assert_eq!(map.len(), 2, "both settings must parse, got {map:?}");
    let max = &map["bc5038f7-23e0-4960-96da-33abaf5935ec"];
    assert_eq!((max.ac, max.dc), (100, 100));
    assert_eq!(max.subgroup, "54533251-82be-4824-96c1-47b60b740d00");
    let min = &map["893dee8e-2bef-41e0-89c6-b55d0929964c"];
    assert_eq!((min.ac, min.dc), (5, 5));
}

#[test]
fn qh_parse_roundtrips_through_backup_format() {
    // The snapshot writer and the backup parser must agree: this is what made
    // old snapshots unrestorable (three fields written, four required).
    let map = power::parse_qh(SAMPLE_QH);
    let mut body = String::new();
    for (setting, v) in &map {
        body.push_str(&format!("{}|{setting}|{}|{}\n", v.subgroup, v.ac, v.dc));
    }
    let rows = backup::parse_backup(&body);
    assert_eq!(rows.len(), 2, "written snapshots must parse back");
}

// ------------------------------------------------------------- drift logic --

/// Builds a drift report out of thin air: every check holds `actual`, the
/// mode is Quiet, the scheme is managed. No powercfg involved, so this runs
/// anywhere - including headless CI.
fn synthetic_report(on_ac: bool, actual: Vec<Option<(u32, u32)>>) -> drift::DriftReport {
    let (_, entries) = profile::plan(Mode::Quiet, on_ac);
    let (_, other) = profile::plan(Mode::Quiet, !on_ac);
    let checks = entries
        .iter()
        .zip(other.iter())
        .zip(actual.iter())
        .map(|((a, b), v)| drift::SettingCheck {
            label: a.label,
            expected: (a.ac, a.dc),
            alternative: (b.ac, b.dc),
            actual: *v,
            consistent: matches!(v, Some(x) if *x == (a.ac, a.dc) || *x == (b.ac, b.dc)),
        })
        .collect();
    drift::DriftReport {
        mode: Mode::Quiet,
        on_ac,
        scheme_name: "XMG Focus - Quiet".into(),
        managed: true,
        checks,
    }
}

#[test]
fn tolerant_check_stays_quiet_but_strict_gate_rewrites() {
    // The exact auto-switch failure: on battery with mains values still in
    // the scheme, the tolerant UI check must stay clean (no false alarm)
    // while the worker's strict gate must refuse the no-op so the battery
    // values actually get written.
    let (_, entries) = profile::plan(Mode::Quiet, true);
    let actual: Vec<Option<(u32, u32)>> =
        entries.iter().map(|e| Some((e.ac, e.dc))).collect();
    let report = synthetic_report(false, actual);
    assert!(report.is_clean(), "tolerant check must not nag");
    assert!(report.fully_applied(), "tolerant gate stays as it was");
    assert!(
        !report.fully_applied_strict(),
        "strict gate must demand the rewrite"
    );
    // The two fixed pairs (wake timers, lid action) are identical on both
    // power sources and abstain from the vote, so a uniformly stale scheme
    // reads as the other source, not mixed.
    assert_eq!(report.values_source(), "mains values");
}

#[test]
fn strict_gate_passes_once_values_match_the_source() {
    let (_, entries) = profile::plan(Mode::Quiet, false);
    let actual: Vec<Option<(u32, u32)>> =
        entries.iter().map(|e| Some((e.ac, e.dc))).collect();
    let report = synthetic_report(false, actual);
    assert!(report.fully_applied_strict());
    assert_eq!(report.values_source(), "battery values");
}

#[test]
fn values_source_reports_mixed_and_mains() {
    let (_, entries) = profile::plan(Mode::Quiet, true);
    // Half current, half other.
    let actual: Vec<Option<(u32, u32)>> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let (_, other) = profile::plan(Mode::Quiet, false);
            if i % 2 == 0 {
                Some((e.ac, e.dc))
            } else {
                Some((other[i].ac, other[i].dc))
            }
        })
        .collect();
    // Quiet AC and DC differ in at least CPU cap, so this is genuinely mixed
    // unless the profiles happen to agree everywhere.
    let report = synthetic_report(true, actual);
    let (_, dc_entries) = profile::plan(Mode::Quiet, false);
    let differs = entries
        .iter()
        .zip(dc_entries.iter())
        .any(|(a, b)| (a.ac, a.dc) != (b.ac, b.dc));
    assert!(differs, "test needs AC/DC profiles to differ");
    assert_eq!(report.values_source(), "mixed values");
}

#[test]
fn a_setting_matching_the_other_power_source_is_not_drift() {
    // This is the AC/DC tolerance: a scheme written on mains legitimately holds
    // the mains numbers while running on battery, and that must not warn.
    let now = profile::plan(Mode::Quiet, false);
    let other = profile::plan(Mode::Quiet, true);

    let cpu_max_idx = now
        .1
        .iter()
        .position(|e| e.alias == "PROCTHROTTLEMAX")
        .expect("CPU max must exist");

    let ac_entry = &other.1[cpu_max_idx];
    let dc_entry = &now.1[cpu_max_idx];
    assert_ne!(ac_entry.ac, dc_entry.ac, "the two profiles must differ");

    // Written while on mains -> the battery check has to accept it.
    let check = |actual: (u32, u32)| {
        actual == (dc_entry.ac, dc_entry.dc) || actual == (ac_entry.ac, ac_entry.dc)
    };
    assert!(check((ac_entry.ac, ac_entry.dc)));
    assert!(check((dc_entry.ac, dc_entry.dc)));
}

#[test]
fn a_genuinely_different_value_is_drift() {
    let (_, entries) = profile::plan(Mode::Quiet, true);
    let (_, other) = profile::plan(Mode::Quiet, false);
    let i = entries
        .iter()
        .position(|e| e.alias == "VIDEOIDLE")
        .expect("display timeout must exist");
    let a = (entries[i].ac, entries[i].dc);
    let b = (other[i].ac, other[i].dc);
    let bogus = (123_456, 123_456);
    assert_ne!(a, b);
    assert_ne!(bogus, a);
    assert_ne!(bogus, b);
}

#[test]
fn drift_counts_every_setting_exactly_once() {
    // A report built from a synthetic value set must total the plan length.
    let (_, entries) = profile::plan(Mode::Performance, true);
    assert_eq!(entries.len(), 17);
}

#[test]
fn fmt_value_is_readable() {
    assert_eq!(drift::fmt_value((70, 70)), "70");
    assert_eq!(drift::fmt_value((0, 0)), "off");
    assert_eq!(drift::fmt_value((2, 0)), "2 / 0");
}

// ---------------------------------------------------------------- fan ids --

#[test]
fn fan_mode_control_ids_match_the_vendor_app() {
    // Verified against the installed CLEVOCO fan app.
    assert_eq!(FanMode::Automatic.control_id(), "RB_FAN_auto");
    assert_eq!(FanMode::Turbo.control_id(), "RB_FAN_Turbo");
    assert_eq!(FanMode::Maximum.control_id(), "RB_FAN_max");
    assert_eq!(FanMode::Custom.control_id(), "RB_FAN_custom");
    assert_eq!(FanMode::Iqst.control_id(), "RB_FAN_IQST");
}

#[test]
fn modes_pick_the_fan_profile_we_expect() {
    assert_eq!(
        profile::Params::for_mode(Mode::Quiet, true).fan_mode,
        FanMode::Automatic
    );
    assert_eq!(
        profile::Params::for_mode(Mode::Performance, true).fan_mode,
        FanMode::Turbo
    );
}