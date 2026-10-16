//! Persisted user settings.
//!
//! Every field has a safe default, and a corrupt or partially written file
//! falls back to defaults instead of refusing to start.

use serde::{Deserialize, Serialize};

use crate::errors::{AppError, AppResult};
use crate::paths;
use crate::profile::Mode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

impl Theme {
    pub fn label(self) -> &'static str {
        match self {
            Theme::System => "Follow system",
            Theme::Light => "Light",
            Theme::Dark => "Dark",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Currently selected optimisation mode.
    pub mode: Mode,

    /// Add this app to the Run key so it comes back at sign-in. This covers the
    /// app's own entry only - never another program's.
    pub start_with_system: bool,

    /// Close to the notification area instead of quitting, so the background
    /// power-state watcher keeps running.
    pub start_minimized_to_tray: bool,

    /// Watch for the charger being plugged in or removed and re-apply the
    /// matching settings automatically.
    pub auto_switch_on_power_change: bool,

    /// Show a short notification after an automatic re-apply.
    pub notify_on_auto_switch: bool,

    /// Ask before applying a mode (extra safety, one click more).
    pub confirm_before_apply: bool,

    /// Manage fans through the vendor's own app.
    pub control_fans: bool,

    /// Manage the NVIDIA power limit.
    pub control_gpu_limit: bool,

    pub theme: Theme,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::Quiet,
            // Off by default: a plain double-click must open the window. Hiding
            // on launch used to be the default, which is why the window flashed
            // open and dropped straight to the tray on first run.
            start_with_system: true,
            start_minimized_to_tray: false,
            auto_switch_on_power_change: true,
            notify_on_auto_switch: false,
            confirm_before_apply: false,
            control_fans: true,
            control_gpu_limit: true,
            theme: Theme::System,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let path = paths::config_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Settings::default();
        };
        match serde_json::from_str::<Settings>(&text) {
            Ok(s) => s,
            Err(_) => {
                // Keep the unreadable file around for diagnosis, then continue
                // with defaults rather than blocking startup.
                let _ = std::fs::rename(&path, path.with_extension("json.corrupt"));
                Settings::default()
            }
        }
    }

    pub fn save(&self) -> AppResult<()> {
        let dir = paths::app_data_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| AppError::Io(format!("{}: {e}", dir.display())))?;

        let path = paths::config_path();
        let tmp = path.with_extension("json.tmp");

        let text = serde_json::to_string_pretty(self)
            .map_err(|e| AppError::Config(e.to_string()))?;
        std::fs::write(&tmp, text)
            .map_err(|e| AppError::Io(format!("{}: {e}", tmp.display())))?;

        // Replace atomically so a crash mid-write cannot truncate the config.
        std::fs::rename(&tmp, &path)
            .map_err(|e| AppError::Io(format!("{}: {e}", path.display())))?;
        Ok(())
    }
}