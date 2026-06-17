//! Wire-contract test for `ahm man`: the binary must print its embedded manual
//! to stdout, exit 0, and render the canonical man-page sections. Spawns the
//! real binary (the in-process helpers drive the server, not the CLI), so this
//! also covers the optional-subcommand wiring end to end.

use std::process::Command;

#[test]
fn man_prints_manual_to_stdout() {
    let output = Command::new(env!("CARGO_BIN_EXE_ahm"))
        .arg("man")
        .output()
        .expect("failed to run `ahm man`");

    assert!(
        output.status.success(),
        "`ahm man` should exit 0, got {:?}",
        output.status
    );

    let stdout = String::from_utf8(output.stdout).expect("manual is UTF-8");

    // Renders the man page: the canonical sections + key flags and modes.
    for marker in [
        "NAME",
        "SYNOPSIS",
        "DESCRIPTION",
        "MODES",
        "OPTIONS",
        "EXAMPLES",
        "EXIT STATUS",
        "ahm man",
        "--origin",
        "--mocks-dir",
        "--rewrite-path",
        "read-write",
        "pass-read",
    ] {
        assert!(
            stdout.contains(marker),
            "manual missing expected marker {marker:?}"
        );
    }
}
