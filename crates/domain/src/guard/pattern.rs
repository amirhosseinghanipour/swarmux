//! Permission rules in Claude Code's syntax: `Tool` or `Tool(specifier)`.

use std::fmt;

use super::glob::{glob_match, is_absolute, is_windows, match_segment, normalize, relative};
use super::shell::collapse;
use crate::DomainError;

/// Tools whose specifier is a shell command.
const COMMAND_TOOLS: [&str; 2] = ["Bash", "PowerShell"];
/// Tools whose specifier is a path glob, and the tools each rule covers.
const PATH_TOOLS: [(&str, &[&str]); 5] = [
    ("Edit", &["Edit", "MultiEdit", "NotebookEdit", "Write"]),
    ("Write", &["Write"]),
    ("Read", &["Read"]),
    ("MultiEdit", &["MultiEdit"]),
    ("NotebookEdit", &["NotebookEdit"]),
];

/// What the part inside the parentheses asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Spec {
    /// No parentheses: every use of the tool.
    Any,
    /// `Bash(cargo test:*)` or `Bash(git *)`: the command or the command plus arguments.
    Prefix(String),
    /// `Bash(git status)`: exactly this command.
    Exact(String),
    /// `Bash(git * main)`: `*` matches anything.
    Wildcard(String),
    /// `Edit(src/**)`: a path glob. `anchored` globs are relative to the
    /// session folder; others match at any depth.
    Path { glob: String, absolute: bool },
    /// `WebFetch(domain:example.com)`; `*.example.com` covers subdomains.
    Domain(String),
}

/// One allow or deny rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    tool: String,
    spec: Spec,
    /// The rule as written, for display.
    text: String,
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

fn invalid(pattern: &str, reason: &str) -> DomainError {
    DomainError::InvalidPattern {
        pattern: pattern.to_string(),
        reason: reason.to_string(),
    }
}

