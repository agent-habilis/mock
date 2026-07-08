use agent_habilis_mock::util::output;

use crate::TaskOutcome;
use crate::util::repo_root;

/// Generated straight from the `agent-mock` clap command tree via `clap_mangen`, which
/// lives in this dev-only crate (never the shipped `agent-mock`). Output is a build
/// artifact (`target/man/`), not checked in.
pub(crate) fn run() -> TaskOutcome {
    output::status("Generating", "man pages");
    let out = repo_root().join("target/man");
    std::fs::create_dir_all(&out)?;
    clap_mangen::generate_to(agent_habilis_mock::cli_command(), &out)?;
    output::status("Generated", "man pages (target/man)");
    Ok(())
}
