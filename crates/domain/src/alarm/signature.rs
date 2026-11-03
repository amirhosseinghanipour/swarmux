//! What makes two asks the same ask: the tool, the pattern of the call and
//! the risk tier. Asks with one signature may be answered together; asks of
//! different tiers never are.

use super::risk::{RiskTier, is_shell, program_and_args};

/// Programs whose first argument names what they do.
const SUBCOMMANDS: [&str; 13] = [
    "git", "cargo", "npm", "pnpm", "yarn", "go", "docker", "gh", "kubectl", "dotnet", "make",
    "pip", "uv",
];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Signature {
    pub tool: String,
    pub pattern: String,
    pub tier: RiskTier,
}

impl Signature {
    /// `Bash cargo test (routine)`.
    pub fn label(&self) -> String {
        format!("{} {}", self.tool, self.pattern)
    }

    /// The key counts are kept under.
    pub fn key(&self) -> String {
        format!("{}\u{1f}{}", self.tool, self.pattern)
    }

    /// The allow rule that would cover this signature, in the Guard's syntax.
    pub fn rule(&self) -> String {
        if is_shell(&self.tool) {
            format!("{}({}:*)", self.tool, self.pattern)
        } else if self.pattern == "*" {
            self.tool.clone()
        } else {
            format!("{}({})", self.tool, self.pattern)
        }
    }
}

/// The signature of a call: for a shell command its program and, for the
/// programs that have them, its subcommand; for a file tool the folder; for
/// anything else the tool alone.
pub fn signature_of(tool: &str, subject: &str, tier: RiskTier) -> Signature {
    let pattern = if is_shell(tool) {
        subject
            .split(['\n', ';', '|', '&'])
            .find_map(program_and_args)
            .map_or_else(
                || "*".to_string(),
                |(program, args)| {
                    let sub = args.iter().find(|a| !a.starts_with('-'));
                    match sub {
                        Some(sub) if SUBCOMMANDS.contains(&program.as_str()) => {
                            format!("{program} {sub}")
                        }
                        _ => program,
                    }
                },
            )
    } else if matches!(
        tool,
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "Read"
    ) {
        let path = subject.trim().replace('\\', "/");
        match path.rsplit_once('/') {
            Some((dir, _)) if !dir.is_empty() => format!("{dir}/**"),
            _ => "**".to_string(),
        }
    } else {
        "*".to_string()
    };
    Signature {
        tool: tool.to_string(),
        pattern,
        tier,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pattern;

    #[test]
    fn shell_calls_are_named_by_program_and_subcommand() {
        let s = |cmd| signature_of("Bash", cmd, RiskTier::Routine);
        assert_eq!(s("cargo test checkout").pattern, "cargo test");
        assert_eq!(s("cargo test other --release").pattern, "cargo test");
        assert_eq!(s("ls -la").pattern, "ls");
        assert_eq!(s("C:\\tools\\Make.exe all").pattern, "make all");
    }

    #[test]
    fn file_tools_are_named_by_folder() {
        let s = signature_of("Edit", "src\\http\\retry.rs", RiskTier::Routine);
        assert_eq!(s.pattern, "src/http/**");
        assert_eq!(
            signature_of("Edit", "a.rs", RiskTier::Routine).pattern,
            "**"
        );
        assert_eq!(
            signature_of("WebFetch", "https://x", RiskTier::Elevated).pattern,
            "*"
        );
    }

    #[test]
    fn tier_is_part_of_the_signature() {
        let a = signature_of("Bash", "cargo test", RiskTier::Routine);
        let b = signature_of("Bash", "cargo test", RiskTier::Elevated);
        assert_ne!(a, b);
        assert_eq!(a.key(), b.key(), "counts are per call pattern");
    }

    #[test]
    fn rules_parse_in_the_guards_syntax() {
        for (tool, subject) in [
            ("Bash", "cargo test checkout"),
            ("Edit", "src/a.rs"),
            ("WebFetch", "https://x"),
        ] {
            let rule = signature_of(tool, subject, RiskTier::Routine).rule();
            assert!(Pattern::parse(&rule).is_ok(), "{rule}");
        }
        assert_eq!(
            signature_of("Bash", "cargo test", RiskTier::Routine).rule(),
            "Bash(cargo test:*)"
        );
    }
}
