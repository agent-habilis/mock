use std::sync::atomic::{AtomicU8, Ordering};

static LOG_LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Verbose as u8);

/// Logging verbosity. Ordered so a higher level enables everything below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LogLevel {
    /// No logging.
    Silent = 0,
    /// Only `error`.
    Error = 1,
    /// `error`, `warn`, `info`, `success`.
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

/// Log an error-level message (red label).
pub fn error(message: &str) {
    if enabled(LogLevel::Error) {
        eprintln!("\x1b[31merror\x1b[0m {message}");
    }
}

/// Log a warning-level message (yellow label).
pub fn warn(message: &str) {
    if enabled(LogLevel::Warn) {
        eprintln!("\x1b[33mwarn\x1b[0m {message}");
    }
}

/// Log an info-level message (blue label). Gated at `Warn`, so per-request and
/// startup lines still show at `--logging warn`.
pub fn info(message: &str) {
    if enabled(LogLevel::Warn) {
        eprintln!("\x1b[34minfo\x1b[0m {message}");
    }
}

/// Log a success message (green label). Gated at `Warn`.
pub fn success(message: &str) {
    if enabled(LogLevel::Warn) {
        eprintln!("\x1b[32msuccess\x1b[0m {message}");
    }
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
        // error: shown at error and above
        assert!(!should_log(LogLevel::Error, LogLevel::Silent));
        assert!(should_log(LogLevel::Error, LogLevel::Error));
        // info/warn/success are gated at warn: info is visible at `warn`, not
        // only at `verbose`.
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
        error("e");
        warn("w");
        info("i");
        success("s");
    }
}
