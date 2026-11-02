//! Model names as Claude Code reports them ("claude-opus-5-5", "sonnet", …).

/// The model families swarmux tells apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Opus,
    Sonnet,
    Haiku,
    Other,
}

impl Family {
    pub fn of(model: &str) -> Family {
        let m = model.to_ascii_lowercase();
        if m.contains("opus") {
            Family::Opus
        } else if m.contains("sonnet") {
            Family::Sonnet
        } else if m.contains("haiku") {
            Family::Haiku
        } else {
            Family::Other
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Family::Opus => "opus",
            Family::Sonnet => "sonnet",
            Family::Haiku => "haiku",
            Family::Other => "model",
        }
    }
}

/// "claude-opus-5-5" → "Opus 5.5", "sonnet" → "Sonnet", "" → "—".
pub fn display_name(model: &str) -> String {
    let m = model.trim();
    if m.is_empty() {
        return "—".into();
    }
    let rest = m.strip_prefix("claude-").unwrap_or(m);
    let rest = rest.split('[').next().unwrap_or(rest);
    let mut parts = rest.split('-');
    let Some(name) = parts.next().filter(|n| !n.is_empty()) else {
        return m.to_string();
    };
    // Keep only short numeric parts (versions), not date stamps.
    let version: Vec<&str> = parts
        .take_while(|p| p.len() <= 2 && p.chars().all(|c| c.is_ascii_digit()))
        .collect();
    let mut chars = name.chars();
    let title: String = chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    if version.is_empty() {
        title
    } else {
        format!("{title} {}", version.join("."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families() {
        assert_eq!(Family::of("claude-opus-5-5"), Family::Opus);
        assert_eq!(Family::of("Sonnet"), Family::Sonnet);
        assert_eq!(Family::of("claude-haiku-5-5"), Family::Haiku);
        assert_eq!(Family::of("fable"), Family::Other);
    }

    #[test]
    fn display_names() {
        assert_eq!(display_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(display_name("claude-sonnet-4-5-20250929"), "Sonnet 4.5");
        assert_eq!(display_name("sonnet"), "Sonnet");
        assert_eq!(display_name("claude-opus-4-6[1m]"), "Opus 4.6");
        assert_eq!(display_name(""), "—");
    }
}
