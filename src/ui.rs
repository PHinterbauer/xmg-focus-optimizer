//! The window.
//!
//! egui 0.36 splits the app entry point into two:
//!
//! * [`eframe::App::logic`] runs even while the window is **hidden**, which is
//!   exactly what this app needs - the tray watcher keeps reacting to the
//!   charger being pulled while there is no window on screen.
//! * [`eframe::App::ui`] paints the window when it is visible.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use egui::{Align, CornerRadius, Layout, Margin, RichText, Stroke, Vec2};

use crate::backup::{self, ArchiveEntry, RestoreSource, SnapshotInfo};
use crate::config::{Settings, Theme};
use crate::drift::{self, DriftReport};
use crate::elevate;
use crate::icon;
use crate::jobs::{self, Job, JobResult};
use crate::monitor::MonitorEvent;
use crate::profile::{self, Mode};
use crate::startup;
use crate::sysutil;
use crate::tray::{self, TrayCommand};
use crate::worker;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Dashboard,
    Backups,
    History,
    Settings,
}

impl Tab {
    /// Heading shown at the top of the page.
    fn title(self) -> &'static str {
        match self {
            Tab::Dashboard => "Dashboard",
            Tab::Backups => "Backups",
            Tab::History => "History",
            Tab::Settings => "Settings",
        }
    }

    /// One line under the heading, so the page explains itself before you read
    /// anything.
    fn blurb(self) -> &'static str {
        match self {
            Tab::Dashboard => "Pick a profile, or let the charger decide.",
            Tab::Backups => "Every change is snapshotted before it happens.",
            Tab::History => "What changed and when — inspect, compare, restore.",
            Tab::Settings => "How the app behaves, and how it looks.",
        }
    }
}

/// Which snapshots the History tab lists. Pure view filter; the Backups tab
/// always shows everything.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum HistoryFilter {
    #[default]
    All,
    Auto,
    Manual,
}

impl HistoryFilter {
    pub(crate) fn label(self) -> &'static str {
        match self {
            HistoryFilter::All => "All",
            HistoryFilter::Auto => "Automatic",
            HistoryFilter::Manual => "Manual",
        }
    }

    /// Does a rendered row pass this filter?
    pub(crate) fn matches(self, kind: &str) -> bool {
        match self {
            HistoryFilter::All => true,
            HistoryFilter::Auto => kind == "auto",
            HistoryFilter::Manual => kind == "manual",
        }
    }
}

/// A button the user pressed in the History grid. Resolved after the grid
/// closes, against the id the row was drawn with.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HistoryAction {
    /// Open/close the detail panel for this row.
    Details,
    /// Mark this row as comparison slot A, or clear it if it already is A.
    MarkA,
    /// Mark this row as comparison slot B, or clear it if it already is B.
    MarkB,
    Restore,
    /// Reveal the snapshot file in Explorer.
    Reveal,
}

