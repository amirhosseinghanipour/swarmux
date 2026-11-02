//! Token counts and what they cost, per model.

/// Tokens of one message (or a sum of messages), split the way they are billed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    /// Prompt cache writes with the 5-minute lifetime.
    pub cache_write_5m: u64,
    /// Prompt cache writes with the 1-hour lifetime.
    pub cache_write_1h: u64,
}

impl TokenUsage {
    pub fn cache_write(&self) -> u64 {
        self.cache_write_5m + self.cache_write_1h
    }

    /// Every token read or written.
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write()
    }

    /// The prompt the model saw: input plus cache reads and writes.
    pub fn context(&self) -> u64 {
        self.input + self.cache_read + self.cache_write()
    }

    pub fn add(&mut self, other: &TokenUsage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write_5m += other.cache_write_5m;
        self.cache_write_1h += other.cache_write_1h;
    }

    /// Share of the prompt served from cache; `None` without any prompt tokens.
    pub fn cache_hit_ratio(&self) -> Option<f64> {
        let prompt = self.context();
        (prompt > 0).then(|| self.cache_read as f64 / prompt as f64)
    }
}

/// US dollars per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
}

impl ModelPrice {
    /// The usual shape: cache writes at 1.25× (5 min) and 2× (1 h) the input price.
    pub fn standard(input: f64, output: f64, cache_read: f64) -> ModelPrice {
        ModelPrice {
            input,
            output,
            cache_write_5m: input * 1.25,
            cache_write_1h: input * 2.0,
            cache_read,
        }
    }

    pub fn cost(&self, u: &TokenUsage) -> f64 {
        (u.input as f64 * self.input
            + u.output as f64 * self.output
            + u.cache_read as f64 * self.cache_read
            + u.cache_write_5m as f64 * self.cache_write_5m
            + u.cache_write_1h as f64 * self.cache_write_1h)
            / 1_000_000.0
    }
}

/// Anthropic list prices (USD per MTok: input, output, cache read), keyed by
/// model id without the `claude-` prefix or a date stamp. Aliases ("opus")
/// mean the current model of the family.
const BUILTIN: &[(&str, f64, f64, f64)] = &[
    ("fable-5-1", 10.0, 50.0, 0.25),
    ("mythos-5-1", 10.0, 50.0, 0.25),
    ("fable-5", 10.0, 50.0, 1.0),
    ("mythos-5", 10.0, 50.0, 1.0),
    ("opus-5-5", 4.0, 20.0, 0.20),
    ("opus-5", 5.0, 25.0, 0.50),
    ("opus-4-8", 5.0, 25.0, 0.50),
    ("opus-4-7", 5.0, 25.0, 0.50),
    ("opus-4-6", 5.0, 25.0, 0.50),
    ("opus-4-5", 5.0, 25.0, 0.50),
    ("opus-4-1", 15.0, 75.0, 1.50),
    ("opus-4", 15.0, 75.0, 1.50),
    ("3-opus", 15.0, 75.0, 1.50),
    ("sonnet-5-5", 2.0, 10.0, 0.20),
    ("sonnet-5", 2.0, 10.0, 0.20),
    ("sonnet-4-6", 3.0, 15.0, 0.30),
    ("sonnet-4-5", 3.0, 15.0, 0.30),
    ("sonnet-4", 3.0, 15.0, 0.30),
    ("3-7-sonnet", 3.0, 15.0, 0.30),
    ("3-5-sonnet", 3.0, 15.0, 0.30),
    ("haiku-5-5", 0.10, 0.50, 0.01),
    ("haiku-4-5", 1.0, 5.0, 0.10),
    ("3-5-haiku", 0.80, 4.0, 0.08),
    ("3-haiku", 0.25, 1.25, 0.03),
    ("fable", 10.0, 50.0, 0.25),
    ("opus", 4.0, 20.0, 0.20),
    ("sonnet", 2.0, 10.0, 0.20),
    ("haiku", 0.10, 0.50, 0.01),
];

/// Prices per model, the built-in list plus the user's overrides.
#[derive(Clone, Debug, PartialEq)]
pub struct PriceTable {
    entries: Vec<(String, ModelPrice)>,
}

impl Default for PriceTable {
    fn default() -> PriceTable {
        PriceTable::builtin()
    }
}

impl PriceTable {
    pub fn builtin() -> PriceTable {
        PriceTable {
            entries: BUILTIN
                .iter()
                .map(|&(k, i, o, r)| (k.to_string(), ModelPrice::standard(i, o, r)))
                .collect(),
        }
    }

