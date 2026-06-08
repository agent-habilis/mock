use xshell::{Shell, cmd};

use crate::TaskOutcome;

pub(crate) fn run(sh: &Shell) -> TaskOutcome {
    eprintln!("=> Checking formatting...");
    cmd!(sh, "cargo fmt --check").quiet().run()?;

    eprintln!("=> Running clippy...");
    cmd!(sh, "cargo clippy --all-targets -- -D warnings")
        .quiet()
        .run()?;

    eprintln!("=> Running tests...");
    cmd!(sh, "cargo test").quiet().run()?;

    Ok(())
}