struct Note {
    text: String,
    kind: NoteKind,
    stamp: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NoteKind {
    Ok,
    Warn,
    Err,
}

/// One rendered history row. Owned (not borrowed from `snapshots`) so the
/// grid closure can call `self.submit(..)` for the Restore buttons without
/// borrowing `self` twice - same pattern as the Backups rows.
#[derive(Clone)]
struct HistoryRow {
    id: String,
    created: String,
    event: String,
    mode: String,
    kind: &'static str,
    /// Absolute snapshot file path, for the "File" button (`worker::reveal`).
    path: String,
    /// Power rows parsed from the snapshot file (0 when unreadable).
    settings: usize,
    /// Settings whose values differ from the next-older snapshot. `None` for
    /// the oldest entry (nothing to compare against) and for unreadable files.
    delta_prev: Option<usize>,
    /// What actually changed vs the previous snapshot: (label, old, new).
    /// Shown in the detail view; empty means identical - or unreadable.
    changes: Vec<(String, String, String)>,
}

/// Setting GUID → human label. One place so the History detail view, the
/// compare panel and the row builder never disagree on names.
///
/// Both key and value are `&'static str` on purpose: `HashMap` is invariant
/// in its parameters, so a `HashMap<&'static str, &'static str>` would *not*
/// coerce to the `HashMap<&str, &str>` a function taking borrowed keys would
/// otherwise ask for.
pub(crate) fn setting_labels() -> std::collections::HashMap<&'static str, &'static str> {
    crate::profile::DEFS
        .iter()
        .map(|(_, label, _, setting)| (*setting, *label))
        .collect()
}

/// Builds render rows from the snapshot list, newest first. Reads each file
/// once; callers cache the result keyed by snapshot id.
fn build_history_rows(snaps: &[SnapshotInfo]) -> Vec<HistoryRow> {
    // Parsed oldest-first so each entry can diff against its predecessor;
    // displayed newest-first as built by the caller order below.
    let parsed: Vec<(SnapshotInfo, Vec<backup::RestoreRow>)> = snaps
        .iter()
        .map(|s| {
            let rows = std::fs::read_to_string(&s.path)
                .ok()
                .map(|t| backup::parse_backup(&t))
                .unwrap_or_default();
            (s.clone(), rows)
        })
        .collect();

    let n = parsed.len();
    // Labels in DEFS order, so the detail view reads like the consistency
    // table instead of hash order.
    let labels = setting_labels();
    parsed
        .into_iter()
        .enumerate()
        .map(|(i, (s, rows))| {
            // Newest first in the UI, so the "previous" snapshot is the next
            // one down the list.
            let older: Vec<backup::RestoreRow> = if i + 1 < n {
                std::fs::read_to_string(&snaps[i + 1].path)
                    .ok()
                    .map(|t| backup::parse_backup(&t))
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let delta_prev = if i + 1 < n {
                Some(backup::count_changed(&older, &rows))
            } else {
                None
            };
            let changes = diff_details(&older, &rows, &labels);
            HistoryRow {
                id: s.id.clone(),
                created: s.created.clone(),
                event: backup::describe_reason(&s.label),
                mode: backup::snapshot_mode(&s.label)
                    .map(|m| m.label().to_string())
                    .unwrap_or_else(|| "—".into()),
                kind: match s.kind {
                    backup::SnapshotKind::Automatic => "auto",
                    backup::SnapshotKind::Manual => "manual",
                },
                path: s.path.clone(),
                settings: rows.len(),
                delta_prev,
                changes,
            }
        })
        .collect()
}

/// Per-setting old→new pairs between two snapshots, in DEFS order. Newly
/// appeared settings read from "—", vanished ones read to "—".
pub(crate) fn diff_details(
    old: &[backup::RestoreRow],
    new: &[backup::RestoreRow],
    labels: &std::collections::HashMap<&'static str, &'static str>,
) -> Vec<(String, String, String)> {
    use std::collections::HashMap;
    let prev: HashMap<&str, (u32, u32)> = old
        .iter()
        .map(|r| (r.setting.as_str(), (r.ac, r.dc)))
        .collect();
    let cur: HashMap<&str, (u32, u32)> = new
        .iter()
        .map(|r| (r.setting.as_str(), (r.ac, r.dc)))
        .collect();
    let mut keys: Vec<&str> = prev.keys().chain(cur.keys()).map(|k| *k).collect();
    keys.sort_unstable();
    keys.dedup();
    let mut out = Vec::new();
    for k in keys {
        let o = prev.get(k);
        let n = cur.get(k);
        if o != n {
            let label = labels.get(k).copied().unwrap_or(k).to_string();
            let fmt = |(ac, dc): (u32, u32)| {
                if ac == dc {
                    format!("{ac}")
                } else {
                    format!("{ac} / {dc}")
                }
            };
            out.push((
                label,
                o.map(|v| fmt(*v)).unwrap_or_else(|| "—".into()),
                n.map(|v| fmt(*v)).unwrap_or_else(|| "—".into()),
            ));
        }
    }
    out
}

pub struct App {
    /// egui is cheap to clone, and holding it lets helpers send viewport
    /// commands before the first frame has been painted.
    ctx: egui::Context,

    settings: Settings,
    tab: Tab,
    on_ac: bool,
    elevated_ready: bool,

    busy: bool,
    /// When the current job was queued, so a job that never reports back can be
    /// given up on instead of locking the UI forever.
    busy_since: Option<std::time::Instant>,
    notes: Vec<Note>,

    /// Live comparison of the selected mode against the machine's real settings.
    drift: Option<DriftReport>,
    /// The user chose to keep the current settings; re-armed on any change.
    drift_acknowledged: bool,
    drift_expanded: bool,

    snapshots: Vec<SnapshotInfo>,
    archives: Vec<ArchiveEntry>,
    backup_dir: PathBuf,
    has_factory: bool,

    confirm_stage: u8,
    confirm_note: String,

    theme_applied: Option<Theme>,
    tray: Option<tray::Tray>,
    autostarted: bool,
    /// Theme value pushed to egui, so it is only pushed when it changes.
    style_pushed: bool,

    tray_rx: Receiver<TrayCommand>,
    monitor_rx: Receiver<MonitorEvent>,
    tx_tray: Sender<TrayCommand>,
    _tx_monitor: Sender<MonitorEvent>,
    stop_watch: Arc<AtomicBool>,
    /// Set when the user asked to quit for real (tray menu or Quit button).
    /// The window-close button only hides to the tray; without this flag the
    /// app would have no way to tell "hide" apart from "quit".
    quit_requested: bool,
    /// Latest temperature readings, refreshed on a background thread. The UI
    /// only ever peeks with `try_lock`, so a slow sensor cannot stall a frame.
    temps: Arc<std::sync::Mutex<crate::temps::Temps>>,
    temps_stop: Arc<AtomicBool>,
    /// Rendered History rows plus the snapshot ids they were built from. The
    /// file parsing behind them runs only when the list itself changes.
    history_ids: Vec<String>,
    history_rows: Vec<HistoryRow>,
    /// Row the user opened in the detail panel, by snapshot id. `None` closes
    /// the panel.
    history_open: Option<String>,
    /// Free-text filter over the history list (matches time, event, mode and
    /// the raw id). Pure view state, never persisted.
    history_search: String,
    /// Which rows the History tab lists. The Backups tab always shows all.
    history_filter: HistoryFilter,
    /// Snapshot ids picked for an arbitrary A/B comparison, in the order they
    /// were marked: first is A, second is B. Two marks make a comparison;
    /// a third mark replaces the older one so the flow never deadlocks.
    history_cmp: Vec<String>,
    /// The A↔B diff, computed once when the second mark lands. Cached so the
    /// panel costs no file IO per frame.
    history_cmp_diff: Option<Vec<(String, String, String)>>,
    /// What a row's buttons asked for this frame, resolved after the grid
    /// closes. Actions act on the id they were drawn with, so a snapshot
    /// trimmed mid-paint cannot make a button hit the wrong row.
    history_act: Option<(String, HistoryAction)>,
    /// Automatic one-time setup runs at most once per process, on a thread,
    /// and only for interactive launches. Without the once-guard a denied UAC
    /// prompt would reappear on every frame.
    setup_attempted: bool,
    setup_rx: Option<Receiver<Result<bool, String>>>,
    /// True for a plain double-click. Login (`--autostart`) and `--tray`
    /// launches stay silent: no automatic UAC prompt from a hidden window.
    interactive: bool,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: egui::Context,
        settings: Settings,
        autostarted: bool,
        tray_rx: Receiver<TrayCommand>,
        monitor_rx: Receiver<MonitorEvent>,
        tx_tray: Sender<TrayCommand>,
        tx_monitor: Sender<MonitorEvent>,
        stop_watch: Arc<AtomicBool>,
        start_hidden: bool,
    ) -> Self {
        let on_ac = crate::power::on_ac_power();
        let (snapshots, archives, backup_dir, has_factory) = Self::state().unwrap_or_default();
        let (temps, temps_stop) = crate::temps::spawn();

        let mut app = App {
            ctx,
            settings,
            tab: Tab::Dashboard,
            on_ac,
            elevated_ready: elevate::task_is_healthy(),
            busy: false,
            busy_since: None,
            notes: Vec::new(),
            drift: None,
            drift_acknowledged: false,
            drift_expanded: false,
            snapshots,
            archives,
            backup_dir,
            has_factory,
            confirm_stage: 0,
            confirm_note: String::new(),
            theme_applied: None,
            tray: None,
            autostarted,
            style_pushed: false,
            tray_rx,
            monitor_rx,
            tx_tray,
            _tx_monitor: tx_monitor,
            stop_watch,
            quit_requested: false,
            temps,
            temps_stop,
            setup_attempted: false,
            setup_rx: None,
            interactive: !autostarted && !start_hidden,
            history_ids: Vec::new(),
            history_rows: Vec::new(),
            history_open: None,
            history_search: String::new(),
            history_filter: HistoryFilter::default(),
            history_cmp: Vec::new(),
            history_cmp_diff: None,
            history_act: None,
        };

        if start_hidden {
            app.hide_to_tray();
        }
        app.refresh();
        // A job that finished while the app was closed still has a result
        // sitting on disk. Show it, rather than leaving the user wondering.
        if let Some(r) = jobs::take_result() {
            log_info!("recovered a job result left over from a previous session");
            app.note("a previous request had finished", NoteKind::Ok);
            app.push_result(r);
        }
        // Reconciles this app's own Run entry with the stored preference, and
        // nothing else. Another program's startup entry is never touched.
        app.reconcile_autostart();
        app.recheck_drift();
        log_info!(
            "UI started: mode={:?} on_ac={} backup_dir={:?} factory={}",
            app.settings.mode,
            app.on_ac,
            app.backup_dir,
            app.has_factory
        );

        // Launched by Windows: silently put the saved mode back in place.
        // Because applying is idempotent this normally changes nothing at all.
        if autostarted {
            app.submit_now(Job::Apply {
                mode: app.settings.mode,
                control_fans: app.settings.control_fans,
                control_gpu: app.settings.control_gpu_limit,
                control_display: app.settings.control_display,
            });
        }
        app
    }

    fn state() -> Result<(Vec<SnapshotInfo>, Vec<ArchiveEntry>, PathBuf, bool), ()> {
        let dir = crate::paths::backup_dir().map_err(|_| ())?;
        let has = dir.join(crate::paths::FACTORY_MARKER).is_file();
        let snaps = backup::list_snapshots().unwrap_or_default();
        let arch = backup::list_archives().unwrap_or_default();
        Ok((snaps, arch, dir, has))
    }

    fn refresh(&mut self) {
        if let Ok((s, a, d, h)) = Self::state() {
            self.snapshots = s;
            self.archives = a;
            self.backup_dir = d;
            self.has_factory = h;
        }
        self.on_ac = crate::power::on_ac_power();
    }

    fn reconcile_autostart(&mut self) {
        if startup::reconcile_autostart(self.settings.start_with_system) {
            self.note(
                if self.settings.start_with_system {
                    "registered to start with Windows"
                } else {
                    "removed from start-up"
                },
                NoteKind::Ok,
            );
        }
    }

    fn recheck_drift(&mut self) {
        let d = drift::check(
            self.settings.mode,
            self.on_ac,
            self.settings.control_display,
        );
        log_debug!(
            "drift check: {}/{} settings consistent, managed={}, scheme={:?}",
            d.total() - d.drifted(),
            d.total(),
            d.managed,
            d.scheme_name
        );
        self.drift = Some(d);
    }

    fn hide_to_tray(&self) {
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    fn show_window(&self) {
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Visible(true));
        self.ctx
            .send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn note(&mut self, text: impl Into<String>, kind: NoteKind) {
        let text = text.into();
        match kind {
            NoteKind::Err => log_error!("note: {text}"),
            NoteKind::Warn => log_warn!("note: {text}"),
            NoteKind::Ok => log_info!("note: {text}"),
        }
        self.notes.insert(
            0,
            Note {
                text: text.into(),
                kind,
                stamp: chrono::Local::now().format("%H:%M:%S").to_string(),
            },
        );
        self.notes.truncate(30);
    }

    fn push_result(&mut self, r: JobResult) {
        let kind = if r.ok { NoteKind::Ok } else { NoteKind::Err };
        self.note(r.summary.clone(), kind);
        for d in &r.details {
            self.note(format!("  {d}"), NoteKind::Ok);
            log_info!("result detail: {d}");
        }
        for w in &r.warnings {
            self.note(format!("  ! {w}"), NoteKind::Warn);
            log_warn!("result warning: {w}");
        }
        self.refresh();
        self.recheck_drift();
    }

    fn submit_now(&mut self, job: Job) {
        // Watch the clock from the moment the job is queued.
        self.busy_since = Some(std::time::Instant::now());
        self.busy = true;
        if !elevate::submit(job) {
            // Dispatch already reported the failure into the result slot.
            self.busy = false;
            self.busy_since = None;
        }
        log_info!("job submitted (busy={})", self.busy);
    }

    fn submit(&mut self, job: Job) {
        if self.busy {
            self.note("still working on the previous request", NoteKind::Warn);
            return;
        }
        self.submit_now(job);
    }

    fn apply_current_mode(&mut self) {
        let s = self.settings.clone();
        // Said out loud in the status bar, so there is a visible record that
        // the click registered before the (slow) worker reports back.
        self.note(format!("Applying {}…", s.mode.label()), NoteKind::Ok);
        self.submit(Job::Apply {
            mode: s.mode,
            control_fans: s.control_fans,
            control_gpu: s.control_gpu_limit,
            control_display: s.control_display,
        });
    }

    fn ensure_tray(&mut self) {
        if self.tray.is_none() {
            self.tray = tray::build(self.settings.mode, self.on_ac, self.is_dark());
        }
    }

    /// The theme actually in effect, resolving "System" against what egui
    /// settled on.
    ///
    /// egui 0.36 removed `Context::style`; `Context::theme` is the replacement
    /// and reports the *resolved* theme, System preference included.
    fn is_dark(&self) -> bool {
        match self.settings.theme {
            Theme::Light => false,
            Theme::Dark => true,
            Theme::System => self.ctx.theme() == egui::Theme::Dark,
        }
    }

    /// Drains the tray menu into the command channel.
    ///
    /// muda menu items are Rc-based and cannot be moved onto another thread, so
    /// the menu is pumped from here - and logic() keeps running even while the
    /// window is hidden, which is exactly when the tray matters most.
    fn poll_tray(&self) {
        let Some(tray) = self.tray.as_ref() else {
            return;
        };
        for cmd in tray.poll() {
            if self.tx_tray.send(cmd).is_err() {
                break;
            }
        }
    }
    /// Drains both channels. Runs whether or not the window is visible.
    fn poll_events(&mut self) {
        // Collected first so the channel borrow ends before the &mut work.
        let setup_done = self
            .setup_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        if let Some(result) = setup_done {
            self.setup_rx = None;
            match result {
                Ok(true) => {
                    self.elevated_ready = true;
                    self.note("background task registered automatically", NoteKind::Ok);
                }
                Ok(false) => self.note(
                    "automatic setup incomplete - retry in Settings → Elevation",
                    NoteKind::Warn,
                ),
                Err(e) => self.note(
                    format!("automatic setup declined or failed ({e}) - charger \
                             auto-switch needs the task; retry in Settings → Elevation"),
                    NoteKind::Warn,
                ),
            }
        }
        while let Ok(cmd) = self.tray_rx.try_recv() {
            match cmd {
                TrayCommand::Show => self.show_window(),
                TrayCommand::Apply(m) => {
                    if self.settings.mode != m {
                        self.settings.mode = m;
                        let _ = self.settings.save();
                        if let Some(tray) = self.tray.as_ref() {
                            tray.set_mode(m);
                        }
                        self.drift_acknowledged = false;
                        self.apply_current_mode();
                    }
                }
                TrayCommand::Restore => {
                    self.tab = Tab::Backups;
                    self.show_window();
                }
                TrayCommand::Quit => {
                    self.stop_watch.store(true, Ordering::Relaxed);
                    self.temps_stop.store(true, Ordering::Relaxed);
                    self.quit_requested = true;
                    self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        while let Ok(ev) = self.monitor_rx.try_recv() {
            match ev {
                MonitorEvent::PowerChanged { on_ac, mode, triggered } => {
                    self.on_ac = on_ac;
                    // The task may have appeared or vanished while running;
                    // re-read it at the exact moment it matters.
                    self.elevated_ready = elevate::task_is_healthy();
                    if triggered {
                        // Say what the switch actually did, not just that it happened:
                        // the headline numbers of the profile now in force.
                        let p = profile::Params::for_mode(mode, on_ac);
                        self.note(
                            format!(
                                "power source changed to {} - re-applied {mode} \
                                 (CPU {}%, GPU {} W, {}, brightness {}%, fans {})",
                                if on_ac { "mains" } else { "battery" },
                                p.cpu_max,
                                p.gpu_watts,
                                crate::display::label_for_mode(mode),
                                p.brightness,
                                p.fan_mode.label()
                            ),
                            NoteKind::Ok,
                        );
                        self.drift_acknowledged = false;
                    }
                    self.recheck_drift();
                }
                MonitorEvent::AutoSwitchSkipped { on_ac, mode: _ } => {
                    self.on_ac = on_ac;
                    self.elevated_ready = elevate::task_is_healthy();
                    // Loud on purpose: without this, the scheme silently keeps
                    // the other source's values while everything else claims
                    // the switch happened.
                    self.note(
                        format!(
                            "charger changed to {} - settings NOT re-applied: \
                             background task missing. Open Settings → Elevation \
                             → Set up now (or run once as administrator).",
                            if on_ac { "mains" } else { "battery" }
                        ),
                        NoteKind::Warn,
                    );
                    self.drift_acknowledged = false;
                    self.recheck_drift();
                }
                MonitorEvent::Error(e) => {
                    log_warn!("power watcher error: {e}");
                    self.note(format!("power watcher: {e}"), NoteKind::Warn);
                }
            }
        }

        if self.busy {
            if let Some(r) = jobs::take_result() {
                let took = self
                    .busy_since
                    .map(|t| t.elapsed().as_secs_f32())
                    .unwrap_or(0.0);
                log_info!("job finished after {:.1}s, ok={}", took, r.ok);
                self.busy = false;
                self.busy_since = None;
                self.push_result(r);
            } else if let Some(since) = self.busy_since {
                // A queued job that never produces a result used to leave
                // `busy` set forever, and submit() then refused every later
                // click with "still working on the previous request" - which
                // looks exactly like every button in the app being broken.
                let waited = since.elapsed().as_secs_f32();
                if waited > JOB_TIMEOUT_SECS {
                    log_error!(
                        "no result {waited:.1}s after submitting; giving up and unlocking the UI"
                    );
                    self.busy = false;
                    self.busy_since = None;
                    self.note(
                        format!(
                            "That request did not finish within {:.0}s and was abandoned. \
                             See the log for why - the buttons are unlocked again.",
                            JOB_TIMEOUT_SECS
                        ),
                        NoteKind::Err,
                    );
                }
            }
        }
    }
}

impl eframe::App for App {
    /// Runs even when the window is hidden - the background watcher lives here.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ensure_tray();
        self.poll_tray();
        self.poll_events();
        // Something changed in the background; make sure a visible window repaints.
        ctx.request_repaint();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // The window-close (X) button hides to the tray instead of quitting.
        // A real quit only happens through the tray menu, a Quit button, or
        // Task Manager. Without this the X button kills the background watcher
        // along with the window.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide_to_tray();
            log_info!("window close intercepted - hiding to tray");
        }

        // Automatic one-time setup, at most once per process and never
        // blocking the UI thread: ensure_setup only prompts when something
        // actually needs doing, and returns immediately when the task is
        // already healthy. Login and tray launches stay silent on purpose -
        // no UAC prompt out of a hidden window.
        if self.interactive && !self.elevated_ready && !self.setup_attempted {
            self.setup_attempted = true;
            let (tx, rx) = std::sync::mpsc::channel();
            self.setup_rx = Some(rx);
            std::thread::Builder::new()
                .name("xmg-auto-setup".into())
                .spawn(move || {
                    let _ = tx.send(elevate::ensure_setup());
                })
                .ok();
            log_info!("automatic setup started in the background");
        }

        // --- theme ---------------------------------------------------------
        //
        // Both palettes are installed every time, and the preference is set
        // afterwards. Doing it this way means "System" resolves to whichever of
        // ours the OS asked for, instead of falling back to egui's stock look.
        if self.theme_applied != Some(self.settings.theme) || !self.style_pushed {
            crate::theme::apply_fonts(&ctx);
            crate::theme::apply_spacing(&ctx);
            ctx.set_visuals_of(egui::Theme::Light, crate::theme::visuals(false));
            ctx.set_visuals_of(egui::Theme::Dark, crate::theme::visuals(true));
            ctx.set_theme(match self.settings.theme {
                Theme::System => egui::ThemePreference::System,
                Theme::Light => egui::ThemePreference::Light,
                Theme::Dark => egui::ThemePreference::Dark,
            });
            self.theme_applied = Some(self.settings.theme);
            self.style_pushed = true;
            // Resolved colours, logged so a "wrong colours" report can be
            // checked against what the app actually installed. dark_mode,
            // override text, and panel fill must all agree with each other.
            {
                let v = ctx.theme();
                let vis = ui.visuals().clone();
                log_info!(
                    "theme installed: setting={:?} resolved={:?} dark_mode={} \
                     text={:?} panel={:?}",
                    self.settings.theme,
                    v,
                    vis.dark_mode,
                    vis.override_text_color,
                    vis.panel_fill,
                );
            }
            // The tray icon follows the theme, and it is the only app icon that
            // can change at runtime.
            if let Some(tray) = self.tray.as_ref() {
                tray.set_dark(self.is_dark());
            }
        }

        self.poll_events();
        self.ensure_tray();

        let visuals = ui.visuals().clone();
        let dark = visuals.dark_mode;
        let tone = accent(self.settings.mode, dark);

        // --- navigation rail ------------------------------------------------
        // egui 0.36 collapsed SidePanel and TopBottomPanel into a single `Panel`
        // with left/right/top/bottom constructors.
        egui::Panel::left("nav")
            .exact_size(228.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    // The rail follows the theme. It used to be hardcoded
                    // black, which left a black sidebar floating in an
                    // otherwise white window in light mode.
                    .fill(crate::theme::panel_bg(dark))
                    .inner_margin(Margin::symmetric(14, 16))
                    .stroke(egui::Stroke::new(
                        1.0,
                        ui.visuals().widgets.noninteractive.bg_stroke.color,
                    )),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::new(&icon::texture(ui.ctx()))
                            .fit_to_exact_size(Vec2::new(30.0, 30.0)),
                    );
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("XMG Focus")
                                .size(15.0)
                                .strong()
                                .color(ui.visuals().text_color()),
                        );
                        ui.label(
                            RichText::new("Optimizer")
                                .size(15.0)
                                .strong()
                                .color(ui.visuals().weak_text_color()),
                        );
                    });
                });

                ui.add_space(18.0);
                nav_label(ui, "MODES");
                for (t, label, hint) in [
                    (Tab::Dashboard, "Dashboard", "Quiet and Performance"),
                    (Tab::Backups, "Backups", "Snapshots and restore"),
                    (Tab::History, "History", "What changed and when"),
                    (Tab::Settings, "Settings", "Behaviour and appearance"),
                ] {
                    if nav_item(ui, self.tab == t, label, hint, tone) {
                        self.tab = t;
                    }
                    ui.add_space(3.0);
                }

                // Everything else is pushed to the bottom of the rail, which
                // is what makes the rail read as navigation rather than as a
                // panel of controls.
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.add_space(10.0);
                    if ui.button("Quit").clicked() {
                        self.stop_watch.store(true, Ordering::Relaxed);
                        self.temps_stop.store(true, Ordering::Relaxed);
                        self.quit_requested = true;
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("Hide to tray").clicked() {
                        self.hide_to_tray();
                    }
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                            .size(11.5)
                            .color(ui.visuals().weak_text_color()),
                    );
                    // `on_hover_text` lives on the Response, not on
                    // RichText - it has to be attached after the label is
                    // built, which is also when there is a rect to point at.
                    ui.label(
                        RichText::new(crate::paths::log_path().display().to_string())
                            .size(10.0)
                            .color(ui.visuals().weak_text_color()),
                    )
                    .on_hover_text(
                        "The log records every action and every command this app runs. \
                         Attach it to a bug report.",
                    );
                });
            });

        // --- page header ----------------------------------------------------
        //
        // One row: the title and its explanation on the left, the live status
        // on the right, vertically centred. This used to stack the title block
        // above a separate right-aligned row inside a 64 px panel, which
        // squeezed the blurb against the separator line.
        egui::Panel::top("header")
            .exact_size(80.0)
            .frame(
                egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(Margin::symmetric(20, 12))
                    .stroke(egui::Stroke::new(
                        0.0,
                        ui.visuals().widgets.noninteractive.bg_stroke.color,
                    )),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(self.tab.title())
                                .size(21.0)
                                .strong()
                                .color(ui.visuals().text_color()),
                        );
                        ui.label(
                            RichText::new(self.tab.blurb())
                                .size(12.5)
                                .color(ui.visuals().weak_text_color()),
                        );
                    });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.busy {
                            ui.spinner();
                        }
                        if self.on_ac {
                            pill(ui, "Mains power", true);
                        } else {
                            pill(ui, "On battery", false);
                        }
                        // Live temperatures, peeked without blocking: the
                        // poller thread owns the data and the UI only reads.
                        // Both pills are always shown so the header layout never
                        // jumps. A sensor with no data reads "–" with a tooltip
                        // naming what was checked; no value is ever invented.
                        // Direct EC reads stay forbidden (see README), so on
                        // machines without a WMI sensor "CPU –" is the honest
                        // answer - running LibreHardwareMonitor fills it in.
                        if let Ok(t) = self.temps.try_lock() {
                            match t.gpu {
                                Some(g) => pill(ui, &format!("GPU {g:.0}°"), g < 80.0),
                                None => {
                                    pill_plain(ui, "GPU –").on_hover_text(
                                        "No GPU reading - nvidia-smi gave no value.",
                                    );
                                }
                            }
                            match t.cpu {
                                Some(c) => pill(ui, &format!("CPU {c:.0}°"), c < 85.0),
                                None => {
                                    pill_plain(ui, "CPU –").on_hover_text(
                                        "No CPU sensor found - checked LibreHardwareMonitor, \
                                         OpenHardwareMonitor and the firmware zones \
                                         (MSAcpi plus the thermal-zone counter). Run \
                                         LibreHardwareMonitor for live CPU temperatures.",
                                    );
                                }
                            }
                        } else {
                            pill_plain(ui, "GPU –");
                            pill_plain(ui, "CPU –");
                        }
                        if self.elevated_ready {
                            pill(ui, "Background task ready", true);
                        } else {
                            // No button here on purpose: setup runs itself on
                            // launch (see App::ui auto-setup). This pill is
                            // only a status light for when that did not finish
                            // - the way back is Settings → Elevation.
                            pill_warn(ui, "Setup needed");
                        }
                        ui.add_space(4.0);
                    });
                });
            });

        // --- status bar -----------------------------------------------------
        //
        // One line, always visible, rather than a resizable log panel eating a
        // third of the window. The full history is on disk, and the newest line
        // is what matters while you are using it.
        egui::Panel::bottom("status")
            .exact_size(30.0)
            .frame(
                egui::Frame::new()
                    .fill(crate::theme::panel_bg(dark))
                    .inner_margin(Margin::symmetric(20, 6))
                    .stroke(egui::Stroke::new(
                        1.0,
                        ui.visuals().widgets.noninteractive.bg_stroke.color,
                    )),
            )
            .show(ui, |ui| {
                if let Some(n) = self.notes.first() {
                    let c = match n.kind {
                        NoteKind::Ok => ui.visuals().weak_text_color(),
                        NoteKind::Warn => warn_color(),
                        NoteKind::Err => err_color(),
                    };
                    ui.label(
                        RichText::new(format!("{}   {}", n.stamp, n.text))
                            .size(11.5)
                            .color(c),
                    );
                } else {
                    ui.label(
                        RichText::new("Ready")
                            .size(11.5)
                            .color(ui.visuals().weak_text_color()),
                    );
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(crate::paths::app_data_dir().display().to_string())
                            .size(10.5)
                            .color(ui.visuals().weak_text_color()),
                    );
                });
            });

        // --- page content ---------------------------------------------------
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(20.0))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    match self.tab {
                        Tab::Dashboard => self.dashboard(ui),
                        Tab::Backups => self.backups(ui),
                        Tab::History => self.history(ui),
                        Tab::Settings => self.settings_tab(ui),
                    }
                });
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_watch.store(true, Ordering::Relaxed);
        self.temps_stop.store(true, Ordering::Relaxed);
        let _ = self.settings.save();
    }
}

