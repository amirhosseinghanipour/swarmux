//! The workspace map: a one-page draft of what the repositories are and
//! how they seem to relate, from the scan alone (no model), for the owner
//! to correct once. Its lines become the core facts every session gets.

use crate::tokens::estimate_tokens;
use crate::{Stack, Workspace};

use super::CORE_TOKENS;

/// Lines that start with this are hints for the owner and are dropped.
pub const COMMENT: &str = "#";
/// A line still holding this was not filled in and is dropped.
pub const PLACEHOLDER: &str = "<fill in";

/// Commands listed per repository.
const COMMANDS_SHOWN: usize = 4;

/// What a repository's stacks say about its role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Client,
    Backend,
    Unknown,
}

fn role(stacks: &[Stack], name: &str) -> Role {
    let client = stacks
        .iter()
        .any(|s| matches!(s, Stack::Flutter | Stack::Unity | Stack::Godot));
    let backend = stacks.iter().any(|s| {
        matches!(
            s,
            Stack::Supabase | Stack::Go | Stack::Rust | Stack::Python | Stack::Ruby | Stack::DotNet
        )
    });
    let web = stacks.contains(&Stack::Node)
        && [
            "admin",
            "web",
            "site",
            "landing",
            "front",
            "ui",
            "dashboard",
        ]
        .iter()
        .any(|w| name.to_ascii_lowercase().contains(w));
    match (client || web, backend) {
        (true, false) => Role::Client,
        (false, true) => Role::Backend,
        _ => Role::Unknown,
    }
}

fn stacks_text(stacks: &[Stack]) -> String {
    if stacks.is_empty() {
        return "—".into();
    }
    stacks
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The draft the owner edits. `core` are the core facts kept so far: when
/// there are some, they are the page (editing it again is a round trip).
pub fn draft(w: &Workspace, core: &[String]) -> String {
    let mut out = vec![
        format!(
            "# {} — the workspace map swarmux gives every session.",
            w.name
        ),
        "# One fact per line. Lines starting with # are dropped, and so are lines".into(),
        format!("# still saying {PLACEHOLDER}. Keep it short: at most {CORE_TOKENS} tokens."),
        "# Guesses come from the scan only (folders, stacks, commands): correct them.".into(),
        String::new(),
    ];
    if !core.is_empty() {
        out.extend(core.iter().cloned());
        out.push(String::new());
        return out.join("\n");
    }
    out.push(format!(
        "{} is <fill in: what the product is, in one line>.",
        w.name
    ));
    for r in &w.repos {
        let commands: Vec<&str> = r
            .commands
            .iter()
            .take(COMMANDS_SHOWN)
            .map(|c| c.line.as_str())
            .collect();
        let commands = if commands.is_empty() {
            String::new()
        } else {
            format!("; checks: {}", commands.join(", "))
        };
        out.push(format!(
            "Repository {} ({}/) is built with {}{commands}.",
            r.name,
            r.path.trim_end_matches('/'),
            stacks_text(&r.stacks)
        ));
    }
    let roles: Vec<(&str, Role)> = w
        .repos
        .iter()
        .map(|r| (r.name.as_str(), role(&r.stacks, &r.name)))
        .collect();
    for (client, _) in roles.iter().filter(|(_, r)| *r == Role::Client) {
        for (backend, _) in roles.iter().filter(|(_, r)| *r == Role::Backend) {
            out.push(format!(
                "{client} is a client of {backend} (guessed from their stacks; correct or delete)."
            ));
        }
    }
    out.push("Exceptions agents must respect: <fill in, one per line>.".into());
    out.push(String::new());
    out.join("\n")
}

/// The facts of an edited page: its lines that are neither hints nor
/// unfilled, trimmed, with list markers dropped, each once.
pub fn facts(page: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in page.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(COMMENT) || line.contains(PLACEHOLDER) {
            continue;
        }
        let line = line.trim_start_matches(['-', '*', '•']).trim().to_string();
        if !line.is_empty() && !out.contains(&line) {
            out.push(line);
        }
    }
    out
}

/// Why `facts` can't be the core, if they can't: too long in all.
pub fn check_size(facts: &[String]) -> Result<(), String> {
    let tokens: usize = facts.iter().map(|f| estimate_tokens(f) + 8).sum();
    if tokens > CORE_TOKENS {
        return Err(format!(
            "the map is about {tokens} tokens; keep it under {CORE_TOKENS}"
        ));
    }
    Ok(())
}
