use agent_habilis_mock::util::output;
use xshell::{Shell, cmd};

use crate::TaskOutcome;

pub(crate) fn run(sh: &Shell) -> TaskOutcome {
    output::status("Cleaning", "build artifacts");
    cmd!(sh, "cargo clean").quiet().run()?;
    // llvm-cov uses a separate target dir
    let _ = sh.remove_path("target/llvm-cov-target");
    Ok(())
}