impl Pattern {
    /// Parses `Tool` or `Tool(specifier)`.
    pub fn parse(text: &str) -> Result<Pattern, DomainError> {
        let text = text.trim();
        let (tool, arg) = match text.find('(') {
            None => (text, None),
            Some(open) => {
                let Some(inner) = text[open + 1..].strip_suffix(')') else {
                    return Err(invalid(text, "missing closing parenthesis"));
                };
                (&text[..open], Some(inner))
            }
        };
        let tool = tool.trim();
        if tool.is_empty() {
            return Err(invalid(text, "missing tool name"));
        }
        if !tool
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '*' | '.'))
        {
            return Err(invalid(text, "tool names use letters, digits, _ and -"));
        }
        let spec = match arg.map(str::trim) {
            None => Spec::Any,
            Some("") => return Err(invalid(text, "empty parentheses")),
            Some("*") | Some("**") if !is_path_tool(tool) => Spec::Any,
            Some(arg) if COMMAND_TOOLS.contains(&tool) => command_spec(arg),
            Some(arg) if is_path_tool(tool) => path_spec(text, arg)?,
            Some(arg) if tool == "WebFetch" => match arg.strip_prefix("domain:") {
                Some(d) if !d.trim().is_empty() => Spec::Domain(d.trim().to_ascii_lowercase()),
                _ => return Err(invalid(text, "use WebFetch(domain:example.com)")),
            },
            Some(arg) => Spec::Exact(collapse(arg)),
        };
        Ok(Pattern {
            tool: tool.to_string(),
            spec,
            text: text.to_string(),
        })
    }

    /// The narrowest rule that allows exactly this call again.
    pub fn exact_for(tool: &str, subject: &str, cwd: &str) -> Pattern {
        let subject = subject.trim();
        let text = if subject.is_empty() || !has_specifier(tool) {
            tool.to_string()
        } else if COMMAND_TOOLS.contains(&tool) {
            format!("{tool}({})", collapse(subject))
        } else if is_path_tool(tool) {
            match relative(subject, cwd) {
                Some(rel) if !rel.is_empty() => format!("{tool}(/{})", escape_glob(&rel)),
                _ => match normalize(subject) {
                    Some(abs) if is_absolute(&abs) => {
                        format!("{tool}(//{})", escape_glob(abs.trim_start_matches('/')))
                    }
                    _ => tool.to_string(),
                },
            }
        } else if tool == "WebFetch" {
            match host_of(subject) {
                Some(host) => format!("WebFetch(domain:{host})"),
                None => tool.to_string(),
            }
        } else {
            tool.to_string()
        };
        Pattern::parse(&text).unwrap_or_else(|_| Pattern {
            tool: tool.to_string(),
            spec: Spec::Any,
            text: tool.to_string(),
        })
    }

    pub fn tool(&self) -> &str {
        &self.tool
    }

    /// True for a rule on a shell tool with a specifier (matched per command).
    pub fn is_command_rule(&self) -> bool {
        COMMAND_TOOLS.contains(&self.tool.as_str()) && self.spec != Spec::Any
    }

    /// Does the rule name this tool?
    pub fn covers_tool(&self, tool: &str) -> bool {
        if let Some(prefix) = self.tool.strip_suffix('*') {
            return tool.starts_with(prefix);
        }
        if let Some((_, tools)) = PATH_TOOLS.iter().find(|(t, _)| *t == self.tool) {
            return tools.contains(&tool);
        }
        self.tool == tool
    }

    /// Matches one simple shell command (already collapsed).
    pub fn matches_command(&self, cmd: &str) -> bool {
        match &self.spec {
            Spec::Any => true,
            Spec::Prefix(p) => {
                cmd == p
                    || cmd
                        .strip_prefix(p.as_str())
                        .is_some_and(|r| r.starts_with(' '))
            }
            Spec::Exact(e) => cmd == e,
            Spec::Wildcard(w) => match_segment(w, cmd, false),
            Spec::Path { .. } | Spec::Domain(_) => false,
        }
    }

    /// Matches a call of a non-shell tool (or any use of a shell tool).
    pub fn matches(&self, tool: &str, subject: &str, cwd: &str) -> bool {
        if !self.covers_tool(tool) {
            return false;
        }
        match &self.spec {
            Spec::Any => true,
            Spec::Path { glob, absolute } => path_matches(glob, *absolute, subject, cwd),
            Spec::Domain(d) => host_of(subject).is_some_and(|h| domain_matches(d, &h)),
            Spec::Exact(e) => collapse(subject) == *e,
            Spec::Prefix(_) | Spec::Wildcard(_) => false,
        }
    }
}

fn is_path_tool(tool: &str) -> bool {
    PATH_TOOLS.iter().any(|(t, _)| *t == tool)
}

/// Tools for which `exact_for` writes a specifier.
fn has_specifier(tool: &str) -> bool {
    COMMAND_TOOLS.contains(&tool) || is_path_tool(tool) || tool == "WebFetch"
}

fn command_spec(arg: &str) -> Spec {
    let arg = collapse(arg);
    if let Some(p) = arg.strip_suffix(":*") {
        return Spec::Prefix(collapse(p));
    }
    if let Some(p) = arg.strip_suffix(" *")
        && !p.contains('*')
    {
        return Spec::Prefix(p.to_string());
    }
    if arg.contains('*') {
        Spec::Wildcard(arg)
    } else {
        Spec::Exact(arg)
    }
}