// ---------------------------------------------------------------- painting --

/// A small tracked heading above a group in the navigation rail.
fn nav_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(10.5)
            .strong()
            .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(4.0);
}

/// One row of the navigation rail.
///
/// Drawn as a button rather than a label so the whole row is the hit target -
/// a 20 pixel line of text is not something to aim at. The active row is
/// filled, outlined, and given an accent bar down its left edge, so the eye
/// finds it without having to read it.
fn nav_item(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    hint: &str,
    tone: egui::Color32,
) -> bool {
    let dark = ui.visuals().dark_mode;
    let text_c = if selected {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };

    let clicked = ui
        .horizontal(|ui| {
            // The marker bar is allocated, not painted, so it reserves its
            // space whether or not it is shown and the rows stay aligned.
            let (rect, _) = ui.allocate_exact_size(Vec2::new(3.0, 26.0), egui::Sense::hover());
            if selected {
                ui.painter()
                    .rect_filled(rect.expand(1.0), CornerRadius::same(2), tone);
            }

            // egui 0.36 has no RichText::on_hover_color, so the unselected rows are
            // already drawn in a mid-grey that stays legible on both the resting
            // and the hovered button fill.
            ui.add(
                egui::Button::new(
                    egui::RichText::new(label)
                        .size(13.5)
                        .strong()
                        .color(text_c),
                )
                .fill(if selected {
                    tone.gamma_multiply(if dark { 0.26 } else { 0.10 })
                } else {
                    // Unselected rows keep a quiet card fill and hairline so
                    // they still read as buttons. Fully transparent rows looked
                    // like plain labels in the screenshots.
                    crate::theme::card_bg(dark)
                })
                .stroke(if selected {
                    egui::Stroke::new(1.0, tone.gamma_multiply(0.5))
                } else {
                    egui::Stroke::new(
                        1.0,
                        ui.visuals().widgets.noninteractive.bg_stroke.color,
                    )
                })
                .corner_radius(CornerRadius::same(7))
                .min_size(Vec2::new(ui.available_width(), 26.0)),
            )
            .clicked()
        })
        .inner;

    // Only the active row carries a hint, so the rail stays short enough to
    // read at a glance instead of becoming a wall of text.
    if selected {
        ui.label(
            egui::RichText::new(hint)
                .size(10.5)
                .color(ui.visuals().weak_text_color()),
        );
    }

    clicked
}

