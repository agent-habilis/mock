//! Command-line argument parsing and validation.
//!
//! Built on `clap`'s derive API (the same arg library the sibling `ahs` binary
//! uses), so the long flags are idiomatic kebab-case: `--mocks-dir`,
//! `--mock-keys`, `--overwrite-request-headers`, and so on. [`Args`] is the raw
//! clap view; [`Args::validate`] turns it into a [`ValidatedArgs`] with resolved
//! paths, parsed JSON header maps, and a checked mock-key set.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use clap::{Parser, ValueEnum};

/// Record/playback strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Mode {
    /// Serve a mock if one exists, otherwise 404.
    Read,
    /// Always fetch from origin and overwrite the mock.
    Write,
    /// Serve a mock if one exists, otherwise fetch, save, and serve.
    ReadWrite,
    /// Always fetch from origin (no mock read or write).
    Pass,
    /// Serve a mock if one exists, otherwise fetch (no save).
    ReadPass,
    /// Fetch from origin; on a 5xx or network error, fall back to a mock.
    PassRead,
}

impl FromStr for Mode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            "read-write" => Ok(Self::ReadWrite),
            "pass" => Ok(Self::Pass),
            "read-pass" => Ok(Self::ReadPass),
            "pass-read" => Ok(Self::PassRead),
            other => Err(format!(
                "invalid mode '{other}', expected one of: read, write, read-write, pass, read-pass, pass-read"
            )),
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::ReadWrite => "read-write",
            Self::Pass => "pass",
            Self::ReadPass => "read-pass",
            Self::PassRead => "pass-read",
        };
        formatter.write_str(value)
    }
}

/// Bulk mock-refresh behavior at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Update {
    /// Never refresh mocks.
    Off,
    /// Refresh every mock in the mocks dir at startup, then serve.
    Startup,
    /// Refresh every mock in the mocks dir, then exit.
    Only,
}

impl FromStr for Update {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "off" => Ok(Self::Off),
            "startup" => Ok(Self::Startup),
            "only" => Ok(Self::Only),
            other => Err(format!(
                "invalid update '{other}', expected one of: off, startup, only"
            )),
        }
    }
}

impl fmt::Display for Update {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Off => "off",
            Self::Startup => "startup",
            Self::Only => "only",
        };
        formatter.write_str(value)
    }
}

/// Logging verbosity. `Verbose` enables every message (including `info`), down
/// to `Silent` which suppresses all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogLevel {
    /// No logging.
    Silent,
    /// Only `error`.
    Error,
    /// `error` and `warn`.
    Warn,
    /// Everything (`error`, `warn`, `info`).
    Verbose,
}

impl FromStr for LogLevel {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "silent" => Ok(Self::Silent),
            "error" => Ok(Self::Error),
            "warn" => Ok(Self::Warn),
            "verbose" => Ok(Self::Verbose),
            other => Err(format!(
                "invalid logging '{other}', expected one of: silent, error, warn, verbose"
            )),
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Silent => "silent",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Verbose => "verbose",
        };
        formatter.write_str(value)
    }
}

/// Maximum synthetic delay (1 hour).
const MAX_DELAY_MS: u64 = 3_600_000;

/// Default mock keys: request method + URL.
const MOCK_KEYS_DEFAULT: &str = "method,url";

/// HTTP mock server for development and stable E2E tests.
///
/// Records HTTP interactions as a reverse proxy and replays them, so tests and
/// local development don't need to reach the real upstream service.
#[derive(Parser, Debug, Clone)]
#[command(name = "ahm", version, after_help = "a tool by agent-habilis █🫈")]
pub struct Args {
    /// Origin base URL to proxy requests to (defaults to localhost in read mode)
    #[arg(long)]
    pub origin: Option<String>,

    /// Port the server listens on
    #[arg(long, default_value_t = 8273)]
    pub port: u16,

    /// Path to the mocked-responses directory
    #[arg(long, default_value = ".")]
    pub mocks_dir: String,

