//! How full a session's context window is.

use super::pricing::model_key;

/// At or above this share of the window Claude Code is close to compacting.
pub const NEAR_COMPACTION: f64 = 0.8;

const ONE_MILLION: u64 = 1_000_000;
const TWO_HUNDRED_K: u64 = 200_000;

/// Context used against the window, when the window is known.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContextGauge {
    /// Prompt tokens of the latest turn (input + cache read + cache write).
    pub used: u64,
    pub window: Option<u64>,
}

impl ContextGauge {
    pub fn new(used: u64, window: Option<u64>) -> ContextGauge {
        ContextGauge { used, window }
    }

    /// Used share of the window (may exceed 1 if the window guess is low).
    pub fn ratio(&self) -> Option<f64> {
        self.window
            .filter(|&w| w > 0)
            .map(|w| self.used as f64 / w as f64)
    }

    /// At least [`NEAR_COMPACTION`] of the window is used.
    pub fn near_compaction(&self) -> bool {
        self.ratio().is_some_and(|r| r >= NEAR_COMPACTION)
    }
}

/// The context window a model runs with in Claude Code, from its id: 1M for
/// `[1m]` ids and the current families, 200K for older ones. A context
/// already larger than 200K means 1M. Unknown models give `None`.
pub fn context_window(model: &str, used: u64) -> Option<u64> {
    if model.to_ascii_lowercase().contains("[1m]") || used > TWO_HUNDRED_K {
        return Some(ONE_MILLION);
    }
    let key = model_key(model);
    let has = |prefixes: &[&str]| {
        prefixes
            .iter()
            .any(|p| key == *p || key.starts_with(&format!("{p}-")))
    };
    // Bare aliases mean the current models.
    let alias = ["opus", "sonnet", "haiku", "fable"].contains(&key.as_str());
    if alias
        || has(&[
            "fable",
            "mythos",
            "opus-5",
            "sonnet-5",
            "haiku-5",
            "opus-4-6",
            "opus-4-7",
            "opus-4-8",
            "sonnet-4-6",
        ])
    {
        Some(ONE_MILLION)
    } else if has(&[
        "opus-4",
        "sonnet-4",
        "haiku-4",
        "3-7-sonnet",
        "3-5-sonnet",
        "3-5-haiku",
        "3-opus",
        "3-haiku",
    ]) {
        Some(TWO_HUNDRED_K)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_by_model() {
        assert_eq!(context_window("claude-opus-5-5", 10), Some(1_000_000));
        assert_eq!(
            context_window("claude-haiku-4-5-20251001", 10),
            Some(200_000)
        );
        assert_eq!(context_window("claude-sonnet-4-5[1m]", 10), Some(1_000_000));
        assert_eq!(
            context_window("claude-sonnet-4-5", 250_000),
            Some(1_000_000)
        );
        assert_eq!(context_window("mystery", 10), None);
        assert_eq!(context_window("<synthetic>", 0), None);
    }

    #[test]
    fn near_compaction_at_eighty_percent() {
        assert!(!ContextGauge::new(159_999, Some(200_000)).near_compaction());
        assert!(ContextGauge::new(160_000, Some(200_000)).near_compaction());
        assert!(!ContextGauge::new(900_000, None).near_compaction());
        assert_eq!(ContextGauge::new(50, Some(200)).ratio(), Some(0.25));
        assert_eq!(ContextGauge::new(50, Some(0)).ratio(), None);
    }
}