use crate::theme::{accent, card_bg, contrast_text, err_color, ok_color, surface, surface_stroke, warn_color};
use crate::log::{log_debug, log_error, log_info, log_warn};

/// How long a queued job may take before the UI stops waiting and unlocks.
///
/// Fans are driven through UI Automation of a vendor app that may be slow to
/// open, so this is generous. It exists only to stop one stuck job from
/// disabling the whole window.
const JOB_TIMEOUT_SECS: f32 = 120.0;

/// A card the whole of which is one big button. Returns `true` when clicked.
///
/// Only for cards that contain nothing else clickable - see [`panel_card`].
fn card<R>(
    ui: &mut egui::Ui,
    selected: bool,
    tone: egui::Color32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> bool {
    let dark = ui.visuals().dark_mode;
    let fill = if selected {
        tone.gamma_multiply(if dark { 0.30 } else { 0.10 })
    } else {
        card_bg(dark)
    };
    let edge = if selected {
        Stroke::new(1.5, tone)
    } else {
        Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color)
    };

    let resp = egui::Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(14.0)
        .stroke(edge)
        .show(ui, add_contents);

    resp.response.interact(egui::Sense::click()).clicked()
}

/// A card that holds buttons of its own and is therefore **not** clickable.
///
/// This split is not cosmetic. A card made clickable with
/// `Response::interact(Sense::click)` claims pointer events across its whole
/// rectangle, which races with every button inside it - the card and the button
/// compete for the same click, and the result is buttons that sometimes do
/// nothing. A card is either one big button, or a container. Never both.
fn panel_card<R>(
    ui: &mut egui::Ui,
    tone: egui::Color32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) {
    let dark = ui.visuals().dark_mode;
    egui::Frame::new()
        .fill(card_bg(dark))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(14.0)
        .stroke(Stroke::new(1.0, tone.gamma_multiply(if dark { 0.45 } else { 0.3 })))
        .show(ui, add_contents);
}

/// A labelled figure for the status strip and the profile cards.
///
/// Takes `impl AsRef<str>` because the value is usually a `format!` result
/// rather than a literal, and picking one at random would be worse than
/// accepting both.
fn stat(ui: &mut egui::Ui, label: &str, value: impl AsRef<str>, colour: egui::Color32) {
    let value = value.as_ref();
    ui.vertical(|ui| {
        ui.label(
            RichText::new(label)
                .size(10.0)
                .strong()
                .color(ui.visuals().weak_text_color()),
        );
        ui.label(RichText::new(value).size(16.0).strong().color(colour));
    });
}

/// One figure inside a profile card. Smaller and quieter than [`stat`].
fn figure(ui: &mut egui::Ui, label: &str, value: impl AsRef<str>) {
    let value = value.as_ref();
    ui.vertical(|ui| {
        ui.label(
            RichText::new(label)
                .size(10.5)
                .color(ui.visuals().weak_text_color()),
        );
        ui.label(
            RichText::new(value)
                .size(14.0)
                .strong()
                .color(ui.visuals().text_color()),
        );
    });
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(
        RichText::new(title.to_uppercase())
            .size(11.0)
            .strong()
            .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(2.0);
}

fn pill(ui: &mut egui::Ui, text: &str, good: bool) {
    let c = if good { ok_color() } else { warn_color() };
    // Squared chip rather than a stadium pill, to match the buttons and cards.
    egui::Frame::new()
        .fill(c.gamma_multiply(0.18))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(c));
        });
}

/// Neutral chip for states that are neither good nor bad: unknown sensor
/// readings and pending setup. Same shape as [`pill`], no semantic colour.
/// Returns the response so callers can hang a tooltip off it.
fn pill_plain(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let c = ui.visuals().weak_text_color();
    egui::Frame::new()
        .fill(ui.visuals().widgets.noninteractive.bg_stroke.color.gamma_multiply(0.25))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(c));
        })
        .response
}

/// Amber chip for attention states. Same shape as [`pill`].
fn pill_warn(ui: &mut egui::Ui, text: &str) {
    let c = warn_color();
    egui::Frame::new()
        .fill(c.gamma_multiply(0.16))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(c));
        });
}

impl App {
    // ------------------------------------------------------------ dashboard --

