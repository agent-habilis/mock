//! Cargo-style status output, mirroring the sibling `browse` crate's
//! `util::output`: bold, 12-column right-aligned green verbs via `anstyle`,
//! printed through `anstream` so color is stripped when stderr isn't a tty.

use anstyle::{AnsiColor, Style};

/// Bold foreground in `color` — the weight cargo uses for status verbs.
pub(crate) fn bold(color: AnsiColor) -> Style {
    Style::new().fg_color(Some(color.into())).bold()
}

/// A cargo-style status line: bold green right-aligned verb + message.
pub fn status(verb: &str, msg: &str) {
    status_line(AnsiColor::Green, verb, msg);
}

/// A bold red `error: <msg>`, matching cargo/rustc diagnostics.
pub fn error(msg: &str) {
    let style = bold(AnsiColor::Red);
    anstream::eprintln!("{style}error{style:#}: {msg}");
}

fn status_line(color: AnsiColor, verb: &str, msg: &str) {
    let style = bold(color);
    anstream::eprintln!("{style}{verb:>12}{style:#} {msg}");
}
