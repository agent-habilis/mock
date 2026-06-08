use xshell::{Shell, cmd};

use crate::TaskOutcome;

pub(crate) fn run(sh: &Shell) -> TaskOutcome {
    eprintln!("=> Installing ahm...");
    // `--force` is required: the crate version rarely changes between builds,
    // and without it `cargo install --path .` treats "already installed" as
    // up-to-date and skips the rebuild, silently leaving a stale binary in
    // place. `--force` always rebuilds + reinstalls the current tree.
    cmd!(sh, "cargo install --path . --force").quiet().run()?;
    eprintln!("=> Installed to ~/.cargo/bin/ahm");
    Ok(())
}
