use std::path::{Path, PathBuf};

use agent_habilis_mock::util::output;
use xshell::{Shell, cmd};

/// The workspace root — the parent of this `tasks` crate's manifest dir.
pub(crate) fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

/// Install `krate` via `cargo install --locked` if the probe command (`check`)
/// fails. Best-effort: a probe or install hiccup must not abort the calling
/// task — its own command surfaces a clear error if the tool is genuinely
/// missing.
pub(crate) fn ensure_installed(sh: &Shell, krate: &str, check: &[&str]) {
    let ok = cmd!(sh, "cargo {check...}")
        .quiet()
        .ignore_stdout()
        .ignore_stderr()
        .run()
        .is_ok();

    if !ok {
        output::status("Installing", krate);
        let _ = cmd!(sh, "cargo install --locked {krate}").quiet().run();
    }
}
