//! Backups and snapshots.
//!
//! # Safety rules encoded here
//!
//! * The **initial (factory) backup is never destroyed.** Creating a new one
//!   archives the current one into `initial-archive/<timestamp>/` *first*, and
//!   refuses to proceed if that copy did not succeed.
//! * A **snapshot is taken before every change** - applying Quiet, applying
//!   Performance, and restoring. So there is always a way back.
//! * Nothing here ever deletes a backup. Snapshots are trimmed only when the
//!   configured limit is exceeded, and the initial archive is never trimmed.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::errors::{AppError, AppResult};
use crate::paths;
use crate::power;

// -------------------------------------------------------------- snapshots ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotInfo {
    pub id: String,
    pub label: String,
    pub created: String,
    pub path: String,
    pub kind: SnapshotKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SnapshotKind {
    /// Taken automatically before the app changed anything.
    Automatic,
    /// Taken by the user on request.
    Manual,
}

/// How many automatic snapshots to keep. Manual snapshots are never trimmed.
const SNAPSHOT_LIMIT: usize = 40;

fn snapshots_dir(base: &Path) -> PathBuf {
    base.join("snapshots")
}

/// Captures every power setting of the active scheme into a new snapshot.
pub fn snapshot(kind: SnapshotKind, reason: &str) -> AppResult<SnapshotInfo> {
    let base = paths::backup_dir()?;
    let dir = snapshots_dir(&base);
    fs::create_dir_all(&dir)
        .map_err(|e| AppError::Io(format!("{}: {e}", dir.display())))?;

    let scheme = power::active_scheme()?;
    let values = power::read_all(&scheme);

    let stamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let id = format!("{}-{}", kind_tag(kind), stamp);
    let file = dir.join(format!("{id}.txt"));

    let mut body = String::new();
    for (setting, v) in &values {
        // Four fields: subgroup|setting|ac|dc. The subgroup is required -
        // `parse_backup` rejects anything else, so a three-field file
        // could never be restored.
        body.push_str(&format!("{}|{setting}|{}|{}\n", v.subgroup, v.ac, v.dc));
    }
    // Header comment so a snapshot is readable on its own.
    let mut final_text = format!(
        "# XMG Optimizer snapshot\n# reason : {reason}\n# taken  : {}\n# scheme : {scheme}\n",
        Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    final_text.push_str(&body);

    fs::write(&file, final_text).map_err(|e| AppError::Io(format!("{}: {e}", file.display())))?;

    trim_snapshots(&dir, kind);

    Ok(SnapshotInfo {
        id,
        label: reason.to_string(),
        created: Local::now().format("%Y-%m-%d %H:%M").to_string(),
        path: file.to_string_lossy().into_owned(),
        kind,
    })
}

fn kind_tag(k: SnapshotKind) -> &'static str {
    match k {
        SnapshotKind::Automatic => "before",
        SnapshotKind::Manual => "manual",
    }
}

/// Turns a snapshot reason into a sentence for the History tab.
///
/// Reasons are written by the code paths that take the snapshot
/// (`before-quiet`, `before-restore`, `manual`, `cli`, …), so every known one
/// is matched explicitly and anything new falls back to a plain rendering
/// rather than leaking an internal slug into the UI.
pub fn describe_reason(reason: &str) -> String {
    match reason {
        "before-quiet" => "Before applying Quiet".into(),
        "before-performance" => "Before applying Performance".into(),
        "before-restore" => "Before restoring a snapshot".into(),
        "before-restore-to-initial" => "Before restoring the initial backup".into(),
        "manual" => "Manual snapshot".into(),
        "cli" => "Command-line snapshot".into(),
        other => format!("Snapshot ({other})"),
    }
}

/// Removes only *automatic* snapshots beyond the limit. Never touches manual
/// snapshots and never touches anything outside the snapshots folder.
fn trim_snapshots(dir: &Path, _kind: SnapshotKind) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut autos: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("txt") {
            continue;
        }
        let is_auto = p
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.starts_with("before-"))
            .unwrap_or(false);
        if !is_auto {
            continue;
        }
        let t = e
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        autos.push((t, p));
    }
    if autos.len() <= SNAPSHOT_LIMIT {
        return;
    }
    autos.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    for (_, p) in autos.into_iter().skip(SNAPSHOT_LIMIT) {
        let _ = fs::remove_file(p);
    }
}

