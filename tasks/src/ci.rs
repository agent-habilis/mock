use agent_habilis_mock::util::output;
use xshell::{Shell, cmd};

use crate::TaskOutcome;

pub(crate) fn run(sh: &Shell) -> TaskOutcome {
    output::status("Checking", "formatting");
    cmd!(sh, "cargo fmt --check").quiet().run()?;

    output::status("Running", "clippy");
    cmd!(sh, "cargo clippy --all-targets -- -D warnings")
        .quiet()
        .run()?;

    output::status("Running", "tests");
    cmd!(sh, "cargo test").quiet().run()?;

    Ok(())
}