    fn dashboard(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        let on_ac = self.on_ac;

        // --- the one thing worth knowing at a glance -----------------------
        //
        // A row of figures rather than a sentence: what mode is live, what the
        // laptop is doing, and whether the settings on disk match it.
        let (consistent, drifted, total, scheme) = match &self.drift {
            Some(d) => (
                d.drifted() == 0,
                d.drifted(),
                d.total(),
                d.scheme_name.clone(),
            ),
            None => (false, 0, 0, String::new()),
        };

        panel_card(ui, accent(self.settings.mode, dark), |ui| {
            ui.horizontal(|ui| {
                stat(
                    ui,
                    "ACTIVE MODE",
                    self.settings.mode.label(),
                    accent(self.settings.mode, dark),
                );
                ui.add_space(28.0);
                stat(
                    ui,
                    "POWER SOURCE",
                    if on_ac { "Mains" } else { "Battery" },
                    if on_ac { ok_color() } else { warn_color() },
                );
                ui.add_space(28.0);
                stat(
                    ui,
                    "SETTINGS",
                    if consistent {
                        "Up to date".to_string()
                    } else if total > 0 {
                        format!("{drifted} of {total} differ")
                    } else {
                        "Checking".into()
                    },
                    if consistent { ok_color() } else { warn_color() },
                );
                ui.add_space(28.0);
                stat(ui, "POWER SCHEME", scheme, ui.visuals().weak_text_color());

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Check settings").clicked() {
                        self.recheck_drift();
                    }
                });
            });
        });

        ui.add_space(18.0);
        section(ui, "Profiles");

        ui.columns(2, |cols| {
            for (i, mode) in [Mode::Quiet, Mode::Performance].iter().enumerate() {
                let selected = self.settings.mode == *mode;
                let tone = accent(*mode, dark);
                cols[i].add_space(4.0);
                let clicked = card(&mut cols[i], selected, tone, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::Image::new(&icon::texture(ui.ctx()))
                                .fit_to_exact_size(Vec2::new(30.0, 30.0)),
                        );
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(mode.label())
                                        .size(19.0)
                                        .strong()
                                        .color(ui.visuals().text_color()),
                                );
                                if selected {
                                    ui.label(
                                        RichText::new("ACTIVE")
                                            .size(9.5)
                                            .strong()
                                            .color(tone),
                                    );
                                }
                            });
                            ui.label(
                                RichText::new(mode.tagline())
                                    .size(12.0)
                                    .color(ui.visuals().weak_text_color()),
                            );
                        });
                    });

                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(mode.blurb())
                            .size(12.5)
                            .color(ui.visuals().weak_text_color()),
                    );

                    ui.add_space(12.0);
                    let p = profile::Params::for_mode(*mode, on_ac);
                    ui.columns(2, |c| {
                        // columns() hands the closure a `[Ui; N]`, so each cell is indexed by value
                    // and has to be re-borrowed mutably.
                    figure(&mut c[0], "CPU cap", format!("{}%", p.cpu_max));
                        figure(&mut c[1], "GPU limit", format!("{} W", p.gpu_watts));
                        figure(&mut c[0], "Efficiency", format!("{}", p.epp));
                        figure(&mut c[1], "Fans", p.fan_mode.label());
                    });

                    // What this profile does on each power source. A mode is
                    // not one set of numbers: mains and battery get different
                    // values, and the charger switch swaps between them.
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("When the charger changes, this profile becomes:")
                            .size(11.5)
                            .color(ui.visuals().weak_text_color()),
                    );
                    // The refresh rate does not vary by power source (Quiet is
                    // fixed, Performance is the panel maximum), so it is
                    // resolved once per card, not per row.
                    let hz = crate::display::label_for_mode(*mode);
                    for (src_label, src_ac) in [("On mains", true), ("On battery", false)] {
                        let sp = profile::Params::for_mode(*mode, src_ac);
                        let live = src_ac == on_ac;
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(if live { "●" } else { "○" })
                                    .size(11.0)
                                    .color(if live {
                                        tone
                                    } else {
                                        ui.visuals().weak_text_color()
                                    }),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "{src_label}: CPU {}% · GPU {} W · {hz} · brightness {}% · fans {}",
                                    sp.cpu_max,
                                    sp.gpu_watts,
                                    sp.brightness,
                                    sp.fan_mode.label()
                                ))
                                .size(12.0)
                                .color(if live {
                                    ui.visuals().text_color()
                                } else {
                                    ui.visuals().weak_text_color()
                                }),
                            );
                        });
                    }
                });
                if clicked && !selected && !self.busy {
                    self.settings.mode = *mode;
                    let _ = self.settings.save();
                    self.drift_acknowledged = false;
                    if let Some(tray) = self.tray.as_ref() {
                        tray.set_mode(*mode);
                    }
                    self.recheck_drift();
                }
            }
        });

        ui.add_space(16.0);
        ui.horizontal(|ui| {
            // The label colour is computed from the fill: white on the deep
            // fills, near-black on the light one. Hardcoded white text is what
            // used to vanish on light fills.
            let fill = accent(self.settings.mode, dark);
            let label = if self.busy {
                format!("Applying {}…", self.settings.mode.label())
            } else {
                format!("Apply {}", self.settings.mode.label())
            };
            if ui
                .add_enabled(
                    !self.busy,
                    egui::Button::new(
                        RichText::new(label)
                            .size(14.5)
                            .strong()
                            .color(contrast_text(fill)),
                    )
                    .fill(fill)
                    .corner_radius(CornerRadius::same(7))
                    .min_size(Vec2::new(200.0, 42.0)),
                )
                .clicked()
            {
                self.apply_current_mode();
            }
            // A mode switch rewrites a scheme, shells out a dozen times and
            // drives the fan app through UI automation - seconds, not frames.
            // The header spinner alone was too easy to miss, so the button
            // also says what is happening right where the click landed.
            if self.busy {
                ui.spinner();
                ui.label(
                    RichText::new("Working… the app has not frozen.")
                        .size(12.0)
                        .color(ui.visuals().weak_text_color()),
                );
            }
        });

        ui.add_space(10.0);
        self.drift_panel(ui);

        ui.add_space(10.0);
        section(ui, "Safety");
        self.safety_panel(ui);
    }

    /// The comparison between what the mode expects and what the machine has.
    fn drift_panel(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        let Some(report) = self.drift.clone() else {
            return;
        };

        let drifted = report.drifted();
        let clean = report.is_clean();
        let managed = report.managed;
        let acknowledged = self.drift_acknowledged;

        section(ui, "Consistency check");

        // The drift report is a status, so it keeps semantic colours: accent when the
        // scheme is ours and consistent, amber when something disagrees.
        let (bg, tint) = if clean && managed {
            (surface(dark), accent(Mode::Quiet, dark))
        } else {
            (
                warn_color().gamma_multiply(if dark { 0.16 } else { 0.10 }),
                warn_color(),
            )
        };

        egui::Frame::new()
            .fill(bg)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(12.0)
            .show(ui, |ui| {
                if clean && managed {
                    ui.label(
                        RichText::new(format!(
                            "All {} settings match {} ({}).",
                            report.total(),
                            report.mode.label(),
                            report.values_source()
                        ))
                        .strong()
                        .size(13.0)
                        .color(ui.visuals().text_color()),
                    );
                    ui.label(
                        RichText::new(format!(
                            "Active scheme: {}  \u{2022}  checked just now",
                            report.scheme_name
                        ))
                        .size(11.5)
                        .color(ui.visuals().weak_text_color()),
                    );
                } else {
                    let headline = if drifted > 0 {
                        format!(
                            "{} of {} settings do not match {}.",
                            drifted,
                            report.total(),
                            report.mode.label()
                        )
                    } else {
                        "The active scheme was not created by this app.".to_string()
                    };
                    ui.label(RichText::new(headline).strong().size(13.0).color(tint));
                    ui.label(
                        RichText::new(format!("Active scheme: {}", report.scheme_name))
                            .size(11.5)
                            .color(ui.visuals().weak_text_color()),
                    );

                    if drifted > 0 && !acknowledged {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("Nothing has been changed yet. What would you like?")
                                .size(12.0),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    !self.busy,
                                    egui::Button::new(
                                        RichText::new(format!("Change to {}", report.mode.label()))
                                            .strong()
                                            .color(contrast_text(tint)),
                                    )
                                    .fill(tint)
                                    .corner_radius(CornerRadius::same(7))
                                    .min_size(Vec2::new(150.0, 32.0)),
                                )
                                .clicked()
                            {
                                self.apply_current_mode();
                            }
                            if ui
                                .add(
                                    egui::Button::new("Keep current settings")
                                        .corner_radius(CornerRadius::same(7))
                                        .min_size(Vec2::new(160.0, 32.0)),
                                )
                                .clicked()
                            {
                                self.drift_acknowledged = true;
                                self.note(
                                    "keeping the current system settings - nothing was changed",
                                    NoteKind::Warn,
                                );
                            }
                        });
                    } else if drifted > 0 && acknowledged {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("You chose to keep the current settings.")
                                .size(12.0)
                                .color(tint),
                        );
                        if ui.small_button("Re-check").clicked() {
                            self.drift_acknowledged = false;
                            self.recheck_drift();
                        }
                    }
                }

                ui.add_space(6.0);
                // Always clickable, even when everything matches: the table is
                // the only place that shows every value, and hiding it exactly
                // when the machine is healthy keeps the healthy state
                // unverifiable.
                if ui
                    .small_button(if self.drift_expanded {
                        "Hide details"
                    } else {
                        "Show all settings"
                    })
                    .clicked()
                {
                    self.drift_expanded = !self.drift_expanded;
                }

                if self.drift_expanded {
                    ui.add_space(6.0);
                    egui::Grid::new("drift")
                        .num_columns(3)
                        .spacing([18.0, 3.0])
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("Setting")
                                    .size(11.0)
                                    .strong()
                                    .color(ui.visuals().text_color()),
                            );
                            ui.label(
                                RichText::new("Expected")
                                    .size(11.0)
                                    .strong()
                                    .color(ui.visuals().text_color()),
                            );
                            ui.label(
                                RichText::new("Actual")
                                    .size(11.0)
                                    .strong()
                                    .color(ui.visuals().text_color()),
                            );
                            ui.end_row();
                            for c in &report.checks {
                                let col = if c.consistent {
                                    ui.visuals().text_color()
                                } else {
                                    tint
                                };
                                ui.label(RichText::new(c.label).size(11.5).color(col));
                                ui.label(
                                    RichText::new(drift::fmt_value(c.expected))
                                        .size(11.5)
                                        .monospace(),
                                );
                                let actual =
                                    c.actual.map(drift::fmt_value).unwrap_or_else(|| "not present".into());
                                let col = if c.consistent {
                                    ui.visuals().weak_text_color()
                                } else {
                                    tint
                                };
                                let text = if c.consistent {
                                    actual
                                } else {
                                    format!("{actual}  (!)")
                                };
                                ui.label(
                                    RichText::new(text).size(11.5).monospace().color(col),
                                );
                                ui.end_row();
                            }
                        });
                }
            });
    }

    fn safety_panel(&self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        egui::Frame::new()
            .fill(surface(dark))
            .stroke(Stroke::new(1.0, surface_stroke(dark)))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.label(
                    RichText::new("This app never:")
                        .strong()
                        .size(12.5)
                        .color(ui.visuals().text_color()),
                );
                for line in [
                    "overclocks or undervolts anything",
                    "disables HVCI / VBS or other security features",
                    "touches the BIOS or flashes firmware",
                    "writes undocumented EC or driver binaries",
                    "changes a setting you asked it to keep",
                    "adds, removes, delays or rewrites startup entries",
                ] {
                    ui.label(
                        RichText::new(format!("  \u{2022}  {line}"))
                            .size(12.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                }
            });
    }

    // -------------------------------------------------------------- backups --

    fn backups(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        ui.add_space(4.0);

        egui::Frame::new()
            .fill(surface(dark))
            .stroke(Stroke::new(1.0, surface_stroke(dark)))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(12.0)
            .show(ui, |ui| {
                // States its colour explicitly rather than relying on the
                // default: this headline sits on a tinted fill, where a wrong
                // default would be unreadable instead of merely off.
                ui.label(
                    RichText::new("Backups are only ever added, never removed.")
                        .strong()
                        .size(13.0)
                        .color(ui.visuals().text_color()),
                );
                ui.label(
                    RichText::new(
                        "A snapshot is taken automatically before every change. Creating a new \
                         initial backup first copies the existing one into the archive, so the \
                         old one always survives.",
                    )
                    .size(12.0)
                    .color(ui.visuals().weak_text_color()),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("Folder: {}", self.backup_dir.display()))
                        .size(11.5)
                        .monospace()
                        .color(ui.visuals().weak_text_color()),
                );
            });

        ui.add_space(12.0);
        section(ui, "Initial backup");
        ui.horizontal(|ui| {
            panel_card(ui, accent(Mode::Quiet, dark), |ui| {
                if self.has_factory {
                    ui.label(
                        RichText::new("Present")
                            .strong()
                            .size(15.0)
                            .color(ui.visuals().text_color()),
                    );
                    ui.label(
                        RichText::new(format!("{} archived copies", self.archives.len()))
                            .size(12.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                } else {
                    ui.label(
                        RichText::new("None yet")
                            .strong()
                            .size(15.0)
                            .color(ui.visuals().text_color()),
                    );
                    ui.label(
                        RichText::new("created automatically before the first change")
                            .size(12.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                }
                if ui.button("Restore from initial backup").clicked() {
                    self.submit(Job::Restore {
                        source: RestoreSource::Factory,
                    });
                }
            });
            panel_card(ui, warn_color(), |ui| {
                ui.label(
                    RichText::new("Create a new one")
                        .strong()
                        .size(15.0)
                        .color(ui.visuals().text_color()),
                );
                ui.label(
                    RichText::new("three confirmations required")
                        .size(12.0)
                        .color(ui.visuals().weak_text_color()),
                );
                if self.confirm_stage == 0 && ui.button("Start").clicked() {
                    self.confirm_stage = 1;
                    self.confirm_note.clear();
                }
            });
        });

        if self.confirm_stage > 0 {
            ui.add_space(8.0);
            egui::Frame::new()
                .fill(warn_color().gamma_multiply(if dark { 0.14 } else { 0.10 }))
                .stroke(Stroke::new(1.0, warn_color().gamma_multiply(0.45)))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(12.0)
                .show(ui, |ui| match self.confirm_stage {
                    1 => {
                        ui.label(
                            RichText::new("Step 1 of 3 - understand what this does")
                                .strong()
                                .color(ui.visuals().text_color()),
                        );
                        ui.label(
                            RichText::new(
                                "This captures the CURRENT machine state as the new initial \
                                 backup. The existing initial backup is copied into the archive \
                                 first and stays available forever.",
                            )
                            .size(12.0)
                            .color(ui.visuals().weak_text_color()),
                        );
                        ui.horizontal(|ui| {
                            if ui.button("Continue").clicked() {
                                self.confirm_stage = 2;
                            }
                            if ui.button("Cancel").clicked() {
                                self.confirm_stage = 0;
                            }
                        });
                    }
                    2 => {
                        ui.label(
                            RichText::new("Step 2 of 3 - name it so you can recognise it")
                                .strong()
                                .color(ui.visuals().text_color()),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut self.confirm_note)
                                .hint_text("e.g. after reinstalling Windows")
                                .desired_width(320.0),
                        );
                        ui.horizontal(|ui| {
                            if ui.button("Continue").clicked() {
                                self.confirm_stage = 3;
                            }
                            if ui.button("Back").clicked() {
                                self.confirm_stage = 1;
                            }
                        });
                    }
                    _ => {
                        ui.label(
                            RichText::new("Step 3 of 3 - final confirmation")
                                .strong()
                                .color(ui.visuals().text_color()),
                        );
                        ui.label(
                            RichText::new("Type  CREATE  to proceed.")
                                .size(12.0)
                                .color(ui.visuals().weak_text_color()),
                        );
                        let mut typed = String::new();
                        ui.add(egui::TextEdit::singleline(&mut typed).desired_width(200.0));
                        ui.horizontal(|ui| {
                            let ok = typed.trim() == "CREATE";
                            let b = ui.add_enabled(
                                ok,
                                egui::Button::new(
                                    RichText::new("Create new initial backup").strong(),
                                ),
                            );
                            if b.clicked() {
                                let note = if self.confirm_note.trim().is_empty() {
                                    "manual".to_string()
                                } else {
                                    self.confirm_note.trim().to_string()
                                };
                                self.confirm_stage = 0;
                                self.submit(Job::CreateFactoryBackup { note });
                            }
                            if ui.button("Cancel").clicked() {
                                self.confirm_stage = 0;
                            }
                        });
                    }
                });
        }

        ui.add_space(12.0);
        section(ui, &format!("Snapshots ({})", self.snapshots.len()));
        ui.horizontal(|ui| {
            if ui.button("Take a snapshot now").clicked() {
                self.submit(Job::Snapshot {
                    reason: "manual".into(),
                    manual: true,
                });
            }
            if ui.button("Open backup folder").clicked() {
                worker::open_backup_folder();
            }
        });
        ui.add_space(6.0);

        // Own the rows first: iterating a field of `self` while calling
        // `self.submit(..)` inside the closure would borrow self twice.
        let rows: Vec<(String, String, &'static str)> = self
            .snapshots
            .iter()
            .map(|s| {
                let kind = match s.kind {
                    backup::SnapshotKind::Automatic => "auto",
                    backup::SnapshotKind::Manual => "manual",
                };
                (s.id.clone(), format!("{}  {}", s.created, s.label), kind)
            })
            .collect();

        // Rows are a grid, not stacked horizontals with a right-aligned button
        // each. Right-alignment inside separate horizontals drifts from row to
        // row as content widths differ; grid columns line up by construction.
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            if rows.is_empty() {
                ui.label(
                    RichText::new("No snapshots yet.").color(ui.visuals().weak_text_color()),
                );
            }
            egui::Grid::new("snapshot-rows")
                .num_columns(3)
                .spacing([14.0, 6.0])
                .striped(true)
                .show(ui, |ui| {
                    for (id, text, kind) in rows.iter() {
                        ui.label(RichText::new(text.clone()).size(12.0));
                        ui.label(
                            RichText::new(*kind)
                                .size(11.0)
                                .color(ui.visuals().weak_text_color()),
                        );
                        ui.with_layout(
                            Layout::right_to_left(Align::Center),
                            |ui| {
                                if ui.small_button("Restore").clicked() {
                                    self.submit(Job::Restore {
                                        source: RestoreSource::Snapshot(id.clone()),
                                    });
                                }
                            },
                        );
                        ui.end_row();
                    }
                });
        });

        if !self.archives.is_empty() {
            ui.add_space(10.0);
            section(ui, "Archived initial backups");
            let arch: Vec<(String, String)> = self
                .archives
                .iter()
                .map(|a| (a.id.clone(), format!("{}  {}", a.id, a.note)))
                .collect();
            egui::Grid::new("archive-rows")
                .num_columns(2)
                .spacing([14.0, 6.0])
                .striped(true)
                .show(ui, |ui| {
                    for (id, text) in arch.iter() {
                        ui.label(RichText::new(text.clone()).size(12.0).monospace());
                        ui.with_layout(
                            Layout::right_to_left(Align::Center),
                            |ui| {
                                if ui.small_button("Restore").clicked() {
                                    self.submit(Job::Restore {
                                        source: RestoreSource::Archive(id.clone()),
                                    });
                                }
                            },
                        );
                        ui.end_row();
                    }
                });
        }
    }

    // -------------------------------------------------------------- history --

    /// Timeline of everything the app did, derived from the snapshots: every
    /// apply and every restore snapshots first, so the list is complete
    /// without any extra bookkeeping.
    ///
    /// Deliberately more than the Backups tab, which is a plain list of
    /// files with one Restore each. This page is for *reading* history:
    /// filter it, search it, expand any entry to see exactly which settings
    /// it captured, compare any two entries against each other, and jump to
    /// the file on disk. Only one thing it offers is a change — Restore,
    /// because undoing is the point of keeping snapshots.
    fn history(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        ui.add_space(4.0);

        if self.snapshots.is_empty() {
            panel_card(ui, accent(Mode::Quiet, dark), |ui| {
                ui.label(
                    RichText::new("No activity yet.")
                        .strong()
                        .size(14.0)
                        .color(ui.visuals().text_color()),
                );
                ui.label(
                    RichText::new(
                        "The first entry appears the first time a mode is applied \
                         or a snapshot is taken.",
                    )
                    .size(12.0)
                    .color(ui.visuals().weak_text_color()),
                );
            });
            return;
        }

        // Rebuilt only when the snapshot list itself changes: parsing every
        // file on every frame would do real IO sixty times a second for
        // content that barely moves.
        let ids: Vec<String> = self.snapshots.iter().map(|s| s.id.clone()).collect();
        if ids != self.history_ids {
            self.history_rows = build_history_rows(&self.snapshots);
            self.history_ids = ids;
            // A snapshot that vanished (trimmed) must not leave a compare pair
            // or an open detail panel pointing at a file that no longer exists.
            let live: Vec<&String> = self.history_rows.iter().map(|r| &r.id).collect();
            self.history_cmp.retain(|id| live.iter().any(|live_id| live_id == id));
            if self.history_cmp.len() < 2 {
                self.history_cmp_diff = None;
            }
            // Cloned, not borrowed: clearing the field while a borrow of it
            // is live is exactly what the borrow checker is there to stop.
            if let Some(open) = self.history_open.clone() {
                if !live.iter().any(|id| *id == &open) {
                    self.history_open = None;
                }
            }
        }

        self.history_toolbar(ui);

        // Filtered view. Taken as an owned Vec so the grid closure never
        // touches `self.history_rows` while it is being drawn.
        let needle = self.history_search.trim().to_ascii_lowercase();
        let filter = self.history_filter;
        let visible: Vec<HistoryRow> = self
            .history_rows
            .iter()
            .filter(|r| filter.matches(r.kind))
            .filter(|r| {
                needle.is_empty()
                    || r.created.to_ascii_lowercase().contains(&needle)
                    || r.event.to_ascii_lowercase().contains(&needle)
                    || r.mode.to_ascii_lowercase().contains(&needle)
                    || r.id.to_ascii_lowercase().contains(&needle)
            })
            .cloned()
            .collect();

        ui.label(
            RichText::new(format!(
                "{} of {} events - automatic snapshots are trimmed past 40, manual ones never",
                visible.len(),
                self.history_rows.len()
            ))
            .size(11.5)
            .color(ui.visuals().weak_text_color()),
        );
        ui.add_space(6.0);

        if visible.is_empty() {
            ui.label(
                RichText::new("Nothing matches that filter.")
                    .size(12.0)
                    .color(ui.visuals().weak_text_color()),
            );
        } else {
            self.history_table(ui, &visible);
        }

        // Actions are resolved after the grid, against the ids it drew.
        if let Some((id, act)) = self.history_act.take() {
            self.run_history_action(&id, act);
        }

        ui.add_space(10.0);
        self.history_detail(ui);
        self.history_compare(ui);
    }

    /// Filter chips, search field and the two page-level actions.
    fn history_toolbar(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        let tone = accent(Mode::Quiet, dark);

        egui::Frame::new()
            .fill(card_bg(dark))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(12, 10))
            .stroke(Stroke::new(1.0, tone.gamma_multiply(if dark { 0.45 } else { 0.3 })))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Show")
                            .size(11.0)
                            .strong()
                            .color(ui.visuals().weak_text_color()),
                    );
                    for f in [
                        HistoryFilter::All,
                        HistoryFilter::Auto,
                        HistoryFilter::Manual,
                    ] {
                        if ui
                            .selectable_label(self.history_filter == f, f.label())
                            .clicked()
                        {
                            self.history_filter = f;
                        }
                    }
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new("Search")
                            .size(11.0)
                            .strong()
                            .color(ui.visuals().weak_text_color()),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.history_search)
                            .hint_text("time, event, mode, id")
                            .desired_width(190.0),
                    );
                    if !self.history_search.is_empty() && ui.small_button("Clear").clicked() {
                        self.history_search.clear();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !self.busy,
                                egui::Button::new("Take a snapshot now"),
                            )
                            .clicked()
                        {
                            self.submit(Job::Snapshot {
                                reason: "manual".into(),
                                manual: true,
                            });
                        }
                        if ui.button("Open backup folder").clicked() {
                            worker::open_backup_folder();
                        }
                    });
                });
            });
    }

    /// The event grid. Buttons only *record* what was pressed
    /// (`self.history_act`); the action runs after the grid, so no closure
    /// ever borrows `self` mutably while it is being drawn.
    fn history_table(&mut self, ui: &mut egui::Ui, rows: &[HistoryRow]) {
        let dark = ui.visuals().dark_mode;
        let tone = accent(Mode::Quiet, dark);
        let open = self.history_open.clone();
        let cmp = self.history_cmp.clone();

        egui::Grid::new("history-rows")
            .num_columns(7)
            .spacing([14.0, 6.0])
            .striped(true)
            .show(ui, |ui| {
                for head in ["TIME", "EVENT", "MODE", "VALUES", "CHANGED", "KIND", ""] {
                    ui.label(
                        RichText::new(head)
                            .size(10.5)
                            .strong()
                            .color(ui.visuals().weak_text_color()),
                    );
                }
                ui.end_row();

                for r in rows {
                    let is_open = open.as_deref() == Some(r.id.as_str());
                    let is_a = cmp.first().map(|a| a == &r.id).unwrap_or(false);
                    let is_b = cmp.get(1).map(|b| b == &r.id).unwrap_or(false);

                    ui.label(
                        RichText::new(r.created.clone())
                            .size(12.0)
                            .color(ui.visuals().text_color()),
                    )
                    .on_hover_text(r.id.clone());
                    ui.label(RichText::new(r.event.clone()).size(12.0));
                    ui.label(
                        RichText::new(r.mode.clone())
                            .size(12.0)
                            .color(ui.visuals().text_color()),
                    );
                    ui.label(
                        RichText::new(if r.settings > 0 {
                            format!("{}", r.settings)
                        } else {
                            "—".into()
                        })
                        .size(12.0)
                        .color(ui.visuals().text_color()),
                    );
                    ui.label(
                        RichText::new(
                            r.delta_prev.map(|d| format!("{d}")).unwrap_or("—".into()),
                        )
                        .size(12.0)
                        .color(ui.visuals().text_color()),
                    );
                    ui.label(
                        RichText::new(r.kind)
                            .size(11.0)
                            .color(ui.visuals().weak_text_color()),
                    );

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if ui
                            .add_enabled(!self.busy, egui::Button::new("Restore"))
                            .on_hover_text(
                                "Write this snapshot back onto the active power scheme. \
                                 The current state is snapshotted first, so this is \
                                 itself undoable.",
                            )
                            .clicked()
                        {
                            self.history_act = Some((r.id.clone(), HistoryAction::Restore));
                        }
                        if ui
                            .small_button("Details")
                            .on_hover_text("Show every setting captured in this entry.")
                            .clicked()
                        {
                            self.history_act = Some((r.id.clone(), HistoryAction::Details));
                        }
                        if ui
                            .small_button("File")
                            .on_hover_text("Show this snapshot in Explorer.")
                            .clicked()
                        {
                            self.history_act = Some((r.id.clone(), HistoryAction::Reveal));
                        }
                        // A/B marks: filled while the row is the marked slot,
                        // outlined otherwise. Two marks produce a comparison.
                        for (label, filled, act) in [
                            ("A", is_a, HistoryAction::MarkA),
                            ("B", is_b, HistoryAction::MarkB),
                        ] {
                            // Bound to locals first. An `if` without braces
                            // around its else arm is not accepted inside a
                            // method-call argument.
                            let mark_fill = if filled { tone } else { else_fill(dark) };
                            let mark_edge = if filled {
                                tone
                            } else {
                                ui.visuals().widgets.noninteractive.bg_stroke.color
                            };
                            let mark_text = if filled {
                                contrast_text(tone)
                            } else {
                                ui.visuals().weak_text_color()
                            };
                            let mark_hint = if filled {
                                format!("Clear comparison slot {label}.")
                            } else {
                                format!("Mark this entry as {label} for the comparison.")
                            };
                            let b = egui::Button::new(
                                RichText::new(label).size(11.0).strong().color(mark_text),
                            )
                            .min_size(Vec2::new(22.0, 20.0))
                            .fill(mark_fill)
                            .stroke(Stroke::new(1.0, mark_edge));
                            if ui.add(b).on_hover_text(mark_hint).clicked() {
                                self.history_act = Some((r.id.clone(), act));
                            }
                        }
                        // Marks the row whose detail panel is open below, so
                        // the table and the panel always agree.
                        if is_open {
                            ui.label(RichText::new("▾").size(11.0).color(tone));
                        }
                    });
                    ui.end_row();
                }
            });
    }

    /// Runs whatever the grid recorded this frame.
    fn run_history_action(&mut self, id: &str, act: HistoryAction) {
        let Some(row) = self.history_rows.iter().find(|r| r.id == id).cloned() else {
            return;
        };
        match act {
            HistoryAction::Details => {
                self.history_open = if self.history_open.as_deref() == Some(id) {
                    None
                } else {
                    Some(id.to_string())
                };
            }
            HistoryAction::Restore => {
                self.submit(Job::Restore {
                    source: RestoreSource::Snapshot(id.to_string()),
                });
            }
            HistoryAction::Reveal => {
                worker::reveal(std::path::Path::new(&row.path));
            }
            HistoryAction::MarkA | HistoryAction::MarkB => {
                let slot = usize::from(act == HistoryAction::MarkB);
                // Tapping a marked row clears that slot, so a mis-click is
                // undoable without a separate Reset.
                if self.history_cmp.get(slot) == Some(&row.id) {
                    self.history_cmp.remove(slot);
                } else if slot < self.history_cmp.len() {
                    self.history_cmp[slot] = row.id.clone();
                } else {
                    self.history_cmp.push(row.id.clone());
                }
                self.recompute_compare();
            }
        }
    }

    /// Marks the entry the detail panel shows, and its raw values.
    fn history_detail(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.history_open.clone() else {
            return;
        };
        let Some(row) = self.history_rows.iter().find(|r| r.id == id).cloned() else {
            self.history_open = None;
            return;
        };
        let dark = ui.visuals().dark_mode;
        let changes = row.changes.clone();
        let busy = self.busy;

        section(ui, "Entry detail");
        panel_card(ui, accent(Mode::Quiet, dark), |ui| {
            egui::Grid::new("history-detail")
                .num_columns(2)
                .spacing([18.0, 2.0])
                .show(ui, |ui| {
                    for (k, v) in [
                        ("When", row.created.clone()),
                        ("Event", row.event.clone()),
                        ("Mode", row.mode.clone()),
                        ("Kind", row.kind.to_string()),
                        ("Settings captured", row.settings.to_string()),
                        (
                            "Changed vs previous",
                            row.delta_prev
                                .map(|d| d.to_string())
                                .unwrap_or_else(|| "— (oldest entry)".into()),
                        ),
                        ("Snapshot id", row.id.clone()),
                        ("File", row.path.clone()),
                    ] {
                        ui.label(
                            RichText::new(k)
                                .size(11.5)
                                .strong()
                                .color(ui.visuals().weak_text_color()),
                        );
                        ui.label(
                            RichText::new(v)
                                .size(11.5)
                                .monospace()
                                .color(ui.visuals().text_color()),
                        );
                        ui.end_row();
                    }
                });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new(format!("Restore {}", row.created)),
                    )
                    .clicked()
                {
                    self.submit(Job::Restore {
                        source: RestoreSource::Snapshot(row.id.clone()),
                    });
                }
                if ui.button("Open backup folder").clicked() {
                    worker::open_backup_folder();
                }
                if ui.small_button("Close").clicked() {
                    self.history_open = None;
                }
            });
        });
        ui.add_space(6.0);
        change_table(
            ui,
            "SETTING",
            "BEFORE",
            "AFTER",
            &changes,
            "No differences.",
        );
    }

    /// A vs B: any two entries, in either order, not just neighbours.
    fn history_compare(&mut self, ui: &mut egui::Ui) {
        if self.history_cmp.is_empty() {
            return;
        }
        let dark = ui.visuals().dark_mode;
        section(ui, "Comparison");

        let a_id = self.history_cmp.first().cloned().unwrap_or_default();
        let b_id = self.history_cmp.get(1).cloned();
        let a_row = self.history_rows.iter().find(|r| r.id == a_id).cloned();

        panel_card(ui, accent(Mode::Quiet, dark), |ui| {
            ui.horizontal(|ui| {
                // Both arms end in `;`, so each evaluates to `()`. A bare
                // `ui.label(..)` without one returns `Response` and the two
                // arms stop agreeing on a type.
                match &a_row {
                    Some(a) => {
                        ui.label(
                            RichText::new(format!("A  {}  \u{2022}  {}", a.created, a.event))
                                .size(12.0)
                                .strong()
                                .color(ui.visuals().text_color()),
                        );
                    }
                    None => {
                        ui.label(
                            RichText::new("A  \u{2014}")
                                .size(12.0)
                                .color(ui.visuals().weak_text_color()),
                        );
                    }
                }
                match &b_id {
                    Some(b) => {
                        let row = self.history_rows.iter().find(|r| &r.id == b);
                        let label = row
                            .map(|r| format!("{}  \u{2022}  {}", r.created, r.event))
                            .unwrap_or_else(|| b.clone());
                        ui.label(
                            RichText::new(format!("B  {label}"))
                                .size(12.0)
                                .strong()
                                .color(ui.visuals().text_color()),
                        );
                    }
                    None => {
                        ui.label(
                            RichText::new("B  \u{2014} pick a second entry with the B button")
                                .size(12.0)
                                .color(ui.visuals().weak_text_color()),
                        );
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("Clear marks").clicked() {
                        self.history_cmp.clear();
                        self.history_cmp_diff = None;
                    }
                });
            });
        });

        let Some(diff) = self.history_cmp_diff.clone() else {
            return;
        };
        ui.add_space(6.0);
        let count = diff.len();
        let headline = if count == 0 {
            "Both entries hold identical values.".to_string()
        } else {
            format!(
                "{count} setting{} differ between A and B.",
                if count == 1 { "" } else { "s" }
            )
        };
        ui.label(
            RichText::new(headline)
                .size(12.0)
                .color(if count == 0 {
                    ok_color()
                } else {
                    ui.visuals().text_color()
                }),
        );
        change_table(ui, "SETTING", "A", "B", &diff, "No differences.");
    }

    /// Diffs the two marked entries and caches the result, so the panel costs
    /// no file IO per frame. Returns early (and clears the cache) unless both
    /// slots are filled.
    fn recompute_compare(&mut self) {
        self.history_cmp_diff = None;
        let (Some(a_id), Some(b_id)) = (self.history_cmp.first().cloned(), self.history_cmp.get(1).cloned())
        else {
            return;
        };
        let read = |rows: &[HistoryRow], id: &str| {
            rows.iter()
                .find(|r| r.id == id)
                .and_then(|r| std::fs::read_to_string(&r.path).ok())
                .map(|t| backup::parse_backup(&t))
                .unwrap_or_default()
        };
        let (a, b) = (
            read(&self.history_rows, &a_id),
            read(&self.history_rows, &b_id),
        );
        self.history_cmp_diff = Some(diff_details(&a, &b, &setting_labels()));
    }

    // ------------------------------------------------------------- settings --

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        let tone = accent(Mode::Quiet, dark);
        ui.add_space(4.0);
        let mut changed = false;

        section(ui, "Launch");
        panel_card(ui, tone, |ui| {
            let before = self.settings.start_with_system;
            ui.checkbox(&mut self.settings.start_with_system, "Start with Windows");
            if before != self.settings.start_with_system {
                changed = true;
                // Act on it straight away rather than waiting for "Save". This
                // writes the app's own entry and nothing else.
                match startup::set_self_autostart(self.settings.start_with_system) {
                    Ok(_) => self.note(
                        if self.settings.start_with_system {
                            "will start with Windows"
                        } else {
                            "no longer starts with Windows"
                        },
                        NoteKind::Ok,
                    ),
                    Err(e) => self.note(format!("could not update start-up: {e}"), NoteKind::Err),
                }
            }
            ui.label(
                RichText::new(
                    "This adds and removes only this app's own entry. Applying a mode never \
                     adds, removes, delays or rewrites another program's startup entry - what \
                     you have set up in Task Manager is left alone.",
                )
                .size(11.5)
                .color(ui.visuals().weak_text_color()),
            );
        });

        ui.add_space(10.0);
        section(ui, "Power watching");
        panel_card(ui, tone, |ui| {
            ui.checkbox(
                &mut self.settings.auto_switch_on_power_change,
                "Re-apply automatically when the charger changes",
            );
            ui.label(
                RichText::new(
                    "Runs quietly in the tray: unplugging the charger re-applies the battery \
                     values, plugging it back in restores the mains values.",
                )
                .size(11.5)
                .color(ui.visuals().weak_text_color()),
            );
            ui.add_space(4.0);
            ui.checkbox(
                &mut self.settings.notify_on_auto_switch,
                "Show a notification after an automatic change",
            );
        });

        ui.add_space(10.0);
        section(ui, "Behaviour");
        panel_card(ui, tone, |ui| {
            ui.checkbox(
                &mut self.settings.confirm_before_apply,
                "Ask before applying a mode",
            );
            ui.checkbox(
                &mut self.settings.control_fans,
                "Control fans through the XMG fan app",
            );
            ui.checkbox(
                &mut self.settings.control_gpu_limit,
                "Control the NVIDIA power limit",
            );
            ui.checkbox(
                &mut self.settings.control_display,
                "Control refresh rate (60 Hz / max) and brightness (70% / 100%)",
            );
        });

        ui.add_space(10.0);
        section(ui, "Appearance");
        panel_card(ui, tone, |ui| {
            ui.horizontal(|ui| {
                ui.label("Theme");
                for t in [Theme::System, Theme::Light, Theme::Dark] {
                    if ui
                        .selectable_label(self.settings.theme == t, t.label())
                        .clicked()
                    {
                        self.settings.theme = t;
                        changed = true;
                    }
                }
            });
        });

        ui.add_space(10.0);
        section(ui, "Elevation");
        panel_card(ui, tone, |ui| {
            if self.elevated_ready {
                ui.label(
                    RichText::new("Background task registered")
                        .strong()
                        .size(13.0)
                        .color(ui.visuals().text_color()),
                );
                ui.label(
                    RichText::new("Applying a mode does not prompt for administrator rights.")
                        .size(11.5)
                        .color(ui.visuals().weak_text_color()),
                );
            } else {
                ui.label(
                    RichText::new("Administrator rights needed once")
                        .strong()
                        .size(13.0)
                        .color(ui.visuals().text_color()),
                );
                ui.label(
                    RichText::new(
                        "Power settings can only be changed by an elevated process. One \
                         confirmation now avoids a prompt on every click later.",
                    )
                    .size(11.5)
                    .color(ui.visuals().weak_text_color()),
                );
                if ui.button("Set up now").clicked() {
                    match elevate::ensure_setup() {
                        Ok(true) => {
                            self.elevated_ready = true;
                            self.note("background task registered", NoteKind::Ok);
                        }
                        Ok(false) => self.note("setup incomplete", NoteKind::Warn),
                        Err(e) => self.note(e, NoteKind::Err),
                    }
                }
            }
        });

        ui.add_space(10.0);
        section(ui, "Safety");
        self.safety_panel(ui);

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let save = ui.button("Save settings").clicked();
            if (save || changed) && self.settings.save().is_err() {
                self.note("could not save settings", NoteKind::Err);
            }
            if ui.button("Quit").clicked() {
                self.stop_watch.store(true, Ordering::Relaxed);
                self.temps_stop.store(true, Ordering::Relaxed);
                self.quit_requested = true;
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ui.label(
                RichText::new(format!(
                    "elevated: {}  \u{2022}  launched: {}",
                    if sysutil::is_elevated() { "yes" } else { "no (normal)" },
                    if self.autostarted { "by Windows" } else { "by you" }
                ))
                .size(11.5)
                .color(ui.visuals().weak_text_color()),
            );
        });
    }
}

