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
    Settings,
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

pub struct App {
    /// egui is cheap to clone, and holding it lets helpers send viewport
    /// commands before the first frame has been painted.
    ctx: egui::Context,

    settings: Settings,
    tab: Tab,
    on_ac: bool,
    elevated_ready: bool,

    busy: bool,
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

        let mut app = App {
            ctx,
            settings,
            tab: Tab::Dashboard,
            on_ac,
            elevated_ready: elevate::task_exists(),
            busy: false,
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
        };

        if start_hidden {
            app.hide_to_tray();
        }
        app.refresh();
        // Reconciles this app's own Run entry with the stored preference, and
        // nothing else. Another program's startup entry is never touched.
        app.reconcile_autostart();
        app.recheck_drift();

        // Launched by Windows: silently put the saved mode back in place.
        // Because applying is idempotent this normally changes nothing at all.
        if autostarted {
            app.submit_now(Job::Apply {
                mode: app.settings.mode,
                control_fans: app.settings.control_fans,
                control_gpu: app.settings.control_gpu_limit,
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
        self.drift = Some(drift::check(self.settings.mode, self.on_ac));
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
        }
        for w in &r.warnings {
            self.note(format!("  ! {w}"), NoteKind::Warn);
        }
        self.refresh();
        self.recheck_drift();
    }

    fn submit_now(&mut self, job: Job) {
        self.busy = true;
        elevate::submit(job);
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
        self.submit(Job::Apply {
            mode: s.mode,
            control_fans: s.control_fans,
            control_gpu: s.control_gpu_limit,
        });
    }

    fn ensure_tray(&mut self) {
        if self.tray.is_none() {
            self.tray = tray::build(self.settings.mode, self.on_ac);
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
                    self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        while let Ok(MonitorEvent::PowerChanged { on_ac, mode, triggered }) =
            self.monitor_rx.try_recv()
        {
            self.on_ac = on_ac;
            if triggered {
                self.note(
                    format!(
                        "power source changed to {} - re-applied {mode}",
                        if on_ac { "mains" } else { "battery" }
                    ),
                    NoteKind::Ok,
                );
                self.drift_acknowledged = false;
            }
            self.recheck_drift();
        }

        if self.busy {
            if let Some(r) = jobs::take_result() {
                self.busy = false;
                self.push_result(r);
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

        // --- theme ---------------------------------------------------------
        //
        // Both palettes are installed every time, and the preference is set
        // afterwards. Doing it this way means "System" resolves to whichever of
        // ours the OS asked for, instead of falling back to egui's stock look.
        if self.theme_applied != Some(self.settings.theme) || !self.style_pushed {
            ctx.set_visuals_of(egui::Theme::Light, crate::theme::visuals(false));
            ctx.set_visuals_of(egui::Theme::Dark, crate::theme::visuals(true));
            ctx.set_theme(match self.settings.theme {
                Theme::System => egui::ThemePreference::System,
                Theme::Light => egui::ThemePreference::Light,
                Theme::Dark => egui::ThemePreference::Dark,
            });
            self.theme_applied = Some(self.settings.theme);
            self.style_pushed = true;
        }

        self.poll_events();
        self.ensure_tray();

        let visuals = ui.visuals().clone();
        let dark = visuals.dark_mode;
        let tone = accent(self.settings.mode, dark);

        // --- title bar -----------------------------------------------------
        egui::Panel::top("titlebar")
            .frame(egui::Frame::new().fill(visuals.panel_fill).inner_margin(8.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::new(&icon::texture(ui.ctx()))
                            .fit_to_exact_size(Vec2::new(24.0, 24.0)),
                    );
                    ui.label(RichText::new("XMG Optimizer").size(15.0).strong());
                    ui.add_space(8.0);
                    pill(
                        ui,
                        if self.on_ac { "mains power" } else { "on battery" },
                        self.on_ac,
                    );
                    if self.busy {
                        ui.spinner();
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("x").on_hover_text("Close to tray").clicked() {
                            self.hide_to_tray();
                        }
                        if ui.button("-").on_hover_text("Minimise").clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                        let (_, resp) =
                            ui.allocate_exact_size(ui.available_size(), egui::Sense::drag());
                        if resp.dragged() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                        }
                    });
                });
            });

