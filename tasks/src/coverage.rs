use xshell::{Shell, cmd};

use crate::TaskOutcome;
use crate::util::ensure_installed;

pub(crate) fn run(sh: &Shell) -> TaskOutcome {
    ensure_installed(sh, "cargo-llvm-cov", &["llvm-cov", "--version"]);
    eprintln!("=> Running tests with coverage instrumentation...");
    cmd!(sh, "cargo llvm-cov --no-report").quiet().run()?;
    eprintln!("=> Coverage summary:");
    cmd!(sh, "cargo llvm-cov report").quiet().run()?;
    Ok(())
}
