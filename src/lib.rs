//! `agent-habilis-mock` — an HTTP mock server for development and stable E2E
//! tests.
//!
//! It is a [self-initializing fake](https://martinfowler.com/bliki/SelfInitializingFake.html)
//! implemented as a reverse proxy: it records HTTP interactions to the
//! filesystem so they can be replayed later, removing the need to reach real
//! external services during local development or broad-stack tests.
//!
//! This crate ships as **both** a binary (the `agent-mock` CLI) and a library. The
//! binary in `src/main.rs` is a thin shim over [`run`]; library consumers drive
//! a server in-process with [`serve`].
//!
//! ```no_run
//! use agent_habilis_mock::args::{Cli, Command};
//! use clap::Parser;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Parse argv, then validate the `serve` flags and serve until Ctrl-C.
//! if let Command::Serve(serve_args) = Cli::parse().command {
//!     agent_habilis_mock::serve(serve_args.validate()?).await?;
//! }
//! # Ok(())
//! # }
//! ```

// Public surface: `args` (CLI + validated config), `server` (the request
// handler + shared state the in-process test harness drives), `mock::manager`
// (read/write of mock files), and `util::output` (cargo-style output +
// verbosity control). Every other module is an implementation detail kept
// `pub(crate)` so internal refactors are never breaking API changes.
pub mod args;
pub mod mock;
pub mod server;
pub mod util;

pub(crate) mod error;
pub(crate) mod http;
pub(crate) mod proxy;
pub(crate) mod stream;

// Re-exported because it appears in `mock::manager`'s public signatures; this
// is what makes it externally reachable (and satisfies `unreachable_pub`).
pub use error::MockerError;

use std::sync::Arc;

use clap::Parser;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

use args::{Cli, Command, LogLevel, Update, ValidatedArgs};
use mock::manager::MockManager;
use server::{AppState, handle_request};
use util::output;

/// Parse `argv`, then either print the manual or run the server to completion.
///
/// This is the entire body of the `agent-mock` binary; it is public so the thin
/// `src/main.rs` shim (which owns only process-level concerns) can call it.
/// With no subcommand the top-level flags drive the server (the default); the
/// only subcommand, `man`, prints the embedded manual and exits.
///
/// # Errors
/// Returns an error if the arguments fail validation (see
/// [`ServeArgs::validate`](args::ServeArgs::validate)) or the server cannot
/// bind its port.
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::Serve(serve_args) => serve(serve_args.validate()?).await,
        // The manual is embedded at compile time (`include_str!`), so the
        // binary documents itself with no repo checkout. It carries no flags,
        // so `agent-mock man` never touches `--origin` or validation.
        Command::Man => {
            print!("{}", include_str!("../docs/manual.txt"));
            Ok(())
        }
    }
}

/// The `agent-mock` clap command tree, for offline man-page generation. Consumed by
/// the `man` task in the dev-only `tasks` crate, which renders it through
/// `clap_mangen`; never reachable from the shipped binary.
#[must_use]
pub fn cli_command() -> clap::Command {
    <Cli as clap::CommandFactory>::command()
}

/// Configure logging from `args`, then bind and serve requests until Ctrl-C.
///
/// In [`Update::Only`] mode the function returns without binding a port. The
/// bulk mock-refresh pass (`--update startup`/`only`) is accepted but not yet
/// implemented, so mocks are left unchanged.
///
/// # Errors
/// Returns an error if the configured port cannot be bound.
pub async fn serve(args: ValidatedArgs) -> Result<(), Box<dyn std::error::Error>> {
    output::set_level(log_level_for(args.logging));

    // Throttle is encoded as `0` for unlimited (the `--throttle Infinity`
    // default); render that as `∞` rather than a misleading `0 B/s`.
    let throttle = if args.throttle == 0 {
        "∞".to_string()
    } else {
        format!("{} B/s", args.throttle)
    };
    output::status(
        "Serving",
        &format!(
            "{} mode (delay {}ms, throttle {}, retries {}, cors {})",
            args.mode,
            args.delay,
            throttle,
            args.retries,
            if args.cors { "on" } else { "off" },
        ),
    );

    // `--update startup`/`only` is accepted for CLI compatibility, but the bulk
    // mock-refresh pass is not implemented yet; say so plainly rather than
    // logging a refresh that never happens.
    match args.update {
        Update::Off => {}
        Update::Startup => {
            output::warning("update mode 'startup' is not implemented; mocks are left unchanged");
        }
        Update::Only => {
            output::warning("update mode 'only' is not implemented; mocks are left unchanged");
            return Ok(());
        }
    }

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], args.port));
    let listener = TcpListener::bind(addr).await?;

    output::status("Proxying", &args.origin);
    output::status(
        "Listening",
        &format!("on {addr} (pid {})", std::process::id()),
    );

    let state = Arc::new(build_state(args));

    let accept_loop = async {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => spawn_connection(TokioIo::new(stream), state.clone()),
                Err(err) => output::error(&format!("failed to accept connection: {err}")),
            }
        }
    };

    tokio::select! {
        () = accept_loop => {}
        result = tokio::signal::ctrl_c() => {
            if let Err(err) = result {
                output::error(&format!("failed to listen for ctrl-c: {err}"));
            }
            output::clear_line();
            output::status("Closing", "agent-mock");
        }
    }

    Ok(())
}

/// Build the shared [`AppState`] (mock manager + rustls client config) from
/// `args`.
fn build_state(args: ValidatedArgs) -> AppState {
    let mock_manager = MockManager::new(
        args.mocks_dir.clone(),
        args.mock_keys.clone(),
        args.redacted_headers.clone(),
    );
    AppState {
        args,
        mock_manager,
        tls_config: server::build_tls_config(),
    }
}

/// Serve a single accepted connection on its own task.
fn spawn_connection(io: TokioIo<tokio::net::TcpStream>, state: Arc<AppState>) {
    tokio::spawn(async move {
        let service = service_fn(move |req| {
            let state = state.clone();
            async move { handle_request(req, state).await }
        });
        let _ = http1::Builder::new().serve_connection(io, service).await;
    });
}

/// Map the user-facing [`LogLevel`] to the internal output level.
fn log_level_for(level: LogLevel) -> output::LogLevel {
    match level {
        LogLevel::Silent => output::LogLevel::Silent,
        LogLevel::Error => output::LogLevel::Error,
        LogLevel::Warn => output::LogLevel::Warn,
        LogLevel::Verbose => output::LogLevel::Verbose,
    }
}

// Shared config for the crate's `proptest!` blocks. Overrides the default
// failure-persistence path so regression seeds land in
// `tests/proptest-regressions` rather than a `proptest-regressions` folder
// at the crate root. Every `proptest!` block opts in with
// `#![proptest_config(crate::proptest_support::config())]`. Kept last so it
// does not trip `clippy::items_after_test_module`.
#[cfg(test)]
pub(crate) mod proptest_support {
    use proptest::test_runner::{Config, FileFailurePersistence};

    pub(crate) fn config() -> Config {
        Config {
            failure_persistence: Some(Box::new(FileFailurePersistence::SourceParallel(
                "tests/proptest-regressions",
            ))),
            ..Config::default()
        }
    }
}
