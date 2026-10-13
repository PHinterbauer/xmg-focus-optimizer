//! The window.
//!
//! Frameless with custom title-bar controls, so "close" can mean "collapse into
//! the tray" rather than "quit" - which is what keeps the power watcher alive.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use egui::{Align, CornerRadius, Layout, RichText, Stroke, Vec2, Widget};

use crate::backup::{self, ArchiveEntry, RestoreSource, SnapshotInfo};
use crate::config::{Settings, Theme};
use crate::elevate;
use crate::icon;
use crate::jobs::{self, Job, JobResult};
use crate::monitor::MonitorEvent;
use crate::power;
use crate::profile::{self, Mode};
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
    settings: Settings,
    tab: Tab,
    on_ac: bool,
    elevated_ready: bool,

    busy: bool,
    notes: Vec<Note>,

    snapshots: Vec<SnapshotInfo>,
    archives: Vec<ArchiveEntry>,
    backup_dir: PathBuf,
    has_factory: bool,

    /// Multi-step confirmation for creating a new initial backup.
    confirm_stage: u8,
    confirm_note: String,

    theme_applied: Option<Theme>,
    tray_alive: bool,

    // channels
    tray_rx: Receiver<TrayCommand>,
    monitor_rx: Receiver<MonitorEvent>,
    tx_tray: Sender<TrayCommand>,
    _tx_monitor: Sender<MonitorEvent>,
    stop_watch: Arc<AtomicBool>,
}

impl App {
    pub fn new(
        settings: Settings,
        tray_rx: Receiver<TrayCommand>,
        monitor_rx: Receiver<MonitorEvent>,
        tx_tray: Sender<TrayCommand>,
        tx_monitor: Sender<MonitorEvent>,
        stop_watch: Arc<AtomicBool>,
        start_hidden: bool,
    ) -> Self {
        let on_ac = power::on_ac_power();
        let (snapshots, archives, backup_dir, has_factory) = self::state().unwrap_or_default();

        let mut app = App {
            settings,
            tab: Tab::Dashboard,
            on_ac,
            elevated_ready: elevate::task_exists(),
            busy: false,
            notes: Vec::new(),
            snapshots,
            archives,
            backup_dir,
            has_factory,
            confirm_stage: 0,
            confirm_note: String::new(),
            theme_applied: None,
            tray_alive: false,
            tray_rx,
            monitor_rx,
            tx_tray,
            _tx_monitor: tx_monitor,
            stop_watch: stop_watch,
        };
        if start_hidden {
            app.hide_to_tray();
        }
        app.refresh();
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
        self.on_ac = power::on_ac_power();
    }

    fn hide_to_tray(&mut self) {
        let ctx = egui::Context::current();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
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
    }

    fn submit(&mut self, job: Job) {
        if self.busy {
            self.note("still working on the previous request", NoteKind::Warn);
            return;
        }
        self.busy = true;
        elevate::submit(job);
    }

    // ------------------------------------------------------------- events --