/// Gitignore-like: `//abs` or `C:/abs` is absolute, a leading `/` or any
/// inner `/` anchors at the session folder, a bare name matches at any depth.
fn path_spec(text: &str, arg: &str) -> Result<Spec, DomainError> {
    let arg = arg.replace('\\', "/");
    if arg.starts_with('~') {
        return Err(invalid(
            text,
            "~ paths aren't supported; use //absolute/path",
        ));
    }
    if let Some(abs) = arg.strip_prefix("//") {
        return Ok(Spec::Path {
            glob: format!("/{abs}"),
            absolute: true,
        });
    }
    if is_windows(&arg) {
        return Ok(Spec::Path {
            glob: arg,
            absolute: true,
        });
    }
    let arg = arg.strip_prefix("./").unwrap_or(&arg);
    if arg.split('/').any(|s| s == "..") {
        return Err(invalid(text, ".. isn't allowed in path rules"));
    }
    let glob = match arg.strip_prefix('/') {
        Some(anchored) => anchored.to_string(),
        None if arg.trim_end_matches('/').contains('/') => arg.to_string(),
        None => format!("**/{arg}"),
    };
    Ok(Spec::Path {
        glob,
        absolute: false,
    })
}

fn path_matches(glob: &str, absolute: bool, subject: &str, cwd: &str) -> bool {
    let fold = is_windows(cwd) || is_windows(subject) || is_windows(glob);
    if absolute {
        return super::glob::absolute(subject, cwd).is_some_and(|abs| glob_match(glob, &abs, fold));
    }
    relative(subject, cwd).is_some_and(|rel| glob_match(glob, &rel, fold))
}

fn escape_glob(path: &str) -> String {
    path.replace(['*', '?'], "?")
}

