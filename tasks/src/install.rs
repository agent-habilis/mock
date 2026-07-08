use agent_habilis_mock::util::output;
use xshell::{Shell, cmd};

use crate::TaskOutcome;

pub(crate) fn run(sh: &Shell) -> TaskOutcome {
    output::status("Installing", "agent-mock");
    // `--force` is required: the crate version rarely changes between builds,
    // and without it `cargo install --path .` treats "already installed" as
    // up-to-date and skips the rebuild, silently leaving a stale binary in
    // place. `--force` always rebuilds + reinstalls the current tree.
    cmd!(sh, "cargo install --path . --force").quiet().run()?;
    output::status("Installed", "~/.cargo/bin/agent-mock");
    Ok(())
}
