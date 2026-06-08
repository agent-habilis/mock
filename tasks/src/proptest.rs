use agent_habilis_mock::util::output;
use xshell::{Shell, cmd};

use crate::TaskOutcome;

pub(crate) fn run(sh: &Shell) -> TaskOutcome {
    output::status("Running", "property-based tests (prop_ prefix)");
    cmd!(sh, "cargo test prop_").quiet().run()?;
    Ok(())
}