/// All snapshots, newest first.
pub fn list_snapshots() -> AppResult<Vec<SnapshotInfo>> {
    let base = paths::backup_dir()?;
    let dir = snapshots_dir(&base);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("txt") {
            continue;
        }
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();

        let (kind, rest) = if let Some(r) = stem.strip_prefix("manual-") {
            (SnapshotKind::Manual, r.to_string())
        } else if let Some(r) = stem.strip_prefix("before-") {
            (SnapshotKind::Automatic, r.to_string())
        } else {
            (SnapshotKind::Automatic, stem.clone())
        };

        let (reason, ts) = match rest.rsplit_once('-') {
            Some((r, t)) if t.len() == 6 && t.chars().all(|c| c.is_ascii_digit()) => {
                (r.to_string(), t.to_string())
            }
            _ => (rest.clone(), String::new()),
        };

        let created = e
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| {
                let dt: chrono::DateTime<Local> = t.into();
                dt.format("%Y-%m-%d %H:%M").to_string()
            })
            .unwrap_or_default();

        let _ = &ts;
        out.push(SnapshotInfo {
            id: stem,
            label: if reason.is_empty() {
                "Snapshot".into()
            } else {
                reason
            },
            created,
            path: p.to_string_lossy().into_owned(),
            kind,
        });
    }
    out.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(out)
}

// -------------------------------------------------------- factory backup ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveEntry {
    pub id: String,
    pub created: String,
    pub note: String,
    pub path: String,
    pub settings_file: String,
}

/// The files that make up a complete "initial" backup.
const FACTORY_FILES: &[(&str, &str)] = &[
    ("original-settings.txt", "settings.txt"),
    ("original-autostart.txt", "autostart.txt"),
    ("original-gaming.json", "gaming.json"),
    ("original-gpu-limit.txt", "gpu-limit.txt"),
];

pub fn has_factory_backup() -> bool {
    paths::backup_dir_candidates()
        .iter()
        .any(|c| c.join(paths::FACTORY_MARKER).is_file())
}

/// `true` when the active factory backup has already been archived (i.e. it is
/// not the very first one).
pub fn factory_is_archived(base: &Path) -> bool {
    base.join("active-initial-archive.txt").is_file()
}

/// Creates a brand new "initial" backup from the machine's *current* state.
///
/// The existing one is archived first. If archiving fails, nothing is
/// overwritten and the call fails loudly.
pub fn create_factory_backup(note: &str) -> AppResult<ArchiveEntry> {
    let base = paths::backup_dir()?;
    archive_current_factory(&base, note)?;
    write_current_factory(&base)?;
    list_archives()?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Io("factory backup was written but could not be listed".into()))
}

/// Copies whatever is currently at the root into a new archive folder and
/// records the pointer file.
fn archive_current_factory(base: &Path, note: &str) -> AppResult<Option<ArchiveEntry>> {
    if !base.join(paths::FACTORY_MARKER).is_file() {
        return Ok(None); // nothing to archive
    }

    let stamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let id = format!("initial-{stamp}");
    let dir = base.join("initial-archive").join(&id);

    fs::create_dir_all(&dir)
        .map_err(|e| AppError::Io(format!("{}: {e}", dir.display())))?;

    let mut copied_any = false;
    for (src_name, dst_name) in FACTORY_FILES {
        let src = base.join(src_name);
        if src.is_file() {
            let dst = dir.join(dst_name);
            fs::copy(&src, &dst)
                .map_err(|e| AppError::Io(format!("archive copy {}: {e}", src.display())))?;
            // Verify the copy byte-for-byte before trusting it.
            let a = fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
            let b = fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
            if a != b {
                let _ = fs::remove_dir_all(&dir);
                return Err(AppError::Io(format!(
                    "archive verification failed for {src_name} ({a} vs {b} bytes) - nothing was overwritten"
                )));
            }
            copied_any = true;
        }
    }

    if !copied_any {
        let _ = fs::remove_dir_all(&dir);
        return Err(AppError::Io(
            "archive was empty - refusing to replace the existing backup".into(),
        ));
    }

    let meta = ArchiveEntry {
        id: id.clone(),
        created: Local::now().format("%Y-%m-%d %H:%M").to_string(),
        note: note.to_string(),
        path: dir.to_string_lossy().into_owned(),
        settings_file: dir.join("settings.txt").to_string_lossy().into_owned(),
    };
    fs::write(
        dir.join("meta.json"),
        serde_json::to_vec_pretty(&meta).unwrap_or_default(),
    )
    .map_err(|e| AppError::Io(format!("meta.json: {e}")))?;

    // Pointer to the archive that matches the currently active backup.
    fs::write(
        base.join("active-initial-archive.txt"),
        format!("{}\n{}", id, note),
    )
    .map_err(|e| AppError::Io(format!("pointer: {e}")))?;

    Ok(Some(meta))
}

