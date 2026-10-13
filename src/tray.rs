//! Notification-area icon and its menu.

use std::sync::mpsc::Sender;

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::TrayIcon;
use tray_icon::TrayIconBuilder;

use crate::icon;
use crate::profile::Mode;

#[derive(Debug, Clone)]
pub enum TrayCommand {
    Show,
    Apply(Mode),
    Restore,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
}

pub fn build(mode: Mode, on_ac: bool, tx: Sender<TrayCommand>) -> Option<Tray> {
    let menu = Menu::new();

    let head = MenuItem::new(format!(
        "XMG Optimizer - {} ({})",
        mode.label(),
        if on_ac { "mains" } else { "battery" }
    ));
    let show = MenuItem::new("Open");
    let quiet = MenuItem::new("Apply Quiet");
    let perf = MenuItem::new("Apply Performance");
    let restore = MenuItem::new("Restore settings...");
    let sep = PredefinedMenuItem::separator();
    let quit = MenuItem::new("Quit");

    menu.append(&head).ok()?;
    menu.append(&sep).ok()?;
    menu.append(&show).ok()?;
    menu.append(&quiet).ok()?;
    menu.append(&perf).ok()?;
    menu.append(&restore).ok()?;
    menu.append(&PredefinedMenuItem::separator()).ok()?;
    menu.append(&quit).ok()?;

    let builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(format!("XMG Optimizer - {}", mode.label()))
        .with_icon(icon::tray_icon(mode)?);

    let tray = builder.build().ok()?;

    // Route menu clicks back to the app.
    std::thread::spawn(move || loop {
        if let Ok(event) = MenuEvent::receiver().recv() {
            let cmd = match event.id.as_ref() {
                id if id == show.id().as_ref() => Some(TrayCommand::Show),
                id if id == quiet.id().as_ref() => Some(TrayCommand::Apply(Mode::Quiet)),
                id if id == perf.id().as_ref() => Some(TrayCommand::Apply(Mode::Performance)),
                id if id == restore.id().as_ref() => Some(TrayCommand::Restore),
                id if id == quit.id().as_ref() => Some(TrayCommand::Quit),
                _ => None,
            };
            if let Some(c) = cmd {
                if tx.send(c).is_err() {
                    break;
                }
            }
        }
    });

    Some(Tray { _icon: tray })
}

/// Swaps the tray icon to reflect the active mode.
pub fn set_mode(tray: &TrayIcon, mode: Mode) {
    if let Some(i) = icon::tray_icon(mode) {
        tray.set_icon(Some(i));
    }
    tray.set_tooltip(Some(format!("XMG Optimizer - {}", mode.label())));
}