    /// Record/playback mode
    #[arg(long, value_enum, default_value_t = Mode::Pass)]
    pub mode: Mode,

    /// Bulk mock-refresh behavior at startup
    #[arg(long, value_enum, default_value_t = Update::Off)]
    pub update: Update,

    /// Synthetic delay added to every response, in milliseconds
    #[arg(long, default_value_t = 0)]
    pub delay: u64,

    /// Synthetic throughput cap, in bytes/second (or "Infinity" for no cap)
    #[arg(long, default_value = "Infinity")]
    pub throttle: String,

    /// Max retries to origin while the response is not a 2xx
    #[arg(long, default_value_t = 0)]
    pub retries: u32,

    /// Comma-separated request attributes used to key mocks
    #[arg(long, default_value = MOCK_KEYS_DEFAULT)]
    pub mock_keys: String,

    /// Logging verbosity
    #[arg(long, value_enum, default_value_t = LogLevel::Verbose)]
    pub logging: LogLevel,

    /// Send CORS headers between client and proxy
    #[arg(long)]
    pub cors: bool,

    /// Upstream HTTP proxy URL to forward origin requests through
    #[arg(long, default_value = "")]
    pub proxy: String,

    /// JSON of header names to redact from mocks and logging
    #[arg(long, default_value = "{}")]
    pub redacted_headers: String,

    /// JSON of response headers to overwrite with the given values
    #[arg(long, default_value = "{}")]
    pub overwrite_response_headers: String,

    /// JSON of request headers to overwrite on the request to origin
    /// (defaults to `{ "host": <origin host> }`)
    #[arg(long)]
    pub overwrite_request_headers: Option<String>,
}

/// Fully resolved and validated configuration handed to the server.
#[derive(Debug, Clone)]
pub struct ValidatedArgs {
    pub origin: String,
    pub port: u16,
    pub mocks_dir: PathBuf,
    pub mode: Mode,
    pub update: Update,
    pub delay: u64,
    /// Bytes/second; `0` means unlimited (the `--throttle Infinity` default).
    pub throttle: u64,
    pub retries: u32,
    pub mock_keys: HashSet<String>,
    pub logging: LogLevel,
    pub cors: bool,
    pub proxy: String,
    pub redacted_headers: HashMap<String, serde_json::Value>,
    pub overwrite_response_headers: HashMap<String, serde_json::Value>,
    pub overwrite_request_headers: HashMap<String, serde_json::Value>,
}

impl Args {
    /// Resolve and type-check the raw arguments.
    ///
    /// # Errors
    /// Returns a human-readable message when origin is not a valid HTTP(S) URL,
    /// `--delay`/`--throttle` are out of range, a `--mock-keys` entry is
    /// unrecognized, or any of the JSON header flags fail to parse.
    pub fn validate(&self) -> Result<ValidatedArgs, String> {
        let origin = resolve_origin(self.origin.as_deref(), self.mode)?;
        let mocks_dir = resolve_mocks_dir(&self.mocks_dir)?;
        let throttle = parse_throttle(&self.throttle)?;

        if self.delay > MAX_DELAY_MS {
            return Err(format!(
                "invalid --delay '{}', expected a non-negative integer up to {MAX_DELAY_MS} ms",
                self.delay
            ));
        }

        let mock_keys = parse_mock_keys(&self.mock_keys)?;
        let redacted_headers = parse_json_headers("redacted-headers", &self.redacted_headers)?;
        let overwrite_response_headers = parse_json_headers(
            "overwrite-response-headers",
            &self.overwrite_response_headers,
        )?;

        let overwrite_request_headers = match &self.overwrite_request_headers {
            Some(json) => parse_json_headers("overwrite-request-headers", json)?,
            None => default_request_headers(&origin),
        };

        Ok(ValidatedArgs {
            origin,
            port: self.port,
            mocks_dir,
            mode: self.mode,
            update: self.update,
            delay: self.delay,
            throttle,
            retries: self.retries,
            mock_keys,
            logging: self.logging,
            cors: self.cors,
            proxy: self.proxy.clone(),
            redacted_headers,
            overwrite_response_headers,
            overwrite_request_headers,
        })
    }
}

