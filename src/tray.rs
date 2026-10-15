//! Notification-area icon and its menu.
//!
//! `muda` menu items are built on `Rc` and are therefore neither `Send` nor
//! `Sync`, so they cannot be moved onto a helper thread. Instead the menu is
//! polled from the main loop via [`Tray::poll`], which the app calls from
//! `eframe::App::logic` - that hook keeps running while the window is hidden,
//! so the tray stays fully functional in the background.

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

/// A live tray icon plus the menu items it dispatches from.
pub struct Tray {
    icon: TrayIcon,
    show: MenuItem,
    quiet: MenuItem,
    performance: MenuItem,
    restore: MenuItem,
    quit: MenuItem,
}

impl Tray {
    /// Non-blocking menu pump. Returns every command raised since last time.
    pub fn poll(&self) -> Vec<TrayCommand> {
        let mut out = Vec::new();
        // Drain the global receiver; it is shared by every tray instance, so
        // unknown ids are simply ignored rather than treated as errors.
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id;
            if id == self.show.id() {
                out.push(TrayCommand::Show);
            } else if id == self.quiet.id() {
                out.push(TrayCommand::Apply(Mode::Quiet));
            } else if id == self.performance.id() {
                out.push(TrayCommand::Apply(Mode::Performance));
            } else if id == self.restore.id() {
                out.push(TrayCommand::Restore);
            } else if id == self.quit.id() {
                out.push(TrayCommand::Quit);
            }
        }
        out
    }

    /// Refreshes the tooltip to reflect the current mode.
    ///
    /// The icon itself is mode-independent - it is a flat violet plate - so
    /// there is nothing to redraw here. The mode is still announced by the
    /// tooltip, which is where a user actually looks.
    pub fn set_mode(&self, mode: Mode) {
        self.icon
            .set_tooltip(Some(format!("XMG Optimizer - {}", mode.label())));
    }

    /// Swaps the tray icon to match the theme.
    ///
    /// This is the only app icon that can change while the app is running.
    /// egui 0.36 has no runtime window-icon setter, and the one compiled into
    /// the executable by `build.rs` is a build-time resource that Explorer,
    /// the Start menu and a pinned taskbar button all read from the file.
    pub fn set_dark(&self, dark: bool) {
        if let Some(i) = icon::tray_icon(dark) {
            let _ = self.icon.set_icon(Some(i));
        }
    }
}

/// Builds the tray icon. `None` when the notification area is unavailable.
pub fn build(mode: Mode, on_ac: bool, dark: bool) -> Option<Tray> {
    let menu = Menu::new();

    let head = MenuItem::new(
        format!(
            "XMG Optimizer - {} ({})",
            mode.label(),
            if on_ac { "mains" } else { "battery" }
        ),
        false,
        None,
    );
    let show = MenuItem::new("Open", true, None);
    let quiet = MenuItem::new("Apply Quiet", true, None);
    let performance = MenuItem::new("Apply Performance", true, None);
    let restore = MenuItem::new("Restore settings...", true, None);
    let quit = MenuItem::new("Quit", true, None);

    menu.append(&head).ok()?;
    menu.append(&PredefinedMenuItem::separator()).ok()?;
    menu.append(&show).ok()?;
    menu.append(&quiet).ok()?;
    menu.append(&performance).ok()?;
    menu.append(&restore).ok()?;
    menu.append(&PredefinedMenuItem::separator()).ok()?;
    menu.append(&quit).ok()?;

    let icon_handle = icon::tray_icon(dark)?;

    let built = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(format!("XMG Optimizer - {}", mode.label()))
        .with_icon(icon_handle)
        .build()
        .ok()?;

    Some(Tray {
        icon: built,
        show,
        quiet,
        performance,
        restore,
        quit,
    })
}