/// Host of a URL, lower-cased, without user info or port.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.trim().split_once("://").map_or(url.trim(), |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = if host.starts_with('[') {
        host.split(']').next().map(|h| format!("{h}]"))?
    } else {
        host.split(':').next()?.to_string()
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

fn domain_matches(rule: &str, host: &str) -> bool {
    match rule.strip_prefix("*.") {
        Some(base) => host
            .strip_suffix(base)
            .is_some_and(|sub| sub.ends_with('.')),
        None => host == rule,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Pattern {
        Pattern::parse(s).unwrap()
    }

    #[test]
    fn parse_errors_are_reported() {
        for bad in [
            "",
            "(x)",
            "Bash(",
            "Bash()",
            "Ba sh",
            "WebFetch(example.com)",
            "Edit(~/x)",
            "Edit(../x)",
        ] {
            assert!(
                matches!(Pattern::parse(bad), Err(DomainError::InvalidPattern { .. })),
                "{bad:?} should not parse"
            );
        }
    }

    #[test]
    fn display_is_the_rule_as_written() {
        assert_eq!(p(" Bash(cargo test:*) ").to_string(), "Bash(cargo test:*)");
    }

    #[test]
    fn bare_tool_matches_any_use() {
        assert!(p("Bash").matches("Bash", "rm x", "/w"));
        assert!(p("Bash").matches_command("anything"));
        assert!(!p("Bash").matches("PowerShell", "x", "/w"));
        assert!(p("Bash(*)").matches("Bash", "x", "/w"));
        assert!(p("mcp__github__*").matches("mcp__github__create_issue", "", "/w"));
    }

    #[test]
    fn command_prefix_respects_word_boundary() {
        let rule = p("Bash(cargo test:*)");
        assert!(rule.matches_command("cargo test"));
        assert!(rule.matches_command("cargo test --workspace"));
        assert!(!rule.matches_command("cargo testing"));
        assert!(!rule.matches_command("cargo build"));
        let new_style = p("Bash(git *)");
        assert!(new_style.matches_command("git"));
        assert!(new_style.matches_command("git status"));
        assert!(!new_style.matches_command("gitk"));
    }

    #[test]
    fn command_exact_and_wildcard() {
        assert!(p("Bash(git status)").matches_command("git status"));
        assert!(!p("Bash(git status)").matches_command("git status -s"));
        assert!(p("Bash(git  status )").matches_command("git status"));
        let w = p("Bash(git push * main)");
        assert!(w.matches_command("git push origin main"));
        assert!(!w.matches_command("git push origin dev"));
    }

    #[test]
    fn path_globs_are_relative_to_cwd() {
        let rule = p("Edit(src/**)");
        assert!(rule.matches("Edit", "/w/src/a/b.rs", "/w"));
        assert!(rule.matches("Edit", "src/a.rs", "/w"));
        assert!(
            rule.matches("Write", "/w/src/new.rs", "/w"),
            "Edit rules cover Write"
        );
        assert!(!rule.matches("Edit", "/w/tests/a.rs", "/w"));
        assert!(!rule.matches("Edit", "/other/src/a.rs", "/w"));
        assert!(!rule.matches("Edit", "/w/src/../../etc/passwd", "/w"));
        assert!(!rule.matches("Read", "/w/src/a.rs", "/w"));
        assert!(!p("Write(src/**)").matches("Edit", "/w/src/a.rs", "/w"));
    }

    #[test]
    fn windows_paths_match() {
        let rule = p("Edit(src/**)");
        assert!(rule.matches("Edit", r"C:\Proj\SRC\main.rs", r"c:\proj"));
        assert!(p(r"Read(C:\data\**)").matches("Read", r"c:\Data\x.csv", r"C:\proj"));
    }

    #[test]
    fn bare_names_match_at_any_depth() {
        let rule = p("Read(**/*.md)");
        assert!(rule.matches("Read", "/w/README.md", "/w"));
        assert!(rule.matches("Read", "/w/docs/a/b.md", "/w"));
        let env = p("Read(.env)");
        assert!(env.matches("Read", "/w/.env", "/w"));
        assert!(env.matches("Read", "/w/app/.env", "/w"));
        let anchored = p("Read(/.env)");
        assert!(!anchored.matches("Read", "/w/app/.env", "/w"));
    }

    #[test]
    fn absolute_path_rules() {
        let rule = p("Read(//etc/**)");
        assert!(rule.matches("Read", "/etc/passwd", "/w"));
        assert!(!rule.matches("Read", "/w/etc/x", "/w"));
    }

    #[test]
    fn webfetch_domains() {
        let rule = p("WebFetch(domain:example.com)");
        assert!(rule.matches("WebFetch", "https://example.com/a?b", "/w"));
        assert!(rule.matches("WebFetch", "https://user@EXAMPLE.com:8080/", "/w"));
        assert!(!rule.matches("WebFetch", "https://example.com.evil.io/", "/w"));
        assert!(!rule.matches("WebFetch", "https://docs.example.com/", "/w"));
        let sub = p("WebFetch(domain:*.example.com)");
        assert!(sub.matches("WebFetch", "https://docs.example.com/", "/w"));
        assert!(!sub.matches("WebFetch", "https://badexample.com/", "/w"));
    }

    #[test]
    fn exact_for_builds_narrow_rules() {
        let cwd = "/w";
        assert_eq!(
            Pattern::exact_for("Bash", "cargo  test", cwd).to_string(),
            "Bash(cargo test)"
        );
        assert_eq!(
            Pattern::exact_for("Edit", "/w/src/a.rs", cwd).to_string(),
            "Edit(/src/a.rs)"
        );
        assert_eq!(
            Pattern::exact_for("Read", "/etc/hosts", cwd).to_string(),
            "Read(//etc/hosts)"
        );
        assert_eq!(
            Pattern::exact_for("WebFetch", "https://docs.rs/x", cwd).to_string(),
            "WebFetch(domain:docs.rs)"
        );
        assert_eq!(
            Pattern::exact_for("TodoWrite", "x", cwd).to_string(),
            "TodoWrite"
        );
        let rule = Pattern::exact_for("Edit", r"C:\p\src\a.rs", r"C:\p");
        assert!(rule.matches("Edit", r"C:\p\src\a.rs", r"C:\p"));
        assert!(!rule.matches("Edit", r"C:\p\src\b.rs", r"C:\p"));
    }

    #[test]
    fn hosts() {
        assert_eq!(host_of("https://a.b/c").as_deref(), Some("a.b"));
        assert_eq!(host_of("a.b").as_deref(), Some("a.b"));
        assert_eq!(host_of("https://[::1]:80/").as_deref(), Some("[::1]"));
        assert_eq!(host_of(""), None);
    }
}
