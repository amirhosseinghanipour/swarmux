use swarmux_domain::{DEFAULT_MODEL, GuardPolicy, Limits, Pattern, Unmatched};

/// User settings (`config.toml`); every field has a default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Jobs that may run at the same time.
    pub max_concurrent: usize,
    /// Queued jobs wait while free memory is below this.
    pub min_free_ram_mb: u64,
    /// Model for new sessions.
    pub default_model: String,
    pub guard: GuardSettings,
}

/// The `[guard]` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuardSettings {
    pub allow: Vec<Pattern>,
    pub deny: Vec<Pattern>,
    /// What happens to calls no rule matches.
    pub unmatched: Unmatched,
    /// Unanswered approvals are denied after this long.
    pub approval_timeout_secs: u64,
    /// Identical failures in a row that stop a job (0 = never).
    pub loop_repeats: u32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            max_concurrent: 3,
            min_free_ram_mb: 1536,
            default_model: DEFAULT_MODEL.to_string(),
            guard: GuardSettings::default(),
        }
    }
}

impl Default for GuardSettings {
    fn default() -> GuardSettings {
        GuardSettings {
            allow: Vec::new(),
            deny: Vec::new(),
            unmatched: Unmatched::Ask,
            approval_timeout_secs: 120,
            loop_repeats: 2,
        }
    }
}

impl Settings {
    pub fn limits(&self) -> Limits {
        Limits {
            max_concurrent: self.max_concurrent,
            min_free_ram: self.min_free_ram_mb * 1024 * 1024,
        }
    }
}

impl GuardSettings {
    /// The configured rules plus the built-in ones.
    pub fn policy(&self) -> GuardPolicy {
        GuardPolicy::new(self.allow.clone(), self.deny.clone(), self.unmatched)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_limits() {
        let s = Settings::default();
        assert_eq!((s.max_concurrent, s.default_model.as_str()), (3, "sonnet"));
        assert_eq!(s.limits().min_free_ram, 1536 * 1024 * 1024);
        assert_eq!(s.guard.unmatched, Unmatched::Ask);
        assert_eq!(
            (s.guard.approval_timeout_secs, s.guard.loop_repeats),
            (120, 2)
        );
    }

    #[test]
    fn policy_includes_builtin_rules() {
        let mut g = GuardSettings::default();
        g.allow.push(Pattern::parse("Bash(make:*)").unwrap());
        let p = g.policy();
        assert_eq!(p.allow_rules()[0].to_string(), "Bash(make:*)");
        assert!(p.deny_rules().len() >= swarmux_domain::BUILTIN_DENY.len());
    }
}
