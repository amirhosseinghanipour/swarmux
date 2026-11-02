use std::fmt;

use crate::{DomainError, Millis};

const NAME_MAX: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PromptId(pub i64);

impl fmt::Display for PromptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The editable part of a prompt template, already validated.
#[derive(Clone, Debug, PartialEq)]
pub struct PromptDraft {
    pub name: String,
    pub body: String,
    pub tags: Vec<String>,
}

impl PromptDraft {
    /// Trims the name (non-empty, at most 64 characters); the body must not be blank.
    pub fn new(name: &str, body: &str, tags: Vec<String>) -> Result<PromptDraft, DomainError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DomainError::EmptyPromptName);
        }
        if name.chars().count() > NAME_MAX {
            return Err(DomainError::PromptNameTooLong { max: NAME_MAX });
        }
        if body.trim().is_empty() {
            return Err(DomainError::EmptyPromptBody);
        }
        Ok(PromptDraft {
            name: name.to_string(),
            body: body.to_string(),
            tags,
        })
    }
}

/// A saved, reusable prompt with `{{ variable }}` placeholders.
#[derive(Clone, Debug, PartialEq)]
pub struct PromptTemplate {
    pub id: PromptId,
    pub name: String,
    pub body: String,
    pub tags: Vec<String>,
    pub created_at: Millis,
    pub updated_at: Millis,
}

/// Splits comma-separated tags: trimmed, no empty or repeated tags.
pub fn parse_tags(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in text.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        if !tags.iter().any(|t| t == tag) {
            tags.push(tag.to_string());
        }
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_trims_name_and_keeps_body() {
        let d = PromptDraft::new("  review ", "  check {{x}}\n", vec![]).unwrap();
        assert_eq!(d.name, "review");
        assert_eq!(d.body, "  check {{x}}\n");
    }

    #[test]
    fn draft_rejects_bad_input() {
        assert_eq!(
            PromptDraft::new(" ", "b", vec![]),
            Err(DomainError::EmptyPromptName)
        );
        assert_eq!(
            PromptDraft::new(&"é".repeat(65), "b", vec![]),
            Err(DomainError::PromptNameTooLong { max: 64 })
        );
        assert!(PromptDraft::new(&"é".repeat(64), "b", vec![]).is_ok());
        assert_eq!(
            PromptDraft::new("n", " \n\t", vec![]),
            Err(DomainError::EmptyPromptBody)
        );
    }

    #[test]
    fn tags_are_trimmed_and_deduplicated() {
        assert_eq!(parse_tags(" rust, ,review,rust ,  "), ["rust", "review"]);
        assert!(parse_tags("").is_empty());
    }
}