/// Resolve `--origin`, applying the read-mode default and HTTP(S) check.
fn resolve_origin(origin: Option<&str>, mode: Mode) -> Result<String, String> {
    let origin = origin.unwrap_or("");
    if origin.is_empty() && mode == Mode::Read {
        return Ok("http://localhost".to_string());
    }
    if !origin.starts_with("http://") && !origin.starts_with("https://") {
        return Err(format!(
            "invalid --origin '{origin}', expected a URL with the http:// or https:// scheme"
        ));
    }
    // A scheme with no authority (e.g. "http://" or "http:///path") is rejected:
    // otherwise the default host header would be empty and proxying would
    // silently break.
    if extract_host(origin).is_none() {
        return Err(format!(
            "invalid --origin '{origin}', expected a URL with a host"
        ));
    }
    Ok(origin.to_string())
}

/// Resolve `--mocks-dir` to an absolute path against the current directory.
fn resolve_mocks_dir(mocks_dir: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(mocks_dir);
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|err| format!("failed to resolve --mocks-dir: {err}"))
    }
}

/// Parse `--throttle`: `Infinity` (or empty) means unlimited, encoded as `0`.
fn parse_throttle(throttle: &str) -> Result<u64, String> {
    let trimmed = throttle.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("infinity") {
        return Ok(0);
    }
    match trimmed.parse::<u64>() {
        // Non-positive caps are rejected; `0` is reserved as the unlimited
        // sentinel, so it is not a valid throughput value.
        Ok(bytes_per_sec) if bytes_per_sec > 0 => Ok(bytes_per_sec),
        _ => Err(format!(
            "invalid --throttle '{throttle}', expected a positive integer or Infinity"
        )),
    }
}

/// Parse and validate `--mock-keys` into a set, accepting `url`, `method`,
/// `headers`, `body`, and dotted `body.*` / `header.*` paths.
fn parse_mock_keys(raw: &str) -> Result<HashSet<String>, String> {
    let mut keys = HashSet::new();
    for key in raw.split(',') {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        if is_valid_mock_key(key) {
            keys.insert(key.to_string());
        } else {
            return Err(format!(
                "invalid --mock-keys entry '{key}', expected one of: url, method, headers, body (or a dotted body.* / header.* path)"
            ));
        }
    }
    Ok(keys)
}

/// Whether `key` is an accepted `--mock-keys` token.
fn is_valid_mock_key(key: &str) -> bool {
    matches!(key, "url" | "method" | "headers" | "body")
        || is_dotted_path("body", key)
        || is_dotted_path("header", key)
}

/// Whether `key` is `prefix.seg(.seg)*` with `[A-Za-z0-9_-]` segments. The bare
/// `prefix` is not accepted here — `url`/`method`/`headers`/`body` are matched
/// directly by [`is_valid_mock_key`], so a lone `header` (which names nothing)
/// is rejected rather than silently ignored during mock keying.
fn is_dotted_path(prefix: &str, key: &str) -> bool {
    let Some(rest) = key
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('.'))
    else {
        return false;
    };
    !rest.is_empty()
        && rest.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        })
}

/// Parse one of the JSON header flags into a `{ name: value }` map.
fn parse_json_headers(
    flag: &str,
    json: &str,
) -> Result<HashMap<String, serde_json::Value>, String> {
    serde_json::from_str(json).map_err(|err| format!("invalid --{flag} JSON: {err}"))
}

/// Build the default `overwrite-request-headers` (`{ host: <origin host> }`).
fn default_request_headers(origin: &str) -> HashMap<String, serde_json::Value> {
    let mut headers = HashMap::new();
    if let Some(host) = extract_host(origin) {
        headers.insert("host".to_string(), serde_json::Value::String(host));
    }
    headers
}