    fn poll_events(&mut self, ctx: &egui::Context) {
        while let Ok(cmd) = self.tray_rx.try_recv() {
            match cmd {
                TrayCommand::Show => self.show_window(ctx),
                TrayCommand::Apply(m) => self.settings.mode = m,
                TrayCommand::Restore => {
                    self.tab = Tab::Backups;
                    self.show_window(ctx);
                }
                TrayCommand::Quit => {
                    self.stop_watch
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        while let Ok(ev) = self.monitor_rx.try_recv() {
            match ev {
                MonitorEvent::PowerChanged { on_ac, mode, triggered } => {
                    self.on_ac = on_ac;
                    if triggered {
                        self.note(
                            format!(
                                "power source changed to {} - re-applied {mode}",
                                if on_ac { "mains" } else { "battery" }
                            ),
                            NoteKind::Ok,
                        );
                    }
                }
                MonitorEvent::Error(e) => self.note(e, NoteKind::Err),
            }
        }

        if self.busy {
            if let Some(r) = jobs::take_result() {
                self.busy = false;
                self.push_result(r);
            }
        }
    }
}

// ---------------------------------------------------------------- painting --

fn accent(mode: Mode, dark: bool) -> egui::Color32 {
    match (mode, dark) {
        (Mode::Quiet, true) => egui::Color32::from_rgb(56, 189, 248),
        (Mode::Quiet, false) => egui::Color32::from_rgb(29, 78, 216),
        (Mode::Performance, true) => egui::Color32::from_rgb(251, 146, 60),
        (Mode::Performance, false) => egui::Color32::from_rgb(194, 65, 12),
    }
}

/// A selectable card. Returns true when clicked.
fn card<R>(
    ui: &mut egui::Ui,
    selected: bool,
    tone: egui::Color32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> bool {
    let dark = ui.visuals().dark_mode;
    let (fill, stroke) = if selected {
        (
            egui::Color32::from_black_alpha(if dark { 60 } else { 16 }),
            Stroke::new(1.5, tone),
        )
    } else {
        (
            ui.visuals().panel_fill,
            Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color()),
        )
    };

    let resp = egui::Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(14.0)
        .stroke(stroke)
        .show(ui, add_contents);

    resp.interact(egui::Sense::click())
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
    let c = if good {
        egui::Color32::from_rgb(34, 160, 90)
    } else {
        egui::Color32::from_rgb(190, 140, 20)
    };
    egui::Frame::new()
        .fill(c.gamma_multiply(0.18))
        .corner_radius(CornerRadius::same(20))
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(c));
        });
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // theme ------------------------------------------------------------
        let want = match self.settings.theme {
            Theme::System => None,
            Theme::Light => Some(egui::Visuals::light()),
            Theme::Dark => Some(egui::Visuals::dark()),
        };
        if self.theme_applied != self.settings.theme {
            if let Some(v) = want {
                ctx.set_visuals(v);
            } else {
                let sys = ctx.style().visuals.clone();
                ctx.set_visuals(if sys.dark_mode {
                    egui::Visuals::dark()
                } else {
                    egui::Visuals::light()
                });
            }
            self.theme_applied = self.settings.theme;
        }

        self.poll_events(ctx);
        self.ensure_tray();

        let dark = ctx.style().visuals.dark_mode;
        let tone = accent(self.settings.mode, dark);