    /// Sets the price of `model` (any spelling Claude Code reports).
    pub fn set(&mut self, model: &str, price: ModelPrice) {
        let key = model_key(model);
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = price,
            None => self.entries.push((key, price)),
        }
    }

    /// The price of `model`: an exact id or alias, else the longest versioned
    /// id it extends (`opus-4-5-xyz` → `opus-4-5`). Unknown models give `None`.
    pub fn price_for(&self, model: &str) -> Option<&ModelPrice> {
        let key = model_key(model);
        if let Some((_, p)) = self.entries.iter().find(|(k, _)| *k == key) {
            return Some(p);
        }
        self.entries
            .iter()
            .filter(|(k, _)| k.chars().any(|c| c.is_ascii_digit()))
            .filter(|(k, _)| {
                key.strip_prefix(k.as_str())
                    .is_some_and(|rest| rest.starts_with('-'))
            })
            .max_by_key(|(k, _)| k.len())
            .map(|(_, p)| p)
    }

    pub fn cost(&self, model: &str, usage: &TokenUsage) -> Option<f64> {
        self.price_for(model).map(|p| p.cost(usage))
    }
}

/// Normal form of a model id: lower case, no provider prefix, no `claude-`,
/// no `[1m]`, `@version`, `-v1:0` or date stamp.
pub fn model_key(model: &str) -> String {
    let m = model.trim().to_ascii_lowercase();
    let m = match m.find("claude-") {
        Some(i) => &m[i + "claude-".len()..],
        None => m.as_str(),
    };
    let m = m.split(['[', '@']).next().unwrap_or_default();
    let mut parts: Vec<&str> = m.split('-').collect();
    while let Some(last) = parts.last() {
        let is_date = last.len() == 8 && last.chars().all(|c| c.is_ascii_digit());
        let is_rev = last.starts_with('v') && last.contains(':');
        if parts.len() > 1 && (is_date || is_rev) {
            parts.pop();
        } else {
            break;
        }
    }
    parts.join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, output: u64, read: u64, w5: u64, w1: u64) -> TokenUsage {
        TokenUsage {
            input,
            output,
            cache_read: read,
            cache_write_5m: w5,
            cache_write_1h: w1,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn keys_drop_prefixes_suffixes_and_dates() {
        assert_eq!(model_key("claude-opus-5-5"), "opus-5-5");
        assert_eq!(model_key("claude-opus-4-6[1m]"), "opus-4-6");
        assert_eq!(model_key("claude-sonnet-4-5-20250929"), "sonnet-4-5");
        assert_eq!(
            model_key("us.anthropic.claude-haiku-4-5-20251001-v1:0"),
            "haiku-4-5"
        );
        assert_eq!(model_key("claude-opus-4-5@20251101"), "opus-4-5");
        assert_eq!(model_key("Sonnet"), "sonnet");
    }

    #[test]
    fn prices_by_exact_id_then_longest_prefix() {
        let t = PriceTable::builtin();
        let input = |m: &str| t.price_for(m).map(|p| p.input);
        assert_eq!(input("claude-opus-5-5"), Some(4.0));
        assert_eq!(input("claude-opus-5"), Some(5.0));
        assert_eq!(input("claude-opus-4-20250514"), Some(15.0));
        assert_eq!(input("claude-3-5-haiku-20241022"), Some(0.8));
        assert_eq!(input("opus"), Some(4.0));
        // An unknown version is not priced as some other model.
        assert_eq!(input("claude-opus-9-9"), None);
        assert_eq!(input("<synthetic>"), None);
        assert_eq!(input(""), None);
    }

    #[test]
    fn cost_of_every_token_kind() {
        let t = PriceTable::builtin();
        let u = usage(1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000);
        // Sonnet 4.6: 3 + 15 + 0.30 + 3.75 + 6.
        assert!(close(t.cost("claude-sonnet-4-6", &u).unwrap(), 28.05));
        // Opus 5.5: 4 + 20 + 0.20 + 5 + 8.
        assert!(close(t.cost("claude-opus-5-5", &u).unwrap(), 37.2));
        let small = usage(2, 214, 37_233, 0, 41_225);
        let want = (2.0 * 4.0 + 214.0 * 20.0 + 37_233.0 * 0.2 + 41_225.0 * 8.0) / 1e6;
        assert!(close(t.cost("claude-opus-5-5", &small).unwrap(), want));
    }

    #[test]
    fn overrides_replace_or_add() {
        let mut t = PriceTable::builtin();
        t.set("claude-opus-5-5", ModelPrice::standard(1.0, 2.0, 0.1));
        assert_eq!(t.price_for("claude-opus-5-5").map(|p| p.input), Some(1.0));
        t.set("claude-opus-9-9", ModelPrice::standard(9.0, 9.0, 0.9));
        assert_eq!(
            t.price_for("claude-opus-9-9-20300101").map(|p| p.input),
            Some(9.0)
        );
    }

    #[test]
    fn usage_sums_and_ratios() {
        let mut a = usage(10, 5, 80, 6, 4);
        assert_eq!((a.cache_write(), a.context(), a.total()), (10, 100, 105));
        assert_eq!(a.cache_hit_ratio(), Some(0.8));
        a.add(&usage(1, 1, 1, 1, 1));
        assert_eq!(a, usage(11, 6, 81, 7, 5));
        assert_eq!(TokenUsage::default().cache_hit_ratio(), None);
    }
}