/// Extract the `host[:port]` authority from an HTTP(S) origin URL.
fn extract_host(origin: &str) -> Option<String> {
    let without_scheme = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))?;
    let host = without_scheme.split('/').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(crate::proptest_support::config())]
        /// `parse_throttle` never panics; only positive ints / Infinity succeed.
        #[test]
        fn prop_parse_throttle_never_panics(value in ".*") {
            if let Ok(parsed) = parse_throttle(&value) {
                let trimmed = value.trim();
                if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("infinity") {
                    prop_assert_eq!(parsed, 0);
                } else {
                    prop_assert!(parsed > 0);
                }
            }
        }

        /// mock-key validation never panics; accepted keys are non-empty.
        #[test]
        fn prop_parse_mock_keys_never_panics(value in ".*") {
            if let Ok(keys) = parse_mock_keys(&value) {
                for key in keys {
                    prop_assert!(!key.is_empty());
                }
            }
        }
    }

    fn args_for(origin: &str) -> Args {
        Args {
            origin: Some(origin.to_string()),
            port: 8273,
            mocks_dir: ".".to_string(),
            mode: Mode::Pass,
            update: Update::Off,
            delay: 0,
            throttle: "Infinity".to_string(),
            retries: 0,
            mock_keys: "url,method".to_string(),
            logging: LogLevel::Verbose,
            cors: false,
            proxy: String::new(),
            redacted_headers: "{}".to_string(),
            overwrite_response_headers: "{}".to_string(),
            overwrite_request_headers: None,
        }
    }

    #[test]
    fn parses_kebab_case_flags() {
        let args = Args::try_parse_from([
            "ahm",
            "--origin",
            "http://example.com",
            "--mocks-dir",
            "/tmp/m",
            "--mock-keys",
            "url,method,body",
            "--redacted-headers",
            "{\"authorization\":null}",
            "--overwrite-response-headers",
            "{\"x-a\":\"b\"}",
            "--overwrite-request-headers",
            "{\"host\":\"h\"}",
            "--mode",
            "read-write",
            "--logging",
            "warn",
            "--cors",
        ])
        .unwrap();

        assert_eq!(args.origin.as_deref(), Some("http://example.com"));
        assert_eq!(args.mocks_dir, "/tmp/m");
        assert_eq!(args.mock_keys, "url,method,body");
        assert_eq!(args.mode, Mode::ReadWrite);
        assert_eq!(args.logging, LogLevel::Warn);
        assert!(args.cors);
    }

    #[test]
    fn default_values_match_js() {
        let args = Args::try_parse_from(["ahm", "--origin", "http://example.com"]).unwrap();
        assert_eq!(args.port, 8273);
        assert_eq!(args.mode, Mode::Pass);
        assert_eq!(args.update, Update::Off);
        assert_eq!(args.delay, 0);
        assert_eq!(args.throttle, "Infinity");
        assert_eq!(args.retries, 0);
        assert_eq!(args.mock_keys, "method,url");
        assert_eq!(args.logging, LogLevel::Verbose);
        assert!(!args.cors);
    }

    #[test]
    fn mode_roundtrips_through_str() {
        for mode in [
            Mode::Read,
            Mode::Write,
            Mode::ReadWrite,
            Mode::Pass,
            Mode::ReadPass,
            Mode::PassRead,
        ] {
            assert_eq!(mode.to_string().parse::<Mode>().unwrap(), mode);
        }
        assert!("nope".parse::<Mode>().is_err());
    }

    #[test]
    fn update_roundtrips_through_str() {
        for update in [Update::Off, Update::Startup, Update::Only] {
            assert_eq!(update.to_string().parse::<Update>().unwrap(), update);
        }
        assert!("nope".parse::<Update>().is_err());
    }

    #[test]
    fn log_level_roundtrips_through_str() {
        for level in [
            LogLevel::Silent,
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Verbose,
        ] {
            assert_eq!(level.to_string().parse::<LogLevel>().unwrap(), level);
        }
        assert!("nope".parse::<LogLevel>().is_err());
    }

    #[test]
    fn invalid_origin_is_rejected() {
        assert!(args_for("example.com").validate().is_err());
        assert!(args_for("ftp://example.com").validate().is_err());
        // scheme present but no host
        assert!(args_for("http://").validate().is_err());
        assert!(args_for("https:///path").validate().is_err());
    }

    #[test]
    fn valid_origin_is_accepted() {
        assert!(args_for("http://example.com").validate().is_ok());
        assert!(args_for("https://example.com").validate().is_ok());
    }

    #[test]
    fn empty_origin_defaults_to_localhost_in_read_mode() {
        let mut args = args_for("");
        args.origin = None;
        args.mode = Mode::Read;
        assert_eq!(args.validate().unwrap().origin, "http://localhost");
    }

    #[test]
    fn empty_origin_errors_outside_read_mode() {
        let mut args = args_for("");
        args.origin = None;
        args.mode = Mode::Pass;
        assert!(args.validate().is_err());
    }

    #[test]
    fn mock_keys_default_set() {
        let validated = args_for("http://example.com").validate().unwrap();
        assert_eq!(validated.mock_keys.len(), 2);
        assert!(validated.mock_keys.contains("url"));
        assert!(validated.mock_keys.contains("method"));
    }

    #[test]
    fn mock_keys_accepts_js_values_and_dotted_paths() {
        let mut args = args_for("http://example.com");
        args.mock_keys =
            "url,method,headers,body,body.variables.id,header.authorization".to_string();
        let validated = args.validate().unwrap();
        assert_eq!(validated.mock_keys.len(), 6);
        assert!(validated.mock_keys.contains("headers"));
        assert!(validated.mock_keys.contains("body.variables.id"));
    }

    #[test]
    fn mock_keys_rejects_unknown_entry() {
        let mut args = args_for("http://example.com");
        args.mock_keys = "url,bogus".to_string();
        assert!(args.validate().is_err());
        // A bare `header` (naming no specific header) and a trailing-dot path
        // are rejected rather than silently ignored during mock keying.
        args.mock_keys = "header".to_string();
        assert!(args.validate().is_err());
        args.mock_keys = "body.".to_string();
        assert!(args.validate().is_err());
    }

    #[test]
    fn throttle_infinity_is_unlimited() {
        let mut args = args_for("http://example.com");
        args.throttle = "Infinity".to_string();
        assert_eq!(args.validate().unwrap().throttle, 0);
        args.throttle = "2048".to_string();
        assert_eq!(args.validate().unwrap().throttle, 2048);
        // `0` and non-numeric values are rejected; `0` is the reserved
        // unlimited sentinel, not a valid throughput cap.
        args.throttle = "0".to_string();
        assert!(args.validate().is_err());
        args.throttle = "nope".to_string();
        assert!(args.validate().is_err());
    }

    #[test]
    fn delay_above_ceiling_is_rejected() {
        let mut args = args_for("http://example.com");
        args.delay = MAX_DELAY_MS + 1;
        assert!(args.validate().is_err());
    }

    #[test]
    fn default_overwrite_request_headers_use_origin_host() {
        let validated = args_for("http://api.example.com:3000").validate().unwrap();
        assert_eq!(
            validated.overwrite_request_headers.get("host"),
            Some(&serde_json::Value::String(
                "api.example.com:3000".to_string()
            ))
        );
    }

    #[test]
    fn mocks_dir_is_resolved_to_absolute() {
        let validated = args_for("http://example.com").validate().unwrap();
        assert!(validated.mocks_dir.is_absolute());
    }

    #[test]
    fn extract_host_strips_scheme_and_path() {
        assert_eq!(
            extract_host("http://example.com"),
            Some("example.com".to_string())
        );
        assert_eq!(
            extract_host("https://example.com/path"),
            Some("example.com".to_string())
        );
        assert_eq!(
            extract_host("http://localhost:3000"),
            Some("localhost:3000".to_string())
        );
    }

    #[test]
    fn invalid_json_header_flag_is_rejected() {
        let mut args = args_for("http://example.com");
        args.redacted_headers = "{not json".to_string();
        assert!(args.validate().is_err());
    }
}
