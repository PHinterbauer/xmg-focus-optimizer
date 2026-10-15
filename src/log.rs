//! File logging.
//!
//! # Why this exists
//!
//! The first real run of the app came back with "the buttons don't work" and no
//! way to find out why. Everything that mattered happened inside the worker, in
//! a scheduled task, in a process the user never sees - so the only symptom was
//! a window that stopped responding. A tool that makes silent system changes
//! and cannot say what it did is not finished.
//!
//! So: every step that touches the system, every external command and its exit
//! status, every button press, and every failure is written here. The file is
//! plain text and can be pasted into an issue.
//!
//! The log lives next to the config, not next to the executable:
//! `%LOCALAPPDATA%\XMG-Optimizer\xmg-optimizer.log`

use std::fmt::Arguments;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;

/// Severity. Lower is more severe; the file records the threshold set in
/// `log_level.txt` next to the config.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
        }
    }

    fn parse(s: &str) -> Level {
        match s.trim().to_ascii_lowercase().as_str() {
            "error" => Level::Error,
            "warn" => Level::Warn,
            "debug" => Level::Debug,
            _ => Level::Info,
        }
    }
}

/// Rotate at this size so a chatty debug session cannot fill the disk.
const MAX_BYTES: u64 = 2 * 1024 * 1024;

static LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);
static FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();

fn sink() -> &'static Mutex<Option<File>> {
    FILE.get_or_init(|| Mutex::new(None))
}

/// Opens (or reopens) the log and applies the level from `log_level.txt`.
///
/// Safe to call more than once; only the first call opens the file.
pub fn init() {
    set_level_from_file();

    let path = crate::paths::log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    // Roll the previous file aside if it got big.
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > MAX_BYTES {
            let old = path.with_extension("log.1");
            let _ = std::fs::rename(&path, old);
        }
    }

    let f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok();

    if let Ok(mut g) = sink().lock() {
        *g = f;
    }
    write(Level::Info, "--- session start ---", &[]);
}

fn set_level_from_file() {
    let p = crate::paths::app_data_dir().join("log_level.txt");
    let lvl = std::fs::read_to_string(&p)
        .ok()
        .map(|s| Level::parse(&s))
        .unwrap_or(Level::Info);
    LEVEL.store(lvl as u8, Ordering::Relaxed);
}

pub fn enabled(level: Level) -> bool {
    (level as u8) <= LEVEL.load(Ordering::Relaxed)
}

fn write(level: Level, msg: &str, args: &Arguments<'_>) {
    let line = if args.is_empty() {
        format!("[{}] {}  {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"), level.tag(), msg)
    } else {
        format!(
            "[{}] {}  {}{}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            level.tag(),
            msg,
            args
        )
    };
    // stderr as well, so a `--tray` launch still leaves a trace somewhere
    // visible when debugging from a terminal.
    eprintln!("{line}");
    if let Ok(mut g) = sink().lock() {
        if let Some(f) = g.as_mut() {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }
}

/// Writes one line. The `$($arg)*` tail is optional and appended verbatim.
#[macro_export]
macro_rules! log_at {
    ($lvl:expr, $($arg:tt)*) => {
        $crate::log::emit($lvl, format!($($arg)*))
    };
}

pub fn emit(level: Level, msg: String) {
    if !enabled(level) {
        return;
    }
    // Split the first line off as the format string so it reads naturally.
    let mut it = msg.splitn(2, '\n');
    let head = it.next().unwrap_or("").to_string();
    let tail = it.next().map(|t| format!("\n{}", t)).unwrap_or_default();
    write(level, &head, &format_args!("{}", tail));
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => { $crate::log::emit($crate::log::Level::Error, format!($($arg)*)) };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => { $crate::log::emit($crate::log::Level::Warn, format!($($arg)*)) };
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::log::emit($crate::log::Level::Info, format!($($arg)*)) };
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => { $crate::log::emit($crate::log::Level::Debug, format!($($arg)*)) };
}

/// Logs a command that is about to be run, and how it went.
///
/// This is the single most useful line in the file: `powercfg` and `nvidia-smi`
/// fail in ways that print nothing useful to a console nobody is watching, and
/// the exit status plus stderr is the only evidence.
pub fn command(dir: &str, args: &[&str], result: &std::process::Output) {
    let cmd = std::iter::once(dir.to_string())
        .chain(args.iter().map(|s| s.to_string()))
        .collect::<Vec<_>>()
        .join(" ");
    match result.status.code() {
        Some(0) => {
            log_info!("command ok      : {cmd}");
        }
        Some(c) => {
            let err = String::from_utf8_lossy(&result.stderr);
            let err = err.trim().lines().take(4).collect::<Vec<_>>().join(" | ");
            log_warn!("command FAILED  : {cmd}\n    exit={c} stderr={err}");
        }
        None => {
            log_error!("command killed  : {cmd}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_order_from_error_to_debug() {
        assert!(Level::Error < Level::Warn);
        assert!(Level::Warn < Level::Info);
        assert!(Level::Info < Level::Debug);
    }

    #[test]
    fn level_parsing_falls_back_to_info() {
        assert_eq!(Level::parse("DEBUG"), Level::Debug);
        assert_eq!(Level::parse("  warn "), Level::Warn);
        assert_eq!(Level::parse("nonsense"), Level::Info);
    }
}