        // --- tabs ----------------------------------------------------------
        egui::Panel::top("tabs")
            .frame(egui::Frame::new().fill(visuals.panel_fill).inner_margin(6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (t, label) in [
                        (Tab::Dashboard, "Dashboard"),
                        (Tab::Backups, "Backups"),
                        (Tab::Settings, "Settings"),
                    ] {
                        let sel = self.tab == t;
                        let c = if sel {
                            tone
                        } else {
                            visuals.weak_text_color()
                        };
                        let resp = ui.add(
                            egui::Button::new(RichText::new(label).color(c).size(13.0))
                                .fill(if sel {
                                    tone.gamma_multiply(0.14)
                                } else {
                                    egui::Color32::TRANSPARENT
                                })
                                .corner_radius(CornerRadius::same(8))
                                .min_size(Vec2::new(96.0, 30.0)),
                        );
                        if resp.clicked() {
                            self.tab = t;
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if self.elevated_ready {
                            pill(ui, "background task ready", true);
                        } else if ui.button("Finish setup").clicked() {
                            match elevate::ensure_setup() {
                                Ok(true) => {
                                    self.elevated_ready = true;
                                    self.note("background task registered", NoteKind::Ok);
                                }
                                Ok(false) => self.note("setup incomplete", NoteKind::Warn),
                                Err(e) => self.note(e, NoteKind::Err),
                            }
                        }
                    });
                });
            });

        // --- body ----------------------------------------------------------
        egui::Panel::bottom("log")
            .frame(egui::Frame::new().fill(visuals.panel_fill).inner_margin(8.0))
            .resizable(true)
            .default_size(120.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for n in self.notes.iter().rev() {
                            let c = match n.kind {
                                NoteKind::Ok => visuals.weak_text_color(),
                                NoteKind::Warn => warn_color(),
                                NoteKind::Err => err_color(),
                            };
                            ui.label(
                                RichText::new(format!("{}  {}", n.stamp, n.text))
                                    .size(12.0)
                                    .color(c)
                                    .monospace(),
                            );
                        }
                    });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(visuals.panel_fill).inner_margin(12.0))
            .show(ui, |ui| match self.tab {
                Tab::Dashboard => self.dashboard(ui),
                Tab::Backups => self.backups(ui),
                Tab::Settings => self.settings_tab(ui),
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_watch.store(true, Ordering::Relaxed);
        let _ = self.settings.save();
    }
}

// ---------------------------------------------------------------- painting --

use crate::theme::{accent, card_bg, err_color, ok_color, surface, surface_stroke, warn_color};