        egui::TopBottomPanel::top("titlebar")
            .frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill))
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::new(&icon::texture(ctx, self.settings.mode))
                            .fit_to_exact_size(Vec2::new(24.0, 24.0)),
                    );
                    ui.label(
                        RichText::new("XMG Optimizer")
                            .size(15.0)
                            .strong(),
                    );
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
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                        // draggable empty area
                        let (rect, resp) = ui.allocate_exact_size(
                            egui::available_size(),
                            egui::Sense::drag(),
                        );
                        if resp.dragged() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                        }
                        let _ = rect;
                    });
                });
                ui.add_space(4.0);
            });

        egui::TopBottomPanel::top("tabs")
            .frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (t, label) in [
                        (Tab::Dashboard, "Dashboard"),
                        (Tab::Backups, "Backups"),
                        (Tab::Settings, "Settings"),
                    ] {
                        let sel = self.tab == t;
                        let c = if sel { tone } else { ctx.style().visuals.weak_text_color() };
                        let resp = ui
                            .add(
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
                        } else {
                            if ui.button("Finish setup").clicked() {
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
                });
                ui.add_space(4.0);
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(ctx.style().visuals.weak_bg_color()))
            .show(ctx, |ui| match self.tab {
                Tab::Dashboard => self.dashboard(ui),
                Tab::Backups => self.backups(ui, ctx),
                Tab::Settings => self.settings_tab(ui),
            });

        if !self.notes.is_empty() {
            egui::TopBottomPanel::bottom("log")
                .frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill))
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(150.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            for n in self.notes.iter().rev() {
                                let c = match n.kind {
                                    NoteKind::Ok => ctx.style().visuals.weak_text_color(),
                                    NoteKind::Warn => egui::Color32::from_rgb(220, 160, 40),
                                    NoteKind::Err => egui::Color32::from_rgb(226, 80, 80),
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
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_watch
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl App {
    fn ensure_tray(&mut self) {
        if !self.tray_alive {
            if let Some(t) = tray::build(self.settings.mode, self.on_ac, self.tx_tray.clone()) {
                // keep it alive for the process lifetime
                std::mem::forget(t);
                self.tray_alive = true;
            }
        }
    }

    // ------------------------------------------------------------ dashboard --

    fn dashboard(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        let on_ac = self.on_ac;
        let (params, entries) = profile::plan(self.settings.mode, on_ac);

        ui.add_space(8.0);
        ui.columns(2, |cols| {
            for (i, mode) in [Mode::Quiet, Mode::Performance].iter().enumerate() {
                let selected = self.settings.mode == *mode;
                let tone = accent(*mode, dark);
                cols[i].add_space(4.0);
                let clicked = card(&mut cols[i], selected, tone, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::Image::new(&icon::texture(ui.ctx(), *mode))
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
                        RichText::new(mode.blurb()).size(12.5).color(ui.visuals().weak_text_color()),
                    );
                    ui.add_space(10.0);
                    let p = profile::Params::for_mode(*mode, on_ac);
                    ui.label(
                        RichText::new(format!(
                            "CPU cap {}%   •   efficiency {}   •   GPU {} W\nFans {}   •   noiseless {}",
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
                    if let Some(t) = tray_icon::TrayIcon::find_by_id(&"") {
                        let _ = t;
                    }
                }
            }
        });

        ui.add_space(14.0);
        let apply_label = format!("Apply {}", self.settings.mode.label());
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(RichText::new(apply_label).size(14.0).strong().color(egui::Color32::WHITE))
                        .fill(accent(self.settings.mode, dark))
                        .corner_radius(CornerRadius::same(10))
                        .min_size(Vec2::new(200.0, 40.0)),
                )
                .clicked()
            {
                let s = self.settings.clone();
                self.submit(Job::Apply {
                    mode: s.mode,
                    control_fans: s.control_fans,
                    control_gpu: s.control_gpu_limit,
                });
            }
            if ui.button("Refresh").clicked() {
                self.refresh();
            }
            ui.label(
                RichText::new(format!("{} settings will be written", entries.len()))
                    .size(12.0)
                    .color(ui.visuals().weak_text_color()),
            );
        });

        ui.add_space(10.0);
        section(ui, "What is active right now");
        let actual = power::read_all(&power::active_scheme().unwrap_or_default());
        egui::Grid::new("active").num_columns(3).spacing([16.0, 4.0]).show(ui, |ui| {
            for e in entries.iter().take(10) {
                let v = actual.get(&e.setting.to_ascii_lowercase());
                ui.label(RichText::new(e.label).size(12.0));
                ui.label(
                    RichText::new(match v {
                        Some(v) if v.ac == 0 => "off".into(),
                        Some(v) => v.ac.to_string(),
                        None => "?".into(),
                    })
                    .size(12.0)
                    .monospace(),
                );
                ui.label(
                    RichText::new(match v {
                        Some(v) if v.ac != e.ac || v.dc != e.dc => {
                            format!("expected {} / {}", e.ac, e.dc)
                        }
                        _ => "matches".into(),
                    })
                    .size(11.5)
                    .color(ui.visuals().weak_text_color()),
                );
                ui.end_row();
            }
        });

        ui.add_space(10.0);
        section(ui, "Safety");
        self.safety_panel(ui);
    }

    fn safety_panel(&self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        egui::Frame::new()
            .fill(egui::Color32::from_rgb(34, 150, 90).gamma_multiply(if dark { 0.16 } else { 0.10 }))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(12.0)
            .show(ui, |ui| {
                ui.label(RichText::new("This app never:").strong().size(12.5));
                for line in [
                    "overclocks or undervolts anything",
                    "disables HVCI / VBS or other security features",
                    "touches the BIOS or flashes firmware",
                    "writes undocumented EC or driver binaries",
                ] {
                    ui.label(
                        RichText::new(format!("  •  {line}"))
                            .size(12.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                }
            });
    }

    // -------------------------------------------------------------- backups --

    fn backups(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        let dark = ui.visuals().dark_mode;
        ui.add_space(8.0);

        egui::Frame::new()
            .fill(egui::Color32::from_rgb(34, 150, 90).gamma_multiply(if dark { 0.15 } else { 0.10 }))
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

        // --- initial backup ------------------------------------------------
        section(ui, "Initial backup");
        ui.horizontal(|ui| {
            card(ui, false, egui::Color32::from_rgb(34, 150, 90), |ui| {
                if self.has_factory {
                    ui.label(RichText::new("Present").strong().size(15.0));
                    ui.label(
                        RichText::new(format!(
                            "{} archived copies",
                            self.archives.len()
                        ))
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
            card(ui, false, egui::Color32::from_rgb(200, 140, 40), |ui| {
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

        // staged confirmation
        if self.confirm_stage > 0 {
            ui.add_space(8.0);
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(200, 120, 40).gamma_multiply(if dark { 0.14 } else { 0.10 }))
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
                                egui::Button::new(RichText::new("Create new initial backup").strong()),
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

        ui.add_space(14.0);

        // --- snapshots -----------------------------------------------------
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

        egui::ScrollArea::vertical()
            .max_height(300.0)
            .show(ui, |ui| {
                if self.snapshots.is_empty() {
                    ui.label(
                        RichText::new("No snapshots yet.")
                            .color(ui.visuals().weak_text_color()),
                    );
                }
                for s in self.snapshots.iter() {
                    ui.horizontal(|ui| {
                        let kind = match s.kind {
                            backup::SnapshotKind::Automatic => "auto",
                            backup::SnapshotKind::Manual => "manual",
                        };
                        ui.label(RichText::new(format!("{}  {}", s.created, s.label)).size(12.0));
                        ui.label(
                            RichText::new(kind)
                                .size(11.0)
                                .color(ui.visuals().weak_text_color()),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("Restore").clicked() {
                                self.submit(Job::Restore {
                                    source: RestoreSource::Snapshot(s.id.clone()),
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
            for a in self.archives.iter() {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{}  {}", a.id, a.note))
                            .size(12.0)
                            .monospace(),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Restore").clicked() {
                            self.submit(Job::Restore {
                                source: RestoreSource::Archive(a.id.clone()),
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
        ui.add_space(8.0);

        let mut changed = false;

        section(ui, "Startup");
        card(ui, false, accent(Mode::Quiet, dark), |ui| {
            ui.checkbox(
                &mut self.settings.start_with_system,
                "Start with Windows",
            );
            ui.label(
                RichText::new("Registers a hidden launch at sign-in so the tray icon is there.")
                    .size(11.5)
                    .color(ui.visuals().weak_text_color()),
            );
            ui.add_space(6.0);
            ui.checkbox(
                &mut self.settings.start_minimized_to_tray,
                "Collapse into the tray on start",
            );
            ui.label(
                RichText::new(
                    "Opens straight into the notification area instead of a window.",
                )
                .size(11.5)
                .color(ui.visuals().weak_text_color()),
            );
        });

        ui.add_space(10.0);
        section(ui, "Power watching");
        card(ui, false, accent(Mode::Quiet, dark), |ui| {
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
        card(ui, false, accent(Mode::Quiet, dark), |ui| {
            ui.checkbox(&mut self.settings.confirm_before_apply, "Ask before applying a mode");
            ui.checkbox(&mut self.settings.control_fans, "Control fans through the XMG fan app");
            ui.checkbox(&mut self.settings.control_gpu_limit, "Control the NVIDIA power limit");
        });

        ui.add_space(10.0);
        section(ui, "Appearance");
        card(ui, false, accent(Mode::Quiet, dark), |ui| {
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
        card(ui, false, accent(Mode::Quiet, dark), |ui| {
            if self.elevated_ready {
                ui.label(
                    RichText::new("Background task registered").strong().size(13.0),
                );
                ui.label(
                    RichText::new(
                        "Applying a mode does not prompt for administrator rights.",
                    )
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
            if ui.button("Save settings").clicked() || changed {
                if let Err(e) = self.settings.save() {
                    self.note(format!("could not save settings: {e}"), NoteKind::Err);
                }
            }
            if ui.button("Quit").clicked() {
                self.stop_watch
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ui.label(
                RichText::new(format!(
                    "elevated: {}",
                    if sysutil::is_elevated() { "yes" } else { "no (normal)" }
                ))
                .size(11.5)
                .color(ui.visuals().weak_text_color()),
            );
        });
    }
}