/// The neutral fill for an unmarked A/B button: quiet enough to read as
/// secondary next to the filled, accent-coloured marked state.
fn else_fill(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_gray(48)
    } else {
        egui::Color32::from_gray(238)
    }
}

/// A three-column table of before→after pairs, used by the entry detail and
/// the A/B comparison so the two never render differently.
fn change_table(
    ui: &mut egui::Ui,
    head_a: &str,
    head_b: &str,
    head_c: &str,
    changes: &[(String, String, String)],
    empty_text: &str,
) {
    if changes.is_empty() {
        ui.label(
            RichText::new(empty_text)
                .size(11.5)
                .color(ui.visuals().weak_text_color()),
        );
        return;
    }
    egui::Grid::new("change-table")
        .num_columns(3)
        .spacing([18.0, 3.0])
        .show(ui, |ui| {
            for head in [head_a, head_b, head_c] {
                ui.label(
                    RichText::new(head)
                        .size(10.5)
                        .strong()
                        .color(ui.visuals().weak_text_color()),
                );
            }
            ui.end_row();
            for (label, before, after) in changes {
                ui.label(RichText::new(label.clone()).size(11.5));
                ui.label(
                    RichText::new(before.clone())
                        .size(11.5)
                        .monospace()
                        .color(ui.visuals().weak_text_color()),
                );
                ui.label(
                    RichText::new(after.clone())
                        .size(11.5)
                        .monospace()
                        .color(ui.visuals().text_color()),
                );
                ui.end_row();
            }
        });
}