fn card<R>(
    ui: &mut egui::Ui,
    selected: bool,
    tone: egui::Color32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> bool {
    let dark = ui.visuals().dark_mode;
    let (fill, stroke) = if selected {
        (
            tone.gamma_multiply(if dark { 0.22 } else { 0.10 }),
            Stroke::new(1.5, tone),
        )
    } else {
        // Cards sit a step above the panel rather than being the same colour,
        // so they read as raised even in a flat theme.
        (
            card_bg(dark),
            Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        )
    };

    let resp = egui::Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(14.0)
        .stroke(stroke)
        .show(ui, add_contents);

    resp.response.interact(egui::Sense::click()).clicked()
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
    egui::Frame::new()
        .fill(c.gamma_multiply(0.18))
        .corner_radius(CornerRadius::same(20))
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

        ui.columns(2, |cols| {
            for (i, mode) in [Mode::Quiet, Mode::Performance].iter().enumerate() {
                let selected = self.settings.mode == *mode;
                let tone = accent(*mode, dark);
                cols[i].add_space(4.0);
                let clicked = card(&mut cols[i], selected, tone, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::Image::new(&icon::texture(ui.ctx()))
                                .fit_to_exact_size(Vec2::new(28.0, 28.0)),
                        );
                        ui.vertical(|ui| {
                            ui.label(RichText::new(mode.label()).size(19.0).strong());
                            ui.label(
                                RichText::new(mode.tagline())
                                    .size(12.0)
                                    .color(ui.visuals().weak_text_color()),
                            );
                        });
                    });
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(mode.blurb())
                            .size(12.5)
                            .color(ui.visuals().weak_text_color()),
                    );
                    ui.add_space(10.0);
                    let p = profile::Params::for_mode(*mode, on_ac);
                    ui.label(
                        RichText::new(format!(
                            "CPU cap {}%   \u{2022}   efficiency {}   \u{2022}   GPU {} W\nFans {}   \u{2022}   noiseless {}",
                            p.cpu_max,
                            p.epp,
                            p.gpu_watts,
                            p.fan_mode.label(),
                            if p.noiseless { "on" } else { "off" }
                        ))
                        .size(12.0),
                    );
                });
                if clicked && !selected {
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

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(format!("Apply {}", self.settings.mode.label()))
                            .size(14.0)
                            .strong()
                            .color(egui::Color32::WHITE),
                    )
                    .fill(accent(self.settings.mode, dark))
                    .corner_radius(CornerRadius::same(10))
                    .min_size(Vec2::new(190.0, 40.0)),
                )
                .clicked()
            {
                self.apply_current_mode();
            }
            if ui.button("Check settings").clicked() {
                self.recheck_drift();
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

        // The drift report is a status, so it keeps semantic colours: violet when the
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
            .corner_radius(CornerRadius::same(10))
            .inner_margin(12.0)
            .show(ui, |ui| {
                if clean && managed {
                    ui.label(
                        RichText::new(format!(
                            "All {} settings match {}.",
                            report.total(),
                            report.mode.label()
                        ))
                        .strong()
                        .size(13.0),
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
                                .add(
                                    egui::Button::new(
                                        RichText::new(format!("Change to {}", report.mode.label()))
                                            .strong()
                                            .color(egui::Color32::WHITE),
                                    )
                                    .fill(tint)
                                    .corner_radius(CornerRadius::same(8))
                                    .min_size(Vec2::new(150.0, 32.0)),
                                )
                                .clicked()
                            {
                                self.apply_current_mode();
                            }
                            if ui
                                .add(
                                    egui::Button::new("Keep current settings")
                                        .corner_radius(CornerRadius::same(8))
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
                if ui
                    .add_enabled(
                        drifted > 0,
                        egui::Button::new(if self.drift_expanded {
                            "Hide details"
                        } else {
                            "Show all settings"
                        })
                        .small(),
                    )
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
                            ui.label(RichText::new("Setting").size(11.0).strong());
                            ui.label(RichText::new("Expected").size(11.0).strong());
                            ui.label(RichText::new("Actual").size(11.0).strong());
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
            .corner_radius(CornerRadius::same(10))
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.label(RichText::new("This app never:").strong().size(12.5));
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
            .corner_radius(CornerRadius::same(10))
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Backups are only ever added, never removed.")
                        .strong()
                        .size(13.0),
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
            card(ui, false, accent(Mode::Quiet, dark), |ui| {
                if self.has_factory {
                    ui.label(RichText::new("Present").strong().size(15.0));
                    ui.label(
                        RichText::new(format!("{} archived copies", self.archives.len()))
                            .size(12.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                } else {
                    ui.label(RichText::new("None yet").strong().size(15.0));
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
            card(ui, false, warn_color(), |ui| {
                ui.label(RichText::new("Create a new one").strong().size(15.0));
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
                .corner_radius(CornerRadius::same(10))
                .inner_margin(12.0)
                .show(ui, |ui| match self.confirm_stage {
                    1 => {
                        ui.label(
                            RichText::new("Step 1 of 3 - understand what this does").strong(),
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
                            RichText::new("Step 2 of 3 - name it so you can recognise it").strong(),
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
                        ui.label(RichText::new("Step 3 of 3 - final confirmation").strong());
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

        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            if rows.is_empty() {
                ui.label(
                    RichText::new("No snapshots yet.").color(ui.visuals().weak_text_color()),
                );
            }
            for (id, text, kind) in rows.iter() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(text.clone()).size(12.0));
                    ui.label(
                        RichText::new(*kind)
                            .size(11.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Restore").clicked() {
                            self.submit(Job::Restore {
                                source: RestoreSource::Snapshot(id.clone()),
                            });
                        }
                    });
                    ui.separator();
                });
            }
        });

        if !self.archives.is_empty() {
            ui.add_space(10.0);
            section(ui, "Archived initial backups");
            let arch: Vec<(String, String)> = self
                .archives
                .iter()
                .map(|a| (a.id.clone(), format!("{}  {}", a.id, a.note)))
                .collect();
            for (id, text) in arch.iter() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(text.clone()).size(12.0).monospace());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Restore").clicked() {
                            self.submit(Job::Restore {
                                source: RestoreSource::Archive(id.clone()),
                            });
                        }
                    });
                    ui.separator();
                });
            }
        }
    }

    // ------------------------------------------------------------- settings --

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        let tone = accent(Mode::Quiet, dark);
        ui.add_space(4.0);
        let mut changed = false;

        section(ui, "Launch");
        card(ui, false, tone, |ui| {
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
            ui.add_space(6.0);
            ui.checkbox(
                &mut self.settings.start_minimized_to_tray,
                "Collapse into the tray when opened",
            );
            ui.label(
                RichText::new("Starts straight in the notification area instead of a window.")
                    .size(11.5)
                    .color(ui.visuals().weak_text_color()),
            );
        });

        ui.add_space(10.0);
        section(ui, "Power watching");
        card(ui, false, tone, |ui| {
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
        card(ui, false, tone, |ui| {
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
        });

        ui.add_space(10.0);
        section(ui, "Appearance");
        card(ui, false, tone, |ui| {
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
        card(ui, false, tone, |ui| {
            if self.elevated_ready {
                ui.label(RichText::new("Background task registered").strong().size(13.0));
                ui.label(
                    RichText::new("Applying a mode does not prompt for administrator rights.")
                        .size(11.5)
                        .color(ui.visuals().weak_text_color()),
                );
            } else {
                ui.label(
                    RichText::new("Administrator rights needed once").strong().size(13.0),
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
