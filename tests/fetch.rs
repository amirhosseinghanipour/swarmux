//! `swarmux fetch` as a user runs it: the real binary and an empty home
//! folder, so the card says the same thing on every machine.

use std::path::Path;
use std::process::{Command, Output};

use swarmux_tui::fetch::strip_ansi;

const EXE: &str = env!("CARGO_BIN_EXE_swarmux");

fn fetch(home: &Path, args: &[&str], no_color: bool) -> Output {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let mut cmd = Command::new(EXE);
    cmd.arg("fetch")
        .args(args)
        .env(var, home)
        .env("SWARMUX_CLAUDE_BIN", home.join("no-such-claude"))
        // The same colour depth on every machine, CI included.
        .env("COLORTERM", "truecolor")
        .env_remove("NO_COLOR");
    if no_color {
        cmd.env("NO_COLOR", "1");
    }
    cmd.output().unwrap()
}

fn stdout(out: &Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn the_card_reads_the_same_with_its_colours_stripped() {
    let home = tempfile::tempdir().unwrap();
    let coloured = stdout(&fetch(
        home.path(),
        &["--theme", "nocturne", "--color", "always"],
        false,
    ));
    assert!(coloured.contains("\x1b[38;2;") || coloured.contains("\x1b[38;5;"));
    let plain = strip_ansi(&coloured);
    let facts = [
        "swarmux@claude-code".to_string(),
        format!("version  {}", env!("CARGO_PKG_VERSION")),
        "theme    nocturne".to_string(),
        "sessions 0".to_string(),
        "agents   none running".to_string(),
        "5h limit unknown".to_string(),
        "7d limit unknown".to_string(),
        "missions 0 delivered".to_string(),
        "ledger   empty".to_string(),
        "claude   not found".to_string(),
        format!(
            "os       {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    ];
    let never = stdout(&fetch(
        home.path(),
        &["--theme", "nocturne", "--color", "never"],
        false,
    ));
    assert!(!never.contains(''));
    for text in [&plain, &never] {
        for fact in &facts {
            assert!(
                text.contains(fact),
                "missing {fact:?} in
{text}"
            );
        }
        assert!(text.contains("███████╗██╗    ██╗"), "the logo");
    }
    assert!(
        plain.contains("██████████████████████████████"),
        "the palette row"
    );
    assert!(
        !never.contains("██████████████████████████████"),
        "no palette without colour"
    );
}

#[test]
fn no_color_wins_over_always_and_a_pipe_gets_plain_text() {
    let home = tempfile::tempdir().unwrap();
    let forced = stdout(&fetch(home.path(), &["--color", "always"], true));
    assert!(!forced.contains('\x1b'), "NO_COLOR is respected");
    let piped = stdout(&fetch(home.path(), &[], false));
    assert!(
        !piped.contains('\x1b'),
        "no colours into a pipe unless asked"
    );
}

#[test]
fn an_unknown_theme_or_colour_mode_is_an_error() {
    let home = tempfile::tempdir().unwrap();
    assert!(
        !fetch(home.path(), &["--theme", "solarized-dark-ish"], false)
            .status
            .success()
    );
    assert!(
        !fetch(home.path(), &["--color", "sometimes"], false)
            .status
            .success()
    );
}
