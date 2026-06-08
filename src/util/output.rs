//! Cargo-style status output with `--logging` level gating.
//!
//! Mirrors cargo/rustc: bold, 12-column right-aligned status verbs (rendered via
//! `anstyle`) printed through `anstream`, so color is stripped when the stream
//! isn't a tty and `NO_COLOR`/`CLICOLOR` are honored. `warning:`/`error:`
//! diagnostics start at column 0, exactly as cargo prints them.
//!
//! This module is also the verbosity authority: a process-global level (set from
//! `--logging`) gates every emitter, so the same helpers serve both the long-
//! running server and the task runner.

use std::sync::atomic::{AtomicU8, Ordering};

use anstyle::{AnsiColor, Style};

static LOG_LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Verbose as u8);

/// Logging verbosity. Ordered so a higher level enables everything below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LogLevel {
    /// No logging.
    Silent = 0,
    /// Only `error`.
    Error = 1,
    /// `error`, `warning`, and status lines.
    Warn = 2,
    /// Everything (currently the same emitters as `Warn`; reserved for raw
    /// verbose output).
    Verbose = 3,
}

impl LogLevel {
    const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Silent,
            1 => Self::Error,
            2 => Self::Warn,
            // 3 and any unknown value map to the most verbose level.
            _ => Self::Verbose,
        }
    }
}

/// Set the global log level.
pub fn set_level(level: LogLevel) {
    LOG_LEVEL.store(level as u8, Ordering::Relaxed);
}

/// Get the current global log level.
pub fn get_level() -> LogLevel {
    LogLevel::from_u8(LOG_LEVEL.load(Ordering::Relaxed))
}

/// Pure threshold check: a message gated at `threshold` prints when the active
/// `current` level is at least as high.
const fn should_log(threshold: LogLevel, current: LogLevel) -> bool {
    (threshold as u8) <= (current as u8)
}

/// Whether a message gated at `threshold` should print at the current level.
fn enabled(threshold: LogLevel) -> bool {
    should_log(threshold, get_level())
}

/// Bold foreground in `color` — the weight cargo uses for status verbs.
pub(crate) fn bold(color: AnsiColor) -> Style {
    Style::new().fg_color(Some(color.into())).bold()
}

/// A cargo-style status line: bold green right-aligned verb + message. Gated at
/// `Warn`, alongside the other status/`warning` output.
pub fn status(verb: &str, msg: &str) {
    if enabled(LogLevel::Warn) {
        status_line(AnsiColor::Green, verb, msg);
    }
}

/// A per-response status line whose `Response` verb is colored by HTTP status
/// class — green for 2xx/3xx, yellow for 4xx, red for 5xx. `note` adds a trailing
/// tag (e.g. `CORS`). Gated at `Warn`.
pub fn response(connection_id: &str, status: u16, note: Option<&str>) {
    if !enabled(LogLevel::Warn) {
        return;
    }
    let color = match status {
        500..=599 => AnsiColor::Red,
        400..=499 => AnsiColor::Yellow,
        _ => AnsiColor::Green,
    };
    let detail = match note {
        Some(note) => format!("{connection_id} {status} {note}"),
        None => format!("{connection_id} {status}"),
    };
    status_line(color, "Response", &detail);
}

/// A bold yellow `warning: <msg>`, matching cargo/rustc diagnostics. Gated at
/// `Warn`.
pub fn warning(msg: &str) {
    if enabled(LogLevel::Warn) {
        let style = bold(AnsiColor::Yellow);
        anstream::eprintln!("{style}warning{style:#}: {msg}");
    }
}

/// A bold red `error: <msg>`, matching cargo/rustc diagnostics. Gated at `Error`.
pub fn error(msg: &str) {
    if enabled(LogLevel::Error) {
        let style = bold(AnsiColor::Red);
        anstream::eprintln!("{style}error{style:#}: {msg}");
    }
}

fn status_line(color: AnsiColor, verb: &str, msg: &str) {
    let style = bold(color);
    anstream::eprintln!("{style}{verb:>12}{style:#} {msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_ordering() {
        assert!(LogLevel::Silent < LogLevel::Error);
        assert!(LogLevel::Error < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Verbose);
    }

    #[test]
    fn test_repr_values() {
        assert_eq!(LogLevel::Silent as u8, 0);
        assert_eq!(LogLevel::Error as u8, 1);
        assert_eq!(LogLevel::Warn as u8, 2);
        assert_eq!(LogLevel::Verbose as u8, 3);
    }

    #[test]
    fn test_from_u8() {
        assert_eq!(LogLevel::from_u8(0), LogLevel::Silent);
        assert_eq!(LogLevel::from_u8(1), LogLevel::Error);
        assert_eq!(LogLevel::from_u8(2), LogLevel::Warn);
        assert_eq!(LogLevel::from_u8(3), LogLevel::Verbose);
        assert_eq!(LogLevel::from_u8(255), LogLevel::Verbose);
    }

    #[test]
    fn test_should_log_thresholds() {
        // error: shown at error and above.
        assert!(!should_log(LogLevel::Error, LogLevel::Silent));
        assert!(should_log(LogLevel::Error, LogLevel::Error));
        // status/warning are gated at warn: visible at `warn`, not only at
        // `verbose`.
        assert!(!should_log(LogLevel::Warn, LogLevel::Error));
        assert!(should_log(LogLevel::Warn, LogLevel::Warn));
        assert!(should_log(LogLevel::Warn, LogLevel::Verbose));
    }

    #[test]
    fn test_set_and_get_level() {
        // Sole test that mutates the process-global level, so its set→get
        // assertions can't race another test's writes.
        set_level(LogLevel::Verbose);
        assert_eq!(get_level(), LogLevel::Verbose);
        set_level(LogLevel::Error);
        assert_eq!(get_level(), LogLevel::Error);
        set_level(LogLevel::Silent);
        assert_eq!(get_level(), LogLevel::Silent);
        set_level(LogLevel::Verbose);
    }

    #[test]
    fn test_emitters_do_not_panic() {
        // Only reads the global level; never writes it (see above).
        status("Running", "tests");
        response("deadbeef", 200, None);
        response("deadbeef", 404, Some("CORS"));
        response("deadbeef", 502, None);
        warning("w");
        error("e");
    }
}