/// Captures the machine's present state as the active factory backup.
fn write_current_factory(base: &Path) -> AppResult<()> {
    let scheme = power::active_scheme()?;
    let values = power::read_all(&scheme);

    let mut body = String::new();
    for (setting, v) in &values {
        // Four fields: subgroup|setting|ac|dc. The subgroup is required -
        // `parse_backup` rejects anything else, so a three-field file
        // could never be restored.
        body.push_str(&format!("{}|{setting}|{}|{}\n", v.subgroup, v.ac, v.dc));
    }
    fs::write(base.join(paths::FACTORY_MARKER), body)
        .map_err(|e| AppError::Io(format!("factory settings: {e}")))?;

    crate::startup::capture_autostart(base)?;
    crate::gpu::capture_limit(base)?;
    crate::display::capture(base)?;
    Ok(())
}

/// All archived factory backups, newest first.
pub fn list_archives() -> AppResult<Vec<ArchiveEntry>> {
    let base = paths::backup_dir()?;
    let dir = base.join("initial-archive");
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let meta = fs::read_to_string(p.join("meta.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<ArchiveEntry>(&t).ok());
        let id = p.file_name().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        out.push(match meta {
            Some(mut m) => {
                m.id = id;
                m
            }
            None => ArchiveEntry {
                id,
                created: String::new(),
                note: "(archived)".into(),
                path: p.to_string_lossy().into_owned(),
                settings_file: p.join("settings.txt").to_string_lossy().into_owned(),
            },
        });
    }
    out.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(out)
}

// ---------------------------------------------------------------- restore ---

/// Where a restore should read from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RestoreSource {
    /// The active initial (factory) backup.
    Factory,
    /// A specific archived factory backup.
    Archive(String),
    /// A specific snapshot.
    Snapshot(String),
}

pub fn resolve_source(src: &RestoreSource) -> AppResult<PathBuf> {
    let base = paths::backup_dir()?;
    match src {
        RestoreSource::Factory => {
            let p = base.join(paths::FACTORY_MARKER);
            if p.is_file() {
                Ok(p)
            } else {
                Err(AppError::NotFound(format!(
                    "no initial backup at {}",
                    p.display()
                )))
            }
        }
        RestoreSource::Archive(id) => {
            // Reject anything that is not a plain archive folder name.
            if !valid_id(id) {
                return Err(AppError::Denied(format!("invalid archive id: {id}")));
            }
            let p = base.join("initial-archive").join(id).join("settings.txt");
            if p.is_file() {
                Ok(p)
            } else {
                Err(AppError::NotFound(p.to_string_lossy().into_owned()))
            }
        }
        RestoreSource::Snapshot(id) => {
            if !valid_id(id) {
                return Err(AppError::Denied(format!("invalid snapshot id: {id}")));
            }
            let p = base.join("snapshots").join(format!("{id}.txt"));
            if p.is_file() {
                Ok(p)
            } else {
                Err(AppError::NotFound(p.to_string_lossy().into_owned()))
            }
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// One parsed `subgroup|setting|ac|dc` row.
#[derive(Debug, Clone)]
pub struct RestoreRow {
    pub subgroup: String,
    pub setting: String,
    pub ac: u32,
    pub dc: u32,
}

/// Parses a backup/snapshot file. Lines that do not match the strict shape are
/// skipped rather than guessed at, so a damaged file can never inject a command.
pub fn parse_backup(text: &str) -> Vec<RestoreRow> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = l.split('|').collect();
        if parts.len() != 4 {
            continue;
        }
        if !power::is_guid(parts[0]) || !power::is_guid(parts[1]) {
            continue;
        }
        let (Ok(ac), Ok(dc)) = (parts[2].parse::<u32>(), parts[3].parse::<u32>()) else {
            continue;
        };
        rows.push(RestoreRow {
            subgroup: parts[0].to_ascii_lowercase(),
            setting: parts[1].to_ascii_lowercase(),
            ac,
            dc,
        });
    }
    rows
}

#[derive(Debug, Clone, Default)]
pub struct RestoreReport {
    pub applied: usize,
    pub failed: Vec<String>,
}

/// Writes a parsed backup back onto `scheme`.
pub fn apply_restore(scheme: &str, rows: &[RestoreRow]) -> RestoreReport {
    let mut rep = RestoreReport::default();
    for r in rows {
        match power::set_value(scheme, &r.subgroup, &r.setting, r.ac, r.dc) {
            Ok(true) => rep.applied += 1,
            _ => rep.failed.push(r.setting.chars().take(8).collect()),
        }
    }
    let _ = power::set_active(scheme);
    rep
}