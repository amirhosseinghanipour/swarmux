//! Splitting shell command lines into the simple commands they run.
//!
//! Not a shell parser: just enough to see every command in `a && b | c` and
//! inside `$(…)` or backticks, honouring quotes.

/// Simple commands in `line`, trimmed and with runs of spaces collapsed.
///
/// Splits on `&&`, `||`, `;`, `|`, `&`, newlines, and the edges of `$(…)`,
/// `` `…` ``, `<(…)` and `(…)` sub-shells, outside quotes.
pub fn split_commands(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some('\''), '\'') => {
                quote = None;
                cur.push(c);
            }
            (Some('\''), _) => cur.push(c),
            (Some('"'), '"') => {
                quote = None;
                cur.push(c);
            }
            (Some('"'), '\\') => {
                cur.push(c);
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            // Command substitution still runs inside double quotes.
            (Some('"'), '`') => flush(&mut parts, &mut cur),
            (Some('"'), '$') if chars.peek() == Some(&'(') => {
                chars.next();
                flush(&mut parts, &mut cur);
            }
            (Some('"'), ')') => flush(&mut parts, &mut cur),
            (Some(_), _) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                cur.push(c);
            }
            (None, '\\') => {
                cur.push(c);
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (None, ';' | '|' | '&' | '\n' | '\r' | '`' | '(' | ')') => flush(&mut parts, &mut cur),
            (None, '$' | '<' | '>') if chars.peek() == Some(&'(') => {
                chars.next();
                flush(&mut parts, &mut cur);
            }
            (None, _) => cur.push(c),
        }
    }
    flush(&mut parts, &mut cur);
    parts
}

fn flush(parts: &mut Vec<String>, cur: &mut String) {
    let collapsed = collapse(cur);
    if !collapsed.is_empty() {
        parts.push(collapsed);
    }
    cur.clear();
}

/// Trims and collapses runs of whitespace into one space.
pub fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// True when the line runs other commands through substitution or sub-shells,
/// which prefix rules can't vouch for.
pub fn has_substitution(line: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut prev = ' ';
    for c in line.chars() {
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), _) => {}
            (_, '`') => return true,
            (_, '(') if matches!(prev, '$' | '<' | '>') => return true,
            (None, '(') => return true,
            (None, '\'' | '"') => quote = Some(c),
            _ => {}
        }
        prev = c;
    }
    false
}

/// `cmd` without leading `VAR=value` assignments and wrappers such as `sudo`
/// or `env`, so deny rules see the command that really runs.
pub fn strip_wrappers(cmd: &str) -> &str {
    const WRAPPERS: [&str; 8] = [
        "sudo", "env", "nohup", "time", "command", "exec", "nice", "builtin",
    ];
    let mut rest = cmd.trim_start();
    loop {
        let word_end = rest.find(' ').unwrap_or(rest.len());
        let word = &rest[..word_end];
        let is_assignment = word.split_once('=').is_some_and(|(k, _)| {
            !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '_')
        });
        if word.is_empty() || !(is_assignment || WRAPPERS.contains(&word)) || word_end == rest.len()
        {
            return rest;
        }
        rest = rest[word_end..].trim_start();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_compound_commands() {
        assert_eq!(
            split_commands("cargo build && cargo  test; echo ok | tee x & y"),
            ["cargo build", "cargo test", "echo ok", "tee x", "y"]
        );
        assert_eq!(split_commands("a || b\nc"), ["a", "b", "c"]);
    }

    #[test]
    fn quotes_are_kept_together() {
        assert_eq!(
            split_commands(r#"echo "a && b" 'c; d'"#),
            [r#"echo "a && b" 'c; d'"#]
        );
        assert_eq!(split_commands(r"echo a\;b"), [r"echo a\;b"]);
    }

    #[test]
    fn substitutions_are_split_out() {
        assert_eq!(split_commands("echo $(rm -rf /)"), ["echo", "rm -rf /"]);
        assert_eq!(split_commands("echo `rm -rf /`"), ["echo", "rm -rf /"]);
        assert_eq!(
            split_commands(r#"echo "$(rm -rf ~)""#),
            [r#"echo ""#, "rm -rf ~", r#"""#]
        );
        assert_eq!(split_commands("(cd x; rm -rf /)"), ["cd x", "rm -rf /"]);
        assert_eq!(split_commands("echo '$(no)'"), ["echo '$(no)'"]);
    }

    #[test]
    fn substitution_detection() {
        assert!(has_substitution("echo $(x)"));
        assert!(has_substitution("echo `x`"));
        assert!(has_substitution("echo \"`x`\""));
        assert!(has_substitution("(a)"));
        assert!(!has_substitution("echo '$(x)'"));
        assert!(!has_substitution("cargo test -- --nocapture"));
    }

    #[test]
    fn wrappers_and_assignments_are_stripped() {
        assert_eq!(strip_wrappers("sudo rm -rf /"), "rm -rf /");
        assert_eq!(strip_wrappers("FOO=1 BAR=x env rm -rf ~"), "rm -rf ~");
        assert_eq!(strip_wrappers("cargo test"), "cargo test");
        assert_eq!(strip_wrappers("sudo"), "sudo");
        assert_eq!(strip_wrappers("a=b"), "a=b");
    }